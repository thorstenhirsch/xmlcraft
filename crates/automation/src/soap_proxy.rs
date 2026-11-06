//! `soap_proxy_start`, `soap_proxy_state`, `soap_proxy_breaks`, `soap_proxy_release`, `soap_proxy_drop` and `soap_proxy_stop`: the
//! debugging proxy of the SOAP Debugger window, driven by tools. It listens on this computer only, forwards what a client sends it to
//! one target and records every request and answer; with breaking on, messages are held until released (as they were or edited) or
//! dropped. It starts only when `soap_proxy_start` is called, needs the session's network to be allowed (as `soap_call` does), and ends
//! with `soap_proxy_stop` or when the session ends.

use std::time::Duration;

use serde_json::{Value, json};
use xmlcraft_engine::soap::{headers_text, parse_headers};
use xmlcraft_platform::proxy::{Breaks, Config, Debugger, Exchange, MAX_EXCHANGES, Message, Stage};

use crate::documents::{fail, json_result};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

fn breaks_properties() -> Value {
    json!({
        "requests": { "type": "boolean", "description": "Hold requests until they are released or dropped" },
        "responses": { "type": "boolean", "description": "Hold answers until they are released or dropped" },
        "contains": { "type": "string", "description": "Hold only messages whose body contains this text (default: all)" },
    })
}

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut start = breaks_properties();
    if let Some(map) = start.as_object_mut() {
        map.insert(
            "target".into(),
            json!({ "type": "string", "description": "Where requests are forwarded: http://host:port/base (the path of a request is added)" }),
        );
        map.insert(
            "port".into(),
            json!({ "type": "integer", "minimum": 0, "maximum": 65535, "description": "The port on this computer (default: one the system picks)" }),
        );
        map.insert(
            "allow_private_hosts".into(),
            json!({ "type": "boolean", "description": "Allow the target to be on this computer or the local network" }),
        );
        map.insert("timeout_seconds".into(), json!({ "type": "integer", "minimum": 1, "maximum": 300 }));
    }
    let id = json!({ "type": "integer", "minimum": 1, "description": "The exchange (from soap_proxy_state)" });
    vec![
        ToolDef {
            name: "soap_proxy_start",
            title: "Start the debugging proxy",
            description: "Start an intercepting proxy on this computer (127.0.0.1 only) that forwards what a client sends it to `target` and records every request and answer. \
                          Returns the `address` to point the client at (plain HTTP, one request to a connection). With `requests` or `responses` the messages are held, \
                          optionally only those that contain `contains`. This starts a listener and sends requests to the target, so the session's network must be allowed; \
                          a target on this computer or the local network needs `allow_private_hosts`. Stop it with soap_proxy_stop.",
            input_schema: json!({ "type": "object", "properties": start, "required": ["target"], "additionalProperties": false }),
            read_only: false,
            destructive: false,
            handler: soap_proxy_start,
        },
        ToolDef {
            name: "soap_proxy_state",
            title: "What the debugging proxy has seen",
            description: "The proxy's `address` (none when it is not started), its breaks, and its exchanges, oldest first (the most recent 500): the id, the `stage` (request_held, forwarding, \
                          answer_held, done, dropped, failed), the method and path, the request and the answer (headers and body as text), the status and the error.",
            input_schema: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: soap_proxy_state,
        },
        ToolDef {
            name: "soap_proxy_breaks",
            title: "Set what the proxy holds",
            description: "Hold requests and/or answers (optionally only those whose body contains `contains`) from now on. Messages already held stay held.",
            input_schema: json!({ "type": "object", "properties": breaks_properties(), "additionalProperties": false }),
            read_only: false,
            destructive: false,
            handler: soap_proxy_breaks,
        },
        ToolDef {
            name: "soap_proxy_release",
            title: "Release a held message",
            description: "Let the held request or answer of an exchange go on, as it was or as `body` and `headers` (`Name: value` lines) say. Nothing is changed unless they are given.",
            input_schema: json!({
                "type": "object",
                "properties": { "id": id.clone(), "body": { "type": "string" }, "headers": { "type": "string", "description": "Name: value, one to a line" } },
                "required": ["id"], "additionalProperties": false
            }),
            read_only: false,
            destructive: false,
            handler: soap_proxy_release,
        },
        ToolDef {
            name: "soap_proxy_drop",
            title: "Drop a held message",
            description: "Drop the held request or answer of an exchange: the client is told that it was dropped (502) and a dropped request never reaches the target.",
            input_schema: json!({ "type": "object", "properties": { "id": id }, "required": ["id"], "additionalProperties": false }),
            read_only: false,
            destructive: true,
            handler: soap_proxy_drop,
        },
        ToolDef {
            name: "soap_proxy_stop",
            title: "Stop the debugging proxy",
            description: "Stop listening; what is held is dropped. The exchanges are forgotten.",
            input_schema: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            read_only: false,
            destructive: false,
            handler: soap_proxy_stop,
        },
    ]
}

fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::HeldRequest => "request_held",
        Stage::Forwarding => "forwarding",
        Stage::HeldResponse => "answer_held",
        Stage::Done => "done",
        Stage::Dropped => "dropped",
        Stage::Failed => "failed",
    }
}

fn message_json(message: &Message) -> Value {
    json!({ "headers": headers_text(&message.headers), "body": String::from_utf8_lossy(&message.body) })
}

fn exchange_json(e: &Exchange) -> Value {
    json!({
        "id": e.id, "stage": stage_name(e.stage), "method": e.method, "path": e.path,
        "request": message_json(&e.request),
        "status": e.status.as_ref().map(|(code, reason)| json!({ "code": code, "reason": reason })),
        "response": e.status.as_ref().map(|_| message_json(&e.response)),
        "error": e.error,
    })
}

