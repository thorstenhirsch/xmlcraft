//! What `xmlcraft-cli` does: parse the command line, then run it.
//!
//! - `tools [--json]`: the tools and their input schemas.
//! - `run <tool> [key=value …] [--root DIR]`: call one tool. A value is read as JSON when it is (`3`, `true`, `["a"]`), else as text.
//! - `run --script FILE [--root DIR] [--keep-going]`: run a JSON list of steps.
//! - `mcp [--root DIR]`: serve the tools over MCP on standard input and output (opt-in; nothing starts it on its own).
//! - `ui --control FILE <method> [key=value …] [--out FILE.png]`: drive a running app that was started with `--control FILE`.
//!
//! Results are JSON on standard output; messages for a person go to standard error. The exit code is 0 for success, 1 when a tool or
//! request failed, and 2 for a command line that could not be understood.

use std::io::Write;
use std::path::PathBuf;

use serde_json::{Map, Value, json};
use xmlcraft_automation::control::ControlClient;
use xmlcraft_automation::{Automation, Content};

pub const USAGE: &str = "Usage:
  xmlcraft-cli tools [--json]
  xmlcraft-cli run <tool> [key=value ...] [--root DIR]
  xmlcraft-cli run --script FILE [--root DIR] [--keep-going]
  xmlcraft-cli mcp [--root DIR]
  xmlcraft-cli ui --control FILE <method> [key=value ...] [--out FILE.png]
  xmlcraft-cli --version | --help

A key=value value is read as JSON when it is valid JSON (3, true, [\"a\"]), and as text otherwise.
--root DIR confines every path a tool reads or writes to DIR.";

#[derive(Debug, PartialEq)]
pub enum Command {
    Version,
    Help,
    Tools {
        json: bool,
    },
    Run {
        tool: String,
        args: Value,
        root: Option<PathBuf>,
    },
    Script {
        file: PathBuf,
        root: Option<PathBuf>,
        keep_going: bool,
    },
    #[cfg(feature = "mcp")]
    Mcp {
        root: Option<PathBuf>,
    },
    Ui {
        control: PathBuf,
        method: String,
        params: Value,
        out: Option<PathBuf>,
    },
}

/// One `key=value` argument: the value is JSON when it parses as JSON, otherwise a string.
fn key_value(arg: &str) -> Result<(String, Value), String> {
    let (key, value) = arg.split_once('=').ok_or_else(|| format!("expected key=value, got {arg:?}"))?;
    if key.is_empty() {
        return Err(format!("the key is empty in {arg:?}"));
    }
    Ok((key.to_owned(), serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_owned()))))
}

fn object(pairs: Vec<(String, Value)>) -> Value {
    let mut map = Map::new();
    for (k, v) in pairs {
        map.insert(k, v);
    }
    Value::Object(map)
}

/// The value of an option such as `--root DIR`, taken out of `rest`.
fn take_option(rest: &mut Vec<String>, name: &str) -> Result<Option<String>, String> {
    let Some(i) = rest.iter().position(|a| a == name) else { return Ok(None) };
    if i + 1 >= rest.len() {
        return Err(format!("{name} needs a value"));
    }
    let value = rest.remove(i + 1);
    rest.remove(i);
    if rest.iter().any(|a| a == name) {
        return Err(format!("{name} given twice"));
    }
    Ok(Some(value))
}

fn take_flag(rest: &mut Vec<String>, name: &str) -> bool {
    let before = rest.len();
    rest.retain(|a| a != name);
    rest.len() != before
}

fn no_extra(rest: &[String]) -> Result<(), String> {
    match rest.first() {
        None => Ok(()),
        Some(a) if a.starts_with("--") => Err(format!("unknown option {a}")),
        Some(a) => Err(format!("unexpected argument {a:?}")),
    }
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    let Some((first, rest)) = args.split_first() else { return Err("no command given".into()) };
    let mut rest: Vec<String> = rest.to_vec();
    match first.as_str() {
        "--version" | "-V" | "version" => Ok(Command::Version),
        "--help" | "-h" | "help" => Ok(Command::Help),
        "tools" => {
            let json = take_flag(&mut rest, "--json");
            no_extra(&rest)?;
            Ok(Command::Tools { json })
        }
        "run" => {
            let root = take_option(&mut rest, "--root")?.map(PathBuf::from);
            if let Some(file) = take_option(&mut rest, "--script")? {
                let keep_going = take_flag(&mut rest, "--keep-going");
                no_extra(&rest)?;
                return Ok(Command::Script { file: PathBuf::from(file), root, keep_going });
            }
            if take_flag(&mut rest, "--keep-going") {
                return Err("--keep-going only applies to --script".into());
            }
            let mut rest = rest.into_iter();
            let tool =
                rest.next().filter(|t| !t.starts_with("--")).ok_or("run needs a tool name (xmlcraft-cli tools lists them) or --script FILE")?;
            let pairs = rest.map(|a| key_value(&a)).collect::<Result<Vec<_>, _>>()?;
            Ok(Command::Run { tool, args: object(pairs), root })
        }
        #[cfg(feature = "mcp")]
        "mcp" => {
            let root = take_option(&mut rest, "--root")?.map(PathBuf::from);
            no_extra(&rest)?;
            Ok(Command::Mcp { root })
        }
        "ui" => {
            let control = take_option(&mut rest, "--control")?.ok_or("ui needs --control FILE (the file the app was started with)")?;
            let out = take_option(&mut rest, "--out")?.map(PathBuf::from);
            let mut rest = rest.into_iter();
            let method = rest.next().filter(|m| !m.starts_with("--")).ok_or("ui needs a method such as state, inspect, click or screenshot")?;
            let method = if method.starts_with("ui.") { method } else { format!("ui.{method}") };
            let pairs = rest.map(|a| key_value(&a)).collect::<Result<Vec<_>, _>>()?;
            Ok(Command::Ui { control: PathBuf::from(control), method, params: object(pairs), out })
        }
        other => Err(format!("unknown command {other:?}")),
    }
}

