//! A Model Context Protocol server over the automation tools.
//!
//! **Opt-in only.** Nothing in XmlCraft starts this server on its own: it runs when a user launches `xmlcraft-cli mcp` (usually by
//! adding that command to their agent's MCP configuration), and stops when its input closes. It opens no network port; the
//! transport is newline-delimited JSON-RPC 2.0 over stdin/stdout.
//!
//! Implemented: `initialize`, `ping`, `tools/list`, `tools/call`, and the `notifications/*` the client sends. Tool failures are
//! reported in the result (`isError: true`) so the agent can read them; protocol errors use JSON-RPC error codes.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

use crate::lines::{LineRead, drain_line, read_line_limited};
use crate::{Automation, Content, ToolError};

/// Protocol revisions we speak, newest first.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;

/// Longest JSON-RPC request line [`McpServer::serve`] buffers into memory (16 MiB).
pub const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

const INSTRUCTIONS: &str = "XmlCraft is an XML editor. app_info lists the tools and the version; command_list lists the commands the app \
registers. Tools take JSON arguments described by their input schemas; failures say what was wrong and what is allowed. Paths are confined \
to the server's root folder when one was given.";

pub struct McpServer {
    automation: Automation,
}

impl McpServer {
    pub fn new(automation: Automation) -> Self {
        Self { automation }
    }

    pub fn automation(&self) -> &Automation {
        &self.automation
    }

    /// Serve until `input` reaches end of file.
    pub fn serve(&mut self, mut input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        let mut buf = Vec::new();
        loop {
            match read_line_limited(&mut input, &mut buf, MAX_LINE_BYTES)? {
                LineRead::End => return Ok(()),
                LineRead::TooLong => {
                    drain_line(&mut input)?;
                    buf.clear();
                    buf.shrink_to(64 * 1024);
                    let reply = error(Value::Null, INVALID_REQUEST, &format!("request line exceeds {MAX_LINE_BYTES} bytes"));
                    writeln!(output, "{reply}")?;
                    output.flush()?;
                    continue;
                }
                LineRead::Line => {}
            }
            let reply = match std::str::from_utf8(&buf) {
                Ok(line) if line.trim().is_empty() => None,
                Ok(line) => self.handle_line(line),
                Err(e) => Some(error(Value::Null, PARSE_ERROR, &format!("parse error: {e}")).to_string()),
            };
            if buf.capacity() > 64 * 1024 {
                buf.clear();
                buf.shrink_to(64 * 1024);
            }
            if let Some(reply) = reply {
                writeln!(output, "{reply}")?;
                output.flush()?;
            }
        }
    }

    /// Handle one JSON-RPC message; returns the serialized reply, if one is due.
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(msg) => self.handle(&msg),
            Err(e) => Some(error(Value::Null, PARSE_ERROR, &format!("parse error: {e}"))),
        };
        reply.map(|r| r.to_string())
    }

    /// Handle one parsed message. Notifications (no `id`) get no reply.
    pub fn handle(&mut self, msg: &Value) -> Option<Value> {
        let Some(obj) = msg.as_object() else { return Some(error(Value::Null, INVALID_REQUEST, "expected a JSON-RPC request object")) };
        let id = obj.get("id").cloned();
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            // A response to something we sent (we send no requests) or garbage.
            return id.map(|id| error(id, INVALID_REQUEST, "missing method"));
        };
        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        // notifications/initialized, notifications/cancelled, …: nothing to answer
        let id = id?;
        let result = match xmlcraft_engine::guard(|| self.dispatch(method, &params)) {
            Ok(result) => result,
            Err(message) => Err((INTERNAL_ERROR, format!("internal error in {method}: {message}"))),
        };
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => error(id, code, &message),
        })
    }

    fn dispatch(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str);
                let version = asked.filter(|v| PROTOCOL_VERSIONS.contains(v)).unwrap_or(PROTOCOL_VERSIONS[0]);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "xmlcraft", "title": "XmlCraft", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_list() })),
            "tools/call" => {
                if !params.is_object() {
                    return Err((INVALID_PARAMS, "tools/call needs an object with a tool name and arguments".into()));
                }
                let name = params.get("name").and_then(Value::as_str).ok_or((INVALID_PARAMS, "tools/call needs a tool name".to_string()))?;
                let args = params.get("arguments").cloned().unwrap_or(Value::Null);
                match self.automation.call(name, &args) {
                    Ok(content) => Ok(call_result(content)),
                    Err(ToolError::UnknownTool(t)) => Err((INVALID_PARAMS, format!("unknown tool {t:?}"))),
                    Err(e) => Ok(tool_error(&e.to_string())),
                }
            }
            other => Err((METHOD_NOT_FOUND, format!("method not found: {other}"))),
        }
    }
}

fn tool_list() -> Vec<Value> {
    crate::tools()
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "title": t.title,
                "description": t.description,
                "inputSchema": t.input_schema,
                "annotations": {
                    "title": t.title,
                    "readOnlyHint": t.read_only,
                    "destructiveHint": t.destructive && !t.read_only,
                    "openWorldHint": false,
                },
            })
        })
        .collect()
}

