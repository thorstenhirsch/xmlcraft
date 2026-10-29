//! What `xmlcraft-cli` does: parse the command line, then run it.
//!
//! - `check FILE… [--root DIR] [--json]`: check the files for well-formedness; the problems are printed as `file:line:column: severity: message`.
//! - `validate FILE… [--schema FILE] [--root DIR] [--json]`: check the files for well-formedness and, when they are well-formed, validity against
//!   their DTD, or against the XML Schema they name (`xsi:schemaLocation`) or `--schema` gives; the problems are printed as
//!   `file:line:column: severity: message` (a problem in an external text of the DTD, or in a schema document, names that file).
//! - `format FILE [--minify] [--indent N | --tabs] [--blank-lines keep|remove] [--check | --in-place] [--root DIR]`: pretty-print (or minify) a
//!   file; the result goes to standard output unless `--in-place`.
//! - `check-one FILE [--edit]`: hold the engine to its promises on one file (what the fuzzer runs): exit 1 and the broken promise on standard
//!   error when one breaks; a file larger than the engine opens is an error too.
//! - `tools [--json]`: the tools and their input schemas.
//! - `run <tool> [key=value …] [--root DIR]`: call one tool. A value is read as JSON when it is (`3`, `true`, `["a"]`), else as text.
//! - `run --script FILE [--root DIR] [--keep-going]`: run a JSON list of steps.
//! - `mcp [--root DIR]`: serve the tools over MCP on standard input and output (opt-in; nothing starts it on its own).
//! - `ui --control FILE <method> [key=value …] [--out FILE.png]`: drive a running app that was started with `--control FILE`.
//!
//! Results are JSON on standard output; messages for a person go to standard error. The exit code is 0 for success, 1 when a tool or
//! request failed (`check`: a problem was found; `format --check`: the file would change), and 2 for a command line that could not be understood.

use std::io::Write;
use std::path::PathBuf;

use serde_json::{Map, Value, json};
use xmlcraft_automation::control::ControlClient;
use xmlcraft_automation::{Automation, Content};

pub const USAGE: &str = "Usage:
  xmlcraft-cli check FILE... [--root DIR] [--json]
  xmlcraft-cli validate FILE... [--schema FILE] [--root DIR] [--json]
  xmlcraft-cli format FILE [--minify] [--indent N | --tabs] [--blank-lines keep|remove] [--check | --in-place] [--root DIR]
  xmlcraft-cli tools [--json]
  xmlcraft-cli check-one FILE [--edit]
  xmlcraft-cli run <tool> [key=value ...] [--root DIR]
  xmlcraft-cli run --script FILE [--root DIR] [--keep-going]
  xmlcraft-cli mcp [--root DIR]
  xmlcraft-cli ui --control FILE <method> [key=value ...] [--out FILE.png]
  xmlcraft-cli --version | --help