fn say(out: &mut impl Write, text: &str) {
    // A closed pipe (`xmlcraft-cli tools | head`) must end the run quietly, not panic.
    let _ = writeln!(out, "{text}");
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

/// Run a command; returns the exit code.
pub fn execute(command: Command, out: &mut impl Write, err: &mut impl Write) -> u8 {
    match command {
        Command::Version => {
            say(out, &format!("xmlcraft-cli {}", env!("CARGO_PKG_VERSION")));
            0
        }
        Command::Help => {
            say(out, USAGE);
            0
        }
        Command::Tools { json } => {
            if json {
                let list: Vec<Value> = xmlcraft_automation::tools()
                    .iter()
                    .map(|t| json!({ "name": t.name, "title": t.title, "description": t.description, "input_schema": t.input_schema, "read_only": t.read_only, "destructive": t.destructive }))
                    .collect();
                say(out, &pretty(&Value::Array(list)));
            } else {
                for t in xmlcraft_automation::tools() {
                    say(out, &format!("{:<16} {}", t.name, t.title));
                }
            }
            0
        }
        Command::Run { tool, args, root } => {
            let mut automation = match automation(root) {
                Ok(a) => a,
                Err(e) => return fail(err, &e),
            };
            match automation.call(&tool, &args) {
                Ok(content) => {
                    for Content::Json(v) in content {
                        say(out, &pretty(&v));
                    }
                    0
                }
                Err(e) => fail(err, &e.to_string()),
            }
        }
        Command::Script { file, root, keep_going } => {
            let text = match std::fs::read_to_string(&file) {
                Ok(t) => t,
                Err(e) => return fail(err, &format!("{}: {e}", file.display())),
            };
            let script: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(e) => return fail(err, &format!("{} is not JSON: {e}", file.display())),
            };
            let mut automation = match automation(root) {
                Ok(a) => a,
                Err(e) => return fail(err, &e),
            };
            match xmlcraft_automation::run_script(&mut automation, &script, keep_going) {
                Ok(results) => {
                    say(out, &pretty(&Value::Array(results.iter().map(xmlcraft_automation::StepResult::to_json).collect())));
                    u8::from(results.iter().any(|r| r.outcome.is_err()))
                }
                Err(e) => fail(err, &e),
            }
        }
        #[cfg(feature = "mcp")]
        Command::Mcp { root } => {
            let automation = match automation(root) {
                Ok(a) => a,
                Err(e) => return fail(err, &e),
            };
            let stdin = std::io::stdin();
            let mut server = xmlcraft_automation::mcp::McpServer::new(automation);
            match server.serve(stdin.lock(), std::io::stdout().lock()) {
                Ok(()) => 0,
                Err(e) => fail(err, &format!("the MCP server stopped: {e}")),
            }
        }
        Command::Ui { control, method, params, out: png } => {
            let mut client = match ControlClient::connect(&control) {
                Ok(c) => c,
                Err(e) => return fail(err, &e),
            };
            match client.call(&method, params) {
                Ok(mut reply) => {
                    // A screenshot is a picture: write the PNG where asked rather than print a wall of base64.
                    if let Some(b64) = reply.get("png_base64").and_then(Value::as_str).map(str::to_owned) {
                        let Some(path) = png else { return fail(err, "this reply is a picture: pass --out FILE.png to save it") };
                        use base64::Engine as _;
                        let bytes = match base64::engine::general_purpose::STANDARD.decode(b64) {
                            Ok(b) => b,
                            Err(e) => return fail(err, &format!("the picture is not valid base64: {e}")),
                        };
                        if let Err(e) = std::fs::write(&path, bytes) {
                            return fail(err, &format!("{}: {e}", path.display()));
                        }
                        if let Value::Object(map) = &mut reply {
                            map.remove("png_base64");
                            map.insert("saved".into(), json!(path.display().to_string()));
                        }
                    }
                    say(out, &pretty(&reply));
                    0
                }
                Err(e) => fail(err, &e),
            }
        }
    }
}