fn breaks_from(args: &Value, base: &Breaks) -> Result<Breaks> {
    let flag = |key: &str, default: bool| match args.get(key) {
        None => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(ToolError::InvalidArgs(format!("{key} is true or false"))),
    };
    let contains = match args.get("contains") {
        None => base.contains.clone(),
        Some(Value::String(s)) => s.clone(),
        Some(_) => return Err(ToolError::InvalidArgs("contains is a string".into())),
    };
    Ok(Breaks { requests: flag("requests", base.requests)?, responses: flag("responses", base.responses)?, contains })
}

fn id_arg(args: &Value) -> Result<u64> {
    args.get("id").and_then(Value::as_u64).ok_or_else(|| ToolError::InvalidArgs("`id` is required (a whole number from soap_proxy_state)".into()))
}

fn soap_proxy_start(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    if a.proxy.is_some() {
        return fail("the proxy is already started: stop it first (soap_proxy_stop)");
    }
    let target = args.get("target").and_then(Value::as_str).ok_or_else(|| ToolError::InvalidArgs("`target` is required".into()))?;
    let port = match args.get("port") {
        None => 0,
        Some(v) => {
            v.as_u64().and_then(|n| u16::try_from(n).ok()).ok_or_else(|| ToolError::InvalidArgs("port is a whole number from 0 to 65535".into()))?
        }
    };
    let allow = match args.get("allow_private_hosts") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(ToolError::InvalidArgs("allow_private_hosts is true or false".into())),
    };
    let timeout = match args.get("timeout_seconds") {
        None => 60,
        Some(v) => v
            .as_u64()
            .filter(|n| (1..=300).contains(n))
            .ok_or_else(|| ToolError::InvalidArgs("timeout_seconds is a whole number from 1 to 300".into()))?,
    };
    let breaks = breaks_from(args, &Breaks::default())?;
    if !a.network().can_download() || a.network().is_offline() {
        return fail(
            "the session is offline, so nothing is forwarded: a person has to allow the network (the program gives the session a network that sends)",
        );
    }
    let debugger = Debugger::start(Config { port, target: target.to_owned(), allow_private_hosts: allow, timeout: Duration::from_secs(timeout) })
        .map_err(ToolError::Failed)?;
    debugger.set_breaks(breaks.clone());
    let address = debugger.address().to_string();
    a.proxy = Some(debugger);
    json_result(
        json!({ "address": address, "url": format!("http://{address}"), "requests": breaks.requests, "responses": breaks.responses, "contains": breaks.contains }),
    )
}

fn soap_proxy_state(a: &mut Automation, _args: &Value) -> Result<Vec<Content>> {
    let Some(proxy) = &a.proxy else { return json_result(json!({ "address": null, "exchanges": [] })) };
    let breaks = proxy.breaks();
    let exchanges = proxy.exchanges();
    let skipped = exchanges.len().saturating_sub(MAX_EXCHANGES);
    json_result(json!({
        "address": proxy.address().to_string(),
        "breaks": { "requests": breaks.requests, "responses": breaks.responses, "contains": breaks.contains },
        "exchanges": exchanges.iter().skip(skipped).map(exchange_json).collect::<Vec<_>>(),
    }))
}

fn started(a: &Automation) -> Result<&Debugger> {
    a.proxy.as_ref().ok_or_else(|| ToolError::Failed("the proxy is not started (soap_proxy_start)".into()))
}

fn soap_proxy_breaks(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let proxy = started(a)?;
    let breaks = breaks_from(args, &proxy.breaks())?;
    proxy.set_breaks(breaks.clone());
    json_result(json!({ "requests": breaks.requests, "responses": breaks.responses, "contains": breaks.contains }))
}

fn soap_proxy_release(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let id = id_arg(args)?;
    let proxy = started(a)?;
    let held = proxy.exchanges().into_iter().find(|e| e.id == id).filter(|e| matches!(e.stage, Stage::HeldRequest | Stage::HeldResponse));
    let Some(held) = held else { return fail(format!("exchange {id} is not held")) };
    let current = if held.stage == Stage::HeldRequest { &held.request } else { &held.response };
    let edited = match (args.get("body"), args.get("headers")) {
        (None, None) => None,
        (body, headers) => {
            let mut message = current.clone();
            match body {
                None => {}
                Some(Value::String(b)) => message.body = b.clone().into_bytes(),
                Some(_) => return Err(ToolError::InvalidArgs("body is a string".into())),
            }
            match headers {
                None => {}
                Some(Value::String(h)) => message.headers = parse_headers(h).map_err(ToolError::InvalidArgs)?,
                Some(_) => return Err(ToolError::InvalidArgs("headers is a string of `Name: value` lines".into())),
            }
            Some(message)
        }
    };
    let released = proxy.release(id, edited);
    json_result(json!({ "released": released, "id": id, "was": stage_name(held.stage) }))
}

fn soap_proxy_drop(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let id = id_arg(args)?;
    let dropped = started(a)?.drop_held(id);
    if !dropped {
        return fail(format!("exchange {id} is not held"));
    }
    json_result(json!({ "dropped": true, "id": id }))
}

fn soap_proxy_stop(a: &mut Automation, _args: &Value) -> Result<Vec<Content>> {
    let was = a.proxy.take().map(|p| p.address().to_string());
    json_result(json!({ "stopped": was.is_some(), "address": was }))
}