/// A tool's results as MCP content: the JSON as text (for clients that read only text) and, for one object result, as
/// `structuredContent`.
fn call_result(content: Vec<Content>) -> Value {
    let texts: Vec<Value> = content
        .iter()
        .map(|Content::Json(v)| json!({ "type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()) }))
        .collect();
    let mut result = json!({ "content": texts, "isError": false });
    if let [Content::Json(v)] = content.as_slice()
        && v.is_object()
    {
        result["structuredContent"] = v.clone();
    }
    result
}

fn tool_error(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> McpServer {
        McpServer::new(Automation::new())
    }

    fn ask(s: &mut McpServer, msg: Value) -> Value {
        let reply = s.handle_line(&msg.to_string()).expect("a reply is due");
        serde_json::from_str(&reply).expect("the reply is JSON")
    }

    #[test]
    fn initialize_answers_with_a_version_we_speak_and_our_identity() {
        let mut s = server();
        let r = ask(&mut s, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}}));
        assert_eq!(r["id"], 1);
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(r["result"]["serverInfo"]["name"], "xmlcraft");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        assert!(r["result"]["capabilities"].get("resources").is_none(), "we offer no resources");
        let r = ask(&mut s, json!({"jsonrpc": "2.0", "id": "x", "method": "initialize", "params": {"protocolVersion": "1999-01-01"}}));
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSIONS[0], "an unknown revision gets our newest");
        assert_eq!(r["id"], "x", "a string id is echoed as is");
    }

    #[test]
    fn tools_list_describes_every_tool_with_a_schema_and_hints() {
        let mut s = server();
        let r = ask(&mut s, json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
        let tools = r["result"]["tools"].as_array().expect("a list");
        assert_eq!(tools.len(), crate::tools().len());
        for t in tools {
            assert!(t["name"].is_string() && t["description"].is_string());
            assert_eq!(t["inputSchema"]["type"], "object");
            assert!(t["annotations"]["readOnlyHint"].is_boolean());
        }
        let info = tools.iter().find(|t| t["name"] == "app_info").expect("app_info is listed");
        assert_eq!(info["annotations"]["readOnlyHint"], true);
        assert_eq!(info["annotations"]["destructiveHint"], false);
    }

    #[test]
    fn a_tool_call_returns_text_and_structured_content() {
        let mut s = server();
        let r = ask(&mut s, json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "app_info", "arguments": {}}}));
        assert_eq!(r["result"]["isError"], false);
        assert_eq!(r["result"]["structuredContent"]["name"], "XmlCraft");
        let text = r["result"]["content"][0]["text"].as_str().expect("text");
        assert!(text.contains("\"XmlCraft\""), "{text}");
    }

    #[test]
    fn a_failing_tool_is_reported_in_the_result_so_the_agent_can_read_it() {
        let mut s = server();
        let r = ask(
            &mut s,
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "command_list", "arguments": {"menu": "Nope"}}}),
        );
        assert!(r.get("error").is_none());
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().is_some_and(|t| t.contains("expected one of")));
    }

    #[test]
    fn protocol_problems_use_json_rpc_error_codes() {
        let mut s = server();
        let unknown = ask(&mut s, json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "nope"}}));
        assert_eq!(unknown["error"]["code"], INVALID_PARAMS);
        let nameless = ask(&mut s, json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {}}));
        assert_eq!(nameless["error"]["code"], INVALID_PARAMS);
        let badparams = ask(&mut s, json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": [1]}));
        assert_eq!(badparams["error"]["code"], INVALID_PARAMS);
        let method = ask(&mut s, json!({"jsonrpc": "2.0", "id": 8, "method": "resources/list"}));
        assert_eq!(method["error"]["code"], METHOD_NOT_FOUND);
        let ping = ask(&mut s, json!({"jsonrpc": "2.0", "id": 9, "method": "ping"}));
        assert_eq!(ping["result"], json!({}));
    }

    #[test]
    fn garbage_gets_an_error_and_notifications_get_nothing() {
        let mut s = server();
        assert_eq!(serde_json::from_str::<Value>(&s.handle_line("{not json").expect("an error reply")).expect("json")["error"]["code"], PARSE_ERROR);
        assert_eq!(serde_json::from_str::<Value>(&s.handle_line("[1,2]").expect("an error reply")).expect("json")["error"]["code"], INVALID_REQUEST);
        assert_eq!(serde_json::from_str::<Value>(&s.handle_line("3").expect("an error reply")).expect("json")["error"]["code"], INVALID_REQUEST);
        assert!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        assert!(s.handle_line(r#"{"jsonrpc":"2.0","id":1}"#).is_some(), "a request with no method is an error, not silence");
        assert!(s.handle_line(r#"{"jsonrpc":"2.0","result":{}}"#).is_none(), "a stray response is ignored");
    }

    #[test]
    fn serve_reads_lines_until_the_input_closes_and_survives_bad_input() {
        let mut s = server();
        let input =
            "\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\u{0}garbage\n\u{ff}\u{fe}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n";
        let mut out = Vec::new();
        s.serve(std::io::Cursor::new(input.as_bytes().to_vec()), &mut out).expect("serves");
        let replies: Vec<Value> = String::from_utf8(out).expect("utf-8").lines().map(|l| serde_json::from_str(l).expect("json")).collect();
        assert_eq!(replies.len(), 4, "two pings and two parse errors: {replies:?}");
        assert_eq!(replies[0]["id"], 1);
        assert_eq!(replies[1]["error"]["code"], PARSE_ERROR);
        assert_eq!(replies[3]["id"], 2, "the server went on after the bad lines");
    }
}