fn automation(root: Option<PathBuf>) -> Result<Automation, String> {
    match root {
        Some(root) => Automation::with_root(&root).map_err(|e| e.to_string()),
        None => Ok(Automation::new()),
    }
}

fn fail(err: &mut impl Write, message: &str) -> u8 {
    let _ = writeln!(err, "xmlcraft-cli: {message}");
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    fn run(command: Command) -> (u8, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = execute(command, &mut out, &mut err);
        (code, String::from_utf8(out).expect("utf-8"), String::from_utf8(err).expect("utf-8"))
    }

    #[test]
    fn simple_commands_parse() {
        assert_eq!(parse(&args("--version")), Ok(Command::Version));
        assert_eq!(parse(&args("-h")), Ok(Command::Help));
        assert_eq!(parse(&args("tools")), Ok(Command::Tools { json: false }));
        assert_eq!(parse(&args("tools --json")), Ok(Command::Tools { json: true }));
    }

    #[test]
    fn run_reads_json_values_and_falls_back_to_text() {
        let c = parse(&args("run command_list menu=View n=3 flag=true list=[1,2] text=a=b --root /tmp")).expect("parses");
        assert_eq!(
            c,
            Command::Run {
                tool: "command_list".into(),
                args: json!({"menu": "View", "n": 3, "flag": true, "list": [1, 2], "text": "a=b"}),
                root: Some(PathBuf::from("/tmp"))
            }
        );
    }

    #[test]
    fn script_options_parse() {
        assert_eq!(
            parse(&args("run --script s.json --keep-going --root r")),
            Ok(Command::Script { file: "s.json".into(), root: Some("r".into()), keep_going: true })
        );
    }

    #[test]
    fn ui_adds_the_ui_prefix_and_keeps_it_when_given() {
        let want = |m: &str| Command::Ui { control: "c.json".into(), method: m.into(), params: json!({"label": "Menu"}), out: None };
        assert_eq!(parse(&args("ui --control c.json click label=Menu")), Ok(want("ui.click")));
        assert_eq!(parse(&args("ui --control c.json ui.click label=Menu")), Ok(want("ui.click")));
    }

    #[test]
    fn mistakes_are_explained() {
        for (line, needle) in [
            ("", "no command given"),
            ("frobnicate", "unknown command"),
            ("tools extra", "unexpected argument"),
            ("tools --what", "unknown option --what"),
            ("run", "run needs a tool name"),
            ("run --root", "--root needs a value"),
            ("run --root a --root b app_info", "given twice"),
            ("run app_info novalue", "expected key=value"),
            ("run app_info =x", "the key is empty"),
            ("run app_info --keep-going", "--keep-going only applies"),
            ("run --script", "--script needs a value"),
            ("ui click", "ui needs --control"),
            ("ui --control c.json", "ui needs a method"),
            ("mcp --root", "--root needs a value"),
            ("mcp extra", "unexpected argument"),
        ] {
            let err = parse(&args(line)).expect_err(line);
            assert!(err.contains(needle), "{line:?}: {err}");
        }
    }

    #[test]
    fn version_and_help_print_and_succeed() {
        let (code, out, _) = run(Command::Version);
        assert_eq!((code, out.trim()), (0, format!("xmlcraft-cli {}", env!("CARGO_PKG_VERSION")).as_str()));
        let (code, out, _) = run(Command::Help);
        assert_eq!(code, 0);
        assert!(out.contains("xmlcraft-cli run <tool>"));
    }

    #[test]
    fn tools_lists_names_and_json_schemas() {
        let (code, out, _) = run(Command::Tools { json: false });
        assert_eq!(code, 0);
        assert!(out.lines().any(|l| l.starts_with("app_info")));
        let (_, out, _) = run(Command::Tools { json: true });
        let list: Value = serde_json::from_str(&out).expect("JSON");
        assert!(list.as_array().is_some_and(|l| l.iter().all(|t| t["input_schema"]["type"] == "object")));
    }

    #[test]
    fn run_prints_the_tools_json_and_reports_failures_on_stderr() {
        let (code, out, _) = run(Command::Run { tool: "app_info".into(), args: json!({}), root: None });
        assert_eq!(code, 0);
        assert_eq!(serde_json::from_str::<Value>(&out).expect("JSON")["name"], "XmlCraft");
        let (code, out, err) = run(Command::Run { tool: "no_such_tool".into(), args: json!({}), root: None });
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(err.contains("unknown tool"), "{err}");
        let (code, _, err) = run(Command::Run { tool: "command_list".into(), args: json!({"menu": "Nope"}), root: None });
        assert_eq!(code, 1);
        assert!(err.contains("expected one of"), "{err}");
        let (code, _, err) = run(Command::Run { tool: "app_info".into(), args: json!({}), root: Some("/no/such/folder".into()) });
        assert_eq!(code, 1);
        assert!(err.contains("cannot be used"), "{err}");
    }

    #[test]
    fn a_script_runs_and_its_exit_code_says_whether_every_step_worked() {
        let dir = tempfile::tempdir().expect("temp dir");
        let good = dir.path().join("good.json");
        std::fs::write(&good, r#"[{"tool": "app_info"}, {"tool": "command_list", "args": {"menu": "File"}}]"#).expect("write");
        let (code, out, _) = run(Command::Script { file: good, root: None, keep_going: false });
        assert_eq!(code, 0);
        let results: Value = serde_json::from_str(&out).expect("JSON");
        assert_eq!(results.as_array().map(Vec::len), Some(2));
        assert!(results.as_array().is_some_and(|r| r.iter().all(|s| s["ok"] == true)));
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, r#"[{"tool": "app_info"}, {"tool": "nope"}, {"tool": "app_info"}]"#).expect("write");
        let (code, out, _) = run(Command::Script { file: bad.clone(), root: None, keep_going: false });
        assert_eq!(code, 1);
        assert_eq!(serde_json::from_str::<Value>(&out).expect("JSON").as_array().map(Vec::len), Some(2), "stopped at the failure");
        let (_, out, _) = run(Command::Script { file: bad, root: None, keep_going: true });
        assert_eq!(serde_json::from_str::<Value>(&out).expect("JSON").as_array().map(Vec::len), Some(3));
    }

    #[test]
    fn a_bad_script_file_is_explained() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (code, _, err) = run(Command::Script { file: dir.path().join("missing.json"), root: None, keep_going: false });
        assert_eq!(code, 1);
        assert!(err.contains("missing.json"), "{err}");
        let junk = dir.path().join("junk.json");
        std::fs::write(&junk, "{not json").expect("write");
        let (_, _, err) = run(Command::Script { file: junk, root: None, keep_going: false });
        assert!(err.contains("is not JSON"), "{err}");
        let shape = dir.path().join("shape.json");
        std::fs::write(&shape, "42").expect("write");
        let (_, _, err) = run(Command::Script { file: shape, root: None, keep_going: false });
        assert!(err.contains("array of steps"), "{err}");
    }

    #[test]
    fn ui_talks_to_a_control_server_and_saves_pictures() {
        use std::sync::mpsc::channel;
        use xmlcraft_automation::control::{Reply, serve, write_endpoint_file};
        let endpoint = serve(|method: &str, params: Value| {
            let (tx, rx) = channel::<Reply>();
            let _ = tx.send(match method {
                "ui.state" => Ok(json!({ "echo": params })),
                "ui.screenshot" => Ok(json!({ "png_base64": "iVBORw0KGgo=", "width": 1, "height": 1 })),
                other => Err(format!("unknown method {other}")),
            });
            rx
        })
        .expect("serves");
        let dir = tempfile::tempdir().expect("temp dir");
        let control = dir.path().join("control.json");
        write_endpoint_file(&control, &endpoint).expect("control file");
        let ui = |method: &str, params: Value, out: Option<PathBuf>| Command::Ui { control: control.clone(), method: method.into(), params, out };

        let (code, out, _) = run(ui("ui.state", json!({"a": 1}), None));
        assert_eq!(code, 0);
        assert_eq!(serde_json::from_str::<Value>(&out).expect("JSON")["echo"], json!({"a": 1}));

        let (code, _, err) = run(ui("ui.screenshot", json!({}), None));
        assert_eq!(code, 1);
        assert!(err.contains("--out"), "{err}");
        let png = dir.path().join("shot.png");
        let (code, out, _) = run(ui("ui.screenshot", json!({}), Some(png.clone())));
        assert_eq!(code, 0);
        assert_eq!(std::fs::read(&png).expect("saved"), [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        let reply: Value = serde_json::from_str(&out).expect("JSON");
        assert!(reply.get("png_base64").is_none() && reply["saved"].is_string(), "the base64 is not printed: {reply}");

        let (code, _, err) = run(ui("ui.nope", json!({}), None));
        assert_eq!(code, 1);
        assert!(err.contains("unknown method"), "{err}");
        let (code, _, err) = run(Command::Ui { control: dir.path().join("missing.json"), method: "ui.state".into(), params: json!({}), out: None });
        assert_eq!(code, 1);
        assert!(err.contains("missing.json"), "{err}");
    }
}