check-one holds the engine to its promises on one file (the fuzzer runs it); --edit also edits it every way and undoes it all.
A key=value value is read as JSON when it is valid JSON (3, true, [\"a\"]), and as text otherwise.
--root DIR confines every path a tool reads or writes to DIR.";

#[derive(Debug, PartialEq)]
pub enum Command {
    Version,
    Help,
    Check {
        files: Vec<String>,
        root: Option<PathBuf>,
        json: bool,
    },
    Validate {
        files: Vec<String>,
        /// An XML Schema to validate against, instead of the DTD or the schema the documents name.
        schema: Option<String>,
        root: Option<PathBuf>,
        json: bool,
    },
    Format {
        file: String,
        /// Arguments of the `xml_format` tool beyond the file.
        options: Value,
        root: Option<PathBuf>,
        mode: FormatMode,
    },
    Tools {
        json: bool,
    },
    CheckOne {
        file: PathBuf,
        edit: bool,
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

/// What `format` does with the result.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum FormatMode {
    /// Print it.
    Print,
    /// Say whether the file would change (exit code 1 if it would), and print nothing else.
    Check,
    /// Write it back to the file.
    InPlace,
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
        "check-one" => {
            let edit = take_flag(&mut rest, "--edit");
            if let Some(flag) = rest.iter().find(|a| a.starts_with("--")) {
                return Err(format!("unknown option {flag}"));
            }
            let [file] = <[String; 1]>::try_from(rest).map_err(|rest| format!("check-one takes one file, not {}", rest.len()))?;
            Ok(Command::CheckOne { file: PathBuf::from(file), edit })
        }
        command @ ("check" | "validate") => {
            let root = take_option(&mut rest, "--root")?.map(PathBuf::from);
            // Only `validate` has a schema to give; for `check` the option stays in `rest` and is refused below.
            let schema = if command == "validate" { take_option(&mut rest, "--schema")? } else { None };
            let json = take_flag(&mut rest, "--json");
            if let Some(flag) = rest.iter().find(|a| a.starts_with("--")) {
                return Err(format!("unknown option {flag}"));
            }
            if rest.is_empty() {
                return Err(format!("{command} needs at least one file"));
            }
            Ok(if command == "check" { Command::Check { files: rest, root, json } } else { Command::Validate { files: rest, schema, root, json } })
        }
        "format" => {
            let root = take_option(&mut rest, "--root")?.map(PathBuf::from);
            let mut options = Map::new();
            if take_flag(&mut rest, "--minify") {
                options.insert("minify".into(), json!(true));
            }
            if take_flag(&mut rest, "--tabs") {
                options.insert("tabs".into(), json!(true));
            }
            if let Some(n) = take_option(&mut rest, "--indent")? {
                let n: u64 = n.parse().map_err(|_| format!("--indent needs a number of spaces, not {n:?}"))?;
                if options.contains_key("tabs") {
                    return Err("--tabs and --indent cannot be combined".into());
                }
                options.insert("indent".into(), json!(n));
            }
            if let Some(b) = take_option(&mut rest, "--blank-lines")? {
                if b != "keep" && b != "remove" {
                    return Err(format!("--blank-lines is keep or remove, not {b:?}"));
                }
                options.insert("blank_lines".into(), json!(b));
            }
            let (check, in_place) = (take_flag(&mut rest, "--check"), take_flag(&mut rest, "--in-place"));
            if check && in_place {
                return Err("--check and --in-place cannot be combined".into());
            }
            if let Some(flag) = rest.iter().find(|a| a.starts_with("--")) {
                return Err(format!("unknown option {flag}"));
            }
            let [file] = <[String; 1]>::try_from(rest).map_err(|rest| format!("format takes one file, not {}", rest.len()))?;
            let mode = if check {
                FormatMode::Check
            } else if in_place {
                FormatMode::InPlace
            } else {
                FormatMode::Print
            };
            Ok(Command::Format { file, options: Value::Object(options), root, mode })
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
        Command::CheckOne { file, edit } => {
            // The size first: the file is not read into memory only to be refused.
            let max = xmlcraft_engine::MAX_OPEN_BYTES;
            match std::fs::metadata(&file) {
                Err(e) => return fail(err, &format!("{}: {e}", file.display())),
                Ok(meta) if meta.len() > max as u64 => {
                    return fail(err, &format!("{}: {} bytes are more than the {max} bytes XmlCraft opens", file.display(), meta.len()));
                }
                Ok(_) => {}
            }
            match std::fs::read(&file) {
                Err(e) => fail(err, &format!("{}: {e}", file.display())),
                Ok(bytes) => match xmlcraft_engine::selfcheck::check(&bytes, edit) {
                    Ok(()) => 0,
                    Err(why) => fail(err, &format!("self-check failed: {why}")),
                },
            }
        }
        Command::Check { files, root, json } => check("xml_wellformed", &files, None, root, json, out, err),
        Command::Validate { files, schema, root, json } => check("xml_validate", &files, schema.as_deref(), root, json, out, err),
        Command::Format { file, options, root, mode } => format_file(&file, options, root, mode, out, err),
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

fn value_of(content: Vec<Content>) -> Value {
    match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    }
}

/// A schema given on the command line, as the tool wants it: the tool takes a relative schema from the folder of the document, but a
/// path typed after `--schema` is read like the files are, from the root when there is one and otherwise from the current folder.
fn schema_path(schema: &str, root: Option<&PathBuf>) -> Result<String, String> {
    let given = std::path::Path::new(schema);
    let joined = match root {
        Some(root) if given.is_relative() => root.join(given),
        _ => given.to_path_buf(),
    };
    let absolute = std::path::absolute(&joined).map_err(|e| format!("--schema {schema}: {e}"))?;
    absolute.to_str().map(str::to_owned).ok_or_else(|| format!("--schema {schema}: the path is not valid UTF-8"))
}

/// `check` and `validate`: every file through `xml_wellformed` or `xml_validate`. Exit 1 if any file has an error or cannot be read, or, for
/// `validate`, has no schema or DTD to be valid against, or a schema with errors.
fn check(tool: &str, files: &[String], schema: Option<&str>, root: Option<PathBuf>, as_json: bool, out: &mut impl Write, err: &mut impl Write) -> u8 {
    let schema = match schema.map(|s| schema_path(s, root.as_ref())).transpose() {
        Ok(schema) => schema,
        Err(e) => return fail(err, &e),
    };
    let mut automation = match automation(root) {
        Ok(a) => a,
        Err(e) => return fail(err, &e),
    };
    let (mut code, mut results) = (0u8, Vec::new());
    for file in files {
        let mut arguments = json!({ "path": file });
        if let (Some(schema), Some(map)) = (&schema, arguments.as_object_mut()) {
            map.insert("schema".into(), json!(schema));
        }
        match automation.call(tool, &arguments) {
            Ok(content) => {
                let result = value_of(content);
                // A schema that has errors makes the verdict doubtful, and a document with no schema was not validated at all.
                let doubtful = result["schema_errors"].as_u64().unwrap_or(0) > 0 || result["note"].is_string();
                if result["errors"].as_u64().unwrap_or(0) > 0 || doubtful {
                    code = 1;
                }
                if let (false, Some(note)) = (as_json, result["note"].as_str()) {
                    let _ = writeln!(err, "xmlcraft-cli: {file}: {note}");
                }
                // Bytes that are not valid in the file's encoding were read as U+FFFD: what was checked is not quite the file.
                if result["lossy"] == true {
                    code = 1;
                    if !as_json {
                        let _ =
                            writeln!(err, "xmlcraft-cli: {file}: has bytes that are not valid in its encoding (checked with each shown as U+FFFD)");
                    }
                }
                if !as_json {
                    for p in result["problems"].as_array().into_iter().flatten() {
                        // A problem in an external text of the DTD is in that file.
                        let place = p["file"].as_str().unwrap_or(file);
                        say(
                            out,
                            &format!(
                                "{place}:{}:{}: {}: {}",
                                p["line"].as_u64().unwrap_or(0),
                                p["column"].as_u64().unwrap_or(0),
                                p["severity"].as_str().unwrap_or("error"),
                                p["message"].as_str().unwrap_or_default()
                            ),
                        );
                    }
                    if result["well_formed"] == true && result["warnings"].as_u64() == Some(0) && result["errors"].as_u64() == Some(0) && !doubtful {
                        say(out, &format!("{file}: {}", if tool == "xml_validate" { "valid" } else { "well-formed" }));
                    }
                }
                results.push(json!({ "file": file, "result": result }));
            }
            Err(e) => {
                code = 1;
                if as_json {
                    results.push(json!({ "file": file, "error": e.to_string() }));
                } else {
                    let _ = writeln!(err, "xmlcraft-cli: {file}: {e}");
                }
            }
        }
    }
    if as_json {
        say(out, &pretty(&Value::Array(results)));
    }
    code
}

/// `format`: the file through `xml_format`; the result is printed, checked against the file, or written back (atomically, through the
/// document tools, so a file that changed meanwhile is not replaced).
fn format_file(file: &str, options: Value, root: Option<PathBuf>, mode: FormatMode, out: &mut impl Write, err: &mut impl Write) -> u8 {
    let mut automation = match automation(root) {
        Ok(a) => a,
        Err(e) => return fail(err, &e),
    };
    let mut args = options;
    if let Value::Object(map) = &mut args {
        map.insert("path".into(), json!(file));
    }
    if mode == FormatMode::InPlace {
        // Through a document, so that the file keeps its encoding, byte-order mark and line endings.
        let id = match automation.call("doc_open", &json!({ "path": file })) {
            Ok(content) => value_of(content)["id"].as_u64().unwrap_or(0),
            Err(e) => return fail(err, &e.to_string()),
        };
        if let Value::Object(map) = &mut args {
            map.remove("path");
            map.insert("id".into(), json!(id));
        }
        let changed = match automation.call("xml_format", &args) {
            Ok(content) => value_of(content)["changed"] == true,
            Err(e) => return fail(err, &e.to_string()),
        };
        if changed && let Err(e) = automation.call("doc_save", &json!({ "id": id })) {
            return fail(err, &e.to_string());
        }
        return 0;
    }
    let result = match automation.call("xml_format", &args) {
        Ok(content) => value_of(content),
        Err(e) => return fail(err, &e.to_string()),
    };
    match mode {
        FormatMode::Check => {
            if result["changed"] == true {
                let _ = writeln!(err, "xmlcraft-cli: {file} would be reformatted");
                1
            } else {
                0
            }
        }
        _ => {
            // Not text that would be written over the file's bytes with U+FFFD in them.
            if result["lossy"] == true {
                return fail(err, &format!("{file} has bytes that are not valid in its encoding; nothing was printed"));
            }
            // Print exactly the formatted text (no extra line break), as a person redirecting it to a file expects.
            let _ = write!(out, "{}", result["text"].as_str().unwrap_or_default());
            0
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
    fn check_one_reads_a_file_and_says_nothing_when_all_is_well() {
        assert_eq!(parse(&args("check-one a.xml --edit")), Ok(Command::CheckOne { file: "a.xml".into(), edit: true }));
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("a.xml");
        std::fs::write(&path, "<a><b>x</b></a>").expect("writes");
        assert_eq!(run(Command::CheckOne { file: path.clone(), edit: true }), (0, String::new(), String::new()));
        let (code, _, err) = run(Command::CheckOne { file: dir.path().join("missing.xml"), edit: false });
        assert_eq!(code, 1);
        assert!(err.contains("missing.xml"), "{err}");
        // A file larger than the engine opens is refused by its size, without being read (a sparse file costs nothing here).
        let big = dir.path().join("big.xml");
        let file = std::fs::File::create(&big).expect("creates");
        file.set_len(xmlcraft_engine::MAX_OPEN_BYTES as u64 + 1).expect("sets the length");
        let (code, _, err) = run(Command::CheckOne { file: big, edit: false });
        assert_eq!(code, 1);
        assert!(err.contains("more than the"), "{err}");
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
            ("check-one", "check-one takes one file, not 0"),
            ("check-one a.xml b.xml", "check-one takes one file, not 2"),
            ("check-one --what a.xml", "unknown option --what"),
            ("check", "needs at least one file"),
            ("check --what a.xml", "unknown option --what"),
            ("format", "format takes one file, not 0"),
            ("format a.xml b.xml", "format takes one file, not 2"),
            ("format a.xml --indent x", "--indent needs a number"),
            ("format a.xml --blank-lines some", "keep or remove"),
            ("format a.xml --check --in-place", "cannot be combined"),
            ("format a.xml --tabs --indent 4", "--tabs and --indent cannot be combined"),
            ("format a.xml --nope", "unknown option --nope"),
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

    #[test]
    fn check_and_format_options_parse() {
        assert_eq!(
            parse(&args("check a.xml b.xml --json --root r")),
            Ok(Command::Check { files: vec!["a.xml".into(), "b.xml".into()], root: Some("r".into()), json: true })
        );
        assert_eq!(
            parse(&args("format a.xml --minify --indent 4 --blank-lines remove --check")),
            Ok(Command::Format {
                file: "a.xml".into(),
                options: json!({"minify": true, "indent": 4, "blank_lines": "remove"}),
                root: None,
                mode: FormatMode::Check
            })
        );
        assert_eq!(
            parse(&args("format a.xml --tabs --in-place")),
            Ok(Command::Format { file: "a.xml".into(), options: json!({"tabs": true}), root: None, mode: FormatMode::InPlace })
        );
    }

    #[test]
    fn check_prints_problems_as_file_line_column_and_sets_the_exit_code() {
        let dir = tempfile::tempdir().expect("temp dir");
        let good = dir.path().join("good.xml");
        let bad = dir.path().join("bad.xml");
        std::fs::write(&good, "<a/>").expect("writes");
        std::fs::write(&bad, "<a>\n  <b></c>\n</a>").expect("writes");
        let (good, bad) = (good.to_str().expect("utf-8").to_owned(), bad.to_str().expect("utf-8").to_owned());
        let (code, out, _) = run(Command::Check { files: vec![good.clone()], root: None, json: false });
        assert_eq!((code, out.trim()), (0, format!("{good}: well-formed").as_str()));
        let (code, out, _) = run(Command::Check { files: vec![good.clone(), bad.clone()], root: None, json: false });
        assert_eq!(code, 1);
        assert!(out.contains(&format!("{bad}:2:6: error: The end tag </c> does not match the start tag <b>.")), "{out}");
        let (code, out, _) = run(Command::Check { files: vec![bad.clone()], root: None, json: true });
        assert_eq!(code, 1);
        assert_eq!(serde_json::from_str::<Value>(&out).expect("JSON")[0]["result"]["errors"], 1);
        let (code, _, err) = run(Command::Check { files: vec![format!("{good}.missing")], root: None, json: false });
        assert_eq!(code, 1);
        assert!(err.contains("missing"), "{err}");
    }

    #[test]
    fn validate_checks_against_the_dtd_and_names_the_file_a_problem_is_in() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = |name: &str, text: &str| {
            let p = dir.path().join(name);
            std::fs::write(&p, text).expect("writes");
            p.to_str().expect("utf-8").to_owned()
        };
        path("r.dtd", "<!ELEMENT r (a)>\n<!ELEMENT a EMPTY>\n<!ATTLIST a x ID #FIXED 'v'>");
        let good = path("good.xml", "<!DOCTYPE r [<!ELEMENT r EMPTY>]><r/>");
        let bad = path("bad.xml", "<!DOCTYPE r [<!ELEMENT r EMPTY>]>\n<r>text</r>");
        let external = path("external.xml", "<!DOCTYPE r SYSTEM \"r.dtd\">\n<r><a/></r>");
        let (code, out, _) = run(Command::Validate { files: vec![good.clone()], schema: None, root: None, json: false });
        assert_eq!((code, out.trim()), (0, format!("{good}: valid").as_str()));
        let (code, out, _) = run(Command::Validate { files: vec![bad.clone()], schema: None, root: None, json: false });
        assert_eq!(code, 1);
        assert!(out.contains(&format!("{bad}:2:4: error: The element 'r' is declared EMPTY, but it has content.")), "{out}");
        // A well-formedness check does not look at validity.
        assert_eq!(run(Command::Check { files: vec![bad.clone()], root: None, json: false }).0, 0);
        // A problem in the DTD's own file is named by that file and its line.
        let (code, out, _) = run(Command::Validate { files: vec![external], schema: None, root: None, json: false });
        assert_eq!(code, 1);
        assert!(out.contains("r.dtd:3:") && out.contains("must be #IMPLIED or #REQUIRED"), "{out}");
        let (code, out, _) = run(Command::Validate { files: vec![bad], schema: None, root: None, json: true });
        assert_eq!((code, serde_json::from_str::<Value>(&out).expect("JSON")[0]["result"]["valid"].clone()), (1, json!(false)));
        assert_eq!(
            parse(&args("validate a.xml --json")),
            Ok(Command::Validate { files: vec!["a.xml".into()], schema: None, root: None, json: true })
        );
        assert!(parse(&args("validate")).expect_err("no file").contains("validate needs at least one file"));
    }

    #[test]
    fn validate_checks_against_an_xml_schema_given_or_named() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = |name: &str, text: &str| {
            let p = dir.path().join(name);
            std::fs::write(&p, text).expect("writes");
            p.to_str().expect("utf-8").to_owned()
        };
        let xsd = path(
            "r.xsd",
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="r"><xs:complexType><xs:sequence><xs:element name="a" type="xs:int"/></xs:sequence></xs:complexType></xs:element></xs:schema>"#,
        );
        let good = path("good.xml", "<r><a>1</a></r>");
        let bad = path("bad.xml", "<r>\n  <a>one</a>\n</r>");
        let named =
            path("named.xml", r#"<r xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:noNamespaceSchemaLocation="r.xsd"><a>x</a></r>"#);
        let validate = |files: Vec<String>, schema: Option<String>, root: Option<PathBuf>, json| run(Command::Validate { files, schema, root, json });
        let (code, out, _) = validate(vec![good.clone()], Some(xsd.clone()), None, false);
        assert_eq!((code, out.trim()), (0, format!("{good}: valid").as_str()));
        let (code, out, _) = validate(vec![bad.clone()], Some(xsd.clone()), None, false);
        assert_eq!(code, 1);
        assert!(out.contains(&format!("{bad}:2:4: error: The value of the element 'a' is not valid")) && !out.contains(": valid"), "{out}");
        // The schema a document names is followed when none is given.
        let (code, out, _) = validate(vec![named.clone()], None, None, false);
        assert_eq!(code, 1);
        assert!(out.contains(&format!("{named}:")), "{out}");
        // A relative schema is read from the root, as the files are.
        let (code, out, _) = validate(vec!["good.xml".into()], Some("r.xsd".into()), Some(dir.path().to_path_buf()), false);
        assert_eq!((code, out.trim()), (0, "good.xml: valid"));
        // A schema with an error in a part the document does not use still has an error: the problem names the schema's file, and the
        // exit code says so, though the document itself was found valid.
        let flawed = path(
            "flawed.xsd",
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="r" type="xs:string"/><xs:element name="unused" type="nope"/></xs:schema>"#,
        );
        let plain = path("plain.xml", "<r>x</r>");
        let (code, out, _) = validate(vec![plain.clone()], Some(flawed.clone()), None, false);
        assert_eq!(code, 1);
        assert!(out.contains(&format!("{flawed}:1:127: error: The type 'nope' is not defined.")) && !out.contains(": valid"), "{out}");
        // A schema that cannot be read leaves the document unchecked: that is not "valid".
        let (code, out, err) = validate(vec![good.clone()], Some(format!("{xsd}.missing")), None, false);
        assert_eq!(code, 1);
        assert!(!out.contains("valid") && err.contains(&good), "{out} / {err}");
        // A schema that is not inside the root is refused.
        let outside = tempfile::tempdir().expect("temp dir");
        let elsewhere = outside.path().join("r.xsd");
        std::fs::copy(&xsd, &elsewhere).expect("copies");
        let (code, out, err) =
            validate(vec!["good.xml".into()], Some(elsewhere.to_str().expect("utf-8").to_owned()), Some(dir.path().to_path_buf()), false);
        assert_eq!(code, 1);
        assert!(!out.contains("valid") && !err.is_empty(), "{out} / {err}");
        // JSON carries the verdict too.
        let (code, out, _) = validate(vec![good], Some(xsd), None, true);
        let result = serde_json::from_str::<Value>(&out).expect("JSON");
        assert_eq!((code, result[0]["result"]["kind"].clone(), result[0]["result"]["valid"].clone()), (0, json!("xsd"), json!(true)));
        // Only `validate` takes a schema.
        assert_eq!(
            parse(&args("validate a.xml --schema s.xsd --root r")),
            Ok(Command::Validate { files: vec!["a.xml".into()], schema: Some("s.xsd".into()), root: Some("r".into()), json: false })
        );
        assert!(parse(&args("check a.xml --schema s.xsd")).expect_err("check has no schema").contains("unknown option --schema"));
        assert!(parse(&args("validate a.xml --schema")).expect_err("needs a value").contains("--schema needs a value"));
    }

    #[test]
    fn format_prints_checks_and_rewrites_in_place_keeping_the_files_style() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("a.xml");
        let file = path.to_str().expect("utf-8").to_owned();
        std::fs::write(&path, "\u{FEFF}<a><b/></a>\r\n").expect("writes");
        let format = |options: Value, mode| Command::Format { file: file.clone(), options, root: None, mode };
        // Printing writes nothing; the text is exact.
        let (code, out, _) = run(format(json!({}), FormatMode::Print));
        assert_eq!((code, out.as_str()), (0, "<a>\r\n  <b/>\r\n</a>\r\n"));
        assert_eq!(std::fs::read(&path).expect("reads"), "\u{FEFF}<a><b/></a>\r\n".as_bytes());
        // --check says so by the exit code.
        let (code, out, err) = run(format(json!({}), FormatMode::Check));
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(err.contains("would be reformatted"), "{err}");
        // In place: the byte-order mark and the CRLFs stay.
        let (code, _, _) = run(format(json!({}), FormatMode::InPlace));
        assert_eq!(code, 0);
        assert_eq!(std::fs::read(&path).expect("reads"), "\u{FEFF}<a>\r\n  <b/>\r\n</a>\r\n".as_bytes());
        let (code, _, _) = run(format(json!({}), FormatMode::Check));
        assert_eq!(code, 0, "formatted already");
        // Minify, and a document that is not well-formed is refused without touching the file.
        let (_, out, _) = run(format(json!({"minify": true}), FormatMode::Print));
        assert_eq!(out, "<a><b/></a>");
        std::fs::write(&path, "<a><b></a>").expect("writes");
        for mode in [FormatMode::Print, FormatMode::Check, FormatMode::InPlace] {
            let (code, _, err) = run(format(json!({}), mode));
            assert_eq!(code, 1);
            assert!(err.contains("not well-formed"), "{err}");
        }
        assert_eq!(std::fs::read(&path).expect("reads"), b"<a><b></a>");
    }

    #[test]
    fn a_file_with_invalid_bytes_is_not_reported_well_formed_printed_or_rewritten() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("bad.xml");
        let original: &[u8] = b"<a>\n  <b/>\n</a><!-- \xFF -->";
        std::fs::write(&path, original).expect("writes");
        let file = path.to_str().expect("utf-8").to_owned();
        let (code, _, err) = run(Command::Check { files: vec![file.clone()], root: None, json: false });
        assert_eq!(code, 1, "the text is well-formed but it is not the file");
        assert!(err.contains("not valid in its encoding"), "{err}");
        let (code, out, err) = run(Command::Format { file: file.clone(), options: json!({"minify": true}), root: None, mode: FormatMode::Print });
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(err.contains("nothing was printed"), "{err}");
        // In place, it would have to write the replacement character over the bad byte: refused.
        let (code, _, err) = run(Command::Format { file, options: json!({"minify": true}), root: None, mode: FormatMode::InPlace });
        assert_eq!(code, 1);
        assert!(err.contains("not valid"), "{err}");
        assert_eq!(std::fs::read(&path).expect("reads"), original);
    }
}
