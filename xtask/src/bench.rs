//! `cargo xtask bench`: the performance targets of the editor (plan §3), measured on synthetic documents.
//!
//! Each scenario builds its text deterministically (no files, nothing from the network), times what a user waits for and compares it
//! with two numbers from `xtask/baselines/bench.toml`: the **target** (the plan's, where there is one) and the **budget**, which is
//! what the scenario is held to now. A budget above its target is a known shortfall (a gap in `docs/gaps.md`), lowered when it is
//! worked on and never raised. The command fails when a scenario is over its budget, and prints every number either way. It re-runs
//! itself with `--release` (the dev profile is not what users get), unless `--no-release`.
//!
//! ```text
//! cargo xtask bench [--only NAME] [--list] [--no-release] [--json]
//! ```

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use xmlcraft_editor::Editor;
use xmlcraft_engine::Session;
use xmlcraft_syntax::Highlighter;
use xmlcraft_text::Buffer;

use crate::gates::root;

/// What a scenario measured.
struct Measured {
    /// The number the budget is about, in milliseconds.
    millis: f64,
    /// What the number is (a median, a 95th percentile, a total).
    what: &'static str,
    /// Anything else worth printing.
    note: String,
}

type Scenario = fn() -> Result<Measured>;

/// Every scenario: its name, what it stands for and how to run it.
const SCENARIOS: &[(&str, &str, Scenario)] = &[
    ("open_100mb", "open a 100 MB file: bytes to a document", open_100mb),
    ("scroll_100mb_far", "show 60 lines after a jump to a far place in a 100 MB document (cold)", scroll_100mb_far),
    ("keystroke_1mb", "type a character in the middle of a 1 MB document and style the visible lines (p95)", keystroke_1mb),
    ("reparse_1mb", "analyse (parse, problems, outline, fold ranges) a 1 MB document", reparse_1mb),
    ("reparse_10mb", "analyse a 10 MB document", reparse_10mb),
    ("format_1mb", "pretty-print a 1 MB document", format_1mb),
    ("find_all_10mb", "find every match of a word in a 10 MB document", find_all_10mb),
];

/// A document of about `bytes` bytes: a root with rows of elements, attributes and text, some comments, over many lines.
fn document(bytes: usize) -> String {
    rows(bytes, true)
}

/// The same rows without their indentation: a document that needs pretty-printing, built to the size asked for.
fn document_without_indentation(bytes: usize) -> String {
    rows(bytes, false)
}

fn rows(bytes: usize, indented: bool) -> String {
    let (one, two) = if indented { ("  ", "    ") } else { ("", "") };
    let mut text = String::with_capacity(bytes + 256);
    text.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<table>\n");
    let mut i = 0usize;
    while text.len() < bytes {
        use std::fmt::Write;
        let _ = write!(
            text,
            "{one}<row id=\"{i}\" kind=\"{}\">\n{two}<name>item {i}</name>\n{two}<note>a note about item {i} &amp; more</note>\n{one}</row>\n",
            i % 7
        );
        if i.is_multiple_of(50) {
            let _ = writeln!(text, "{one}<!-- block {} -->", i / 50);
        }
        i += 1;
    }
    text.push_str("</table>\n");
    text
}

/// A pseudo-random number generator (an LCG): the same numbers on every machine.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn percentile(samples: &mut [f64], p: f64) -> f64 {
    samples.sort_by(f64::total_cmp);
    let at = ((samples.len() as f64) * p).ceil() as usize;
    samples.get(at.saturating_sub(1).min(samples.len().saturating_sub(1))).copied().unwrap_or(0.0)
}

fn median_of(runs: usize, mut f: impl FnMut() -> Duration) -> f64 {
    let mut samples: Vec<f64> = (0..runs).map(|_| millis(f())).collect();
    percentile(&mut samples, 0.5)
}

fn open_100mb() -> Result<Measured> {
    let text = document(100 * 1024 * 1024);
    let bytes = text.into_bytes();
    let mut session = Session::new();
    let started = Instant::now();
    session.open(None, &bytes, None).map_err(|e| anyhow::anyhow!("{e}"))?;
    let elapsed = started.elapsed();
    let lines = session.active_document().map_or(0, |d| d.editor().buffer().line_count());
    Ok(Measured { millis: millis(elapsed), what: "once", note: format!("{} MB, {lines} lines", bytes.len() / (1024 * 1024)) })
}

/// What drawing a frame of `rows` lines at `first` costs: their text and their colours.
fn show(buffer: &Buffer, highlighter: &mut Highlighter, first: usize, rows: usize) -> usize {
    let mut spans = 0;
    for line in first..(first + rows).min(buffer.line_count()) {
        let _ = buffer.line_text(line);
        spans += highlighter.spans(buffer, line).len();
    }
    spans
}

fn scroll_100mb_far() -> Result<Measured> {
    let text = document(100 * 1024 * 1024);
    let buffer = Buffer::from_text(&text);
    drop(text);
    let mut highlighter = Highlighter::new();
    let lines = buffer.line_count();
    // The first look at the end of the document: nothing is known about the lines before it.
    let started = Instant::now();
    let spans = show(&buffer, &mut highlighter, lines.saturating_sub(70), 60);
    let cold = started.elapsed();
    // After that the lines in between are known: more jumps are cheap.
    let mut rng = Lcg(7);
    let mut samples: Vec<f64> = (0..50)
        .map(|_| {
            let first = (rng.next() as usize) % lines.saturating_sub(70).max(1);
            let started = Instant::now();
            show(&buffer, &mut highlighter, first, 60);
            millis(started.elapsed())
        })
        .collect();
    let later = percentile(&mut samples, 0.95);
    Ok(Measured { millis: millis(cold), what: "first jump to the end", note: format!("{spans} spans; later jumps p95 {later:.2} ms, {lines} lines") })
}

fn keystroke_1mb() -> Result<Measured> {
    let text = document(1024 * 1024);
    let mut editor = Editor::new(&text);
    let mut highlighter = Highlighter::new();
    let middle = text.len() / 2;
    let at = editor.buffer().line_start(editor.buffer().line_of(middle)).unwrap_or(0);
    editor.set_selection(at, at);
    let first = editor.buffer().line_of(at).saturating_sub(20);
    let mut samples = Vec::new();
    for i in 0..300 {
        let started = Instant::now();
        editor.set_time(i as f64 * 0.001);
        editor.type_text("x");
        // What the view does next: the styler hears about the change, and the visible lines are coloured again.
        if let Some(line) = editor.first_changed_line_since(editor.version().saturating_sub(1)) {
            highlighter.invalidate_from(line);
        }
        show(editor.buffer(), &mut highlighter, first, 60);
        samples.push(millis(started.elapsed()));
    }
    let p95 = percentile(&mut samples, 0.95);
    Ok(Measured { millis: p95, what: "p95 of 300", note: format!("{} lines", editor.buffer().line_count()) })
}

fn reparse(bytes: usize, runs: usize) -> Result<Measured> {
    let text = document(bytes);
    let mut last = None;
    let median = median_of(runs, || {
        let started = Instant::now();
        let analysis = xmlcraft_engine::analysis::analyze(&text, 1);
        let elapsed = started.elapsed();
        // The result it replaces is dropped here, outside the time taken.
        last = Some(analysis);
        elapsed
    });
    let analysis = last.context("the analysis ran")?;
    Ok(Measured { millis: median, what: "median", note: format!("{} outline items, {} fold ranges", analysis.outline.len(), analysis.folds.len()) })
}

fn reparse_1mb() -> Result<Measured> {
    reparse(1024 * 1024, 9)
}

fn reparse_10mb() -> Result<Measured> {
    reparse(10 * 1024 * 1024, 3)
}

fn format_1mb() -> Result<Measured> {
    let text = document_without_indentation(1024 * 1024);
    let options = xmlcraft_format::FormatOptions::default();
    // The same input every time: if it formats once it formats in every run, and a failure is reported, not timed as nothing.
    let edits = xmlcraft_format::pretty_print(&text, &options).map_err(|e| anyhow::anyhow!("formatting the document failed: {e}"))?.len();
    let median = median_of(5, || {
        let started = Instant::now();
        let result = xmlcraft_format::pretty_print(&text, &options);
        let elapsed = started.elapsed();
        drop(result);
        elapsed
    });
    Ok(Measured { millis: median, what: "median", note: format!("{edits} edits") })
}

fn find_all_10mb() -> Result<Measured> {
    let text = document(10 * 1024 * 1024);
    let matcher = xmlcraft_editor::find::Matcher::new(&xmlcraft_editor::find::Query::text("block 7 ")).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut found = 0;
    let median = median_of(3, || {
        let started = Instant::now();
        let result = matcher.find_all(&text, 0..text.len(), xmlcraft_editor::find::MAX_MATCHES);
        let elapsed = started.elapsed();
        found = result.0.len();
        elapsed
    });
    Ok(Measured { millis: median, what: "median", note: format!("{found} matches") })
}

/// The numbers of one table of `xtask/baselines/bench.toml`, in milliseconds, by scenario name.
fn table_of(name: &str) -> Result<BTreeMap<String, f64>> {
    let path = root().join("xtask/baselines/bench.toml");
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let table: toml::Table = text.parse().with_context(|| format!("parsing {}", path.display()))?;
    let mut out = BTreeMap::new();
    if let Some(numbers) = table.get(name).and_then(toml::Value::as_table) {
        for (scenario, value) in numbers {
            let ms =
                value.as_float().or_else(|| value.as_integer().map(|i| i as f64)).with_context(|| format!("{name}.{scenario} is not a number"))?;
            out.insert(scenario.clone(), ms);
        }
    }
    Ok(out)
}

/// The scenario `--only` names, if the option is given; a missing or unknown name is an error, not "run nothing" or "run everything".
fn only_scenario(args: &[String]) -> Result<Option<&'static str>> {
    let Some(i) = args.iter().position(|a| a == "--only") else { return Ok(None) };
    let given = args.get(i + 1).map(String::as_str);
    match SCENARIOS.iter().find(|(name, _, _)| Some(*name) == given) {
        Some((name, _, _)) => Ok(Some(name)),
        None => {
            let names: Vec<&str> = SCENARIOS.iter().map(|(name, _, _)| *name).collect();
            bail!("--only needs the name of a scenario ({}), not {}", names.join(", "), given.unwrap_or("nothing"))
        }
    }
}

pub fn run(args: &[String]) -> Result<()> {
    let flag = |name: &str| args.iter().any(|a| a == name);
    if flag("--list") {
        for (name, what, _) in SCENARIOS {
            println!("{name:20} {what}");
        }
        return Ok(());
    }
    if cfg!(debug_assertions) && !flag("--no-release") {
        // Debug builds are not what users get: run the same command optimised.
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
        let status = std::process::Command::new(cargo)
            .args(["run", "--release", "--quiet", "-p", "xtask", "--", "bench", "--no-release"])
            .args(args)
            .status()
            .context("running the benchmark in release mode")?;
        if !status.success() {
            bail!("the benchmark failed");
        }
        return Ok(());
    }
    let only = only_scenario(args)?;
    let json = flag("--json");
    let (budgets, targets) = (table_of("budget_ms")?, table_of("target_ms")?);
    for name in budgets.keys().chain(targets.keys()) {
        if !SCENARIOS.iter().any(|(n, _, _)| n == name) {
            bail!("xtask/baselines/bench.toml names a scenario that does not exist: {name}");
        }
    }
    for (name, _, _) in SCENARIOS {
        if !budgets.contains_key(*name) {
            bail!("xtask/baselines/bench.toml has no budget for the scenario {name}: every scenario is held to one");
        }
    }
    for (name, target) in &targets {
        if budgets.get(name).is_some_and(|budget| budget < target) {
            bail!("the budget of {name} is below its target: a budget is never lower than the target it works towards");
        }
    }
    let mut over = Vec::new();
    let mut results = Vec::new();
    // With --json standard output is the JSON and nothing else; the table goes to standard error.
    let say = |line: String| if json { eprintln!("{line}") } else { println!("{line}") };
    say(format!("{:20} {:>10}  {:>8}  {:>8}  what", "scenario", "ms", "target", "budget"));
    for (name, _, scenario) in SCENARIOS.iter().filter(|(name, _, _)| only.is_none_or(|o| o == *name)) {
        let measured = scenario().with_context(|| format!("scenario {name}"))?;
        let (budget, target) = (budgets.get(*name).copied(), targets.get(*name).copied());
        let verdict = match (budget, target) {
            (Some(limit), _) if measured.millis > limit => {
                over.push(format!("{name}: {:.1} ms is over its budget of {limit} ms", measured.millis));
                "OVER BUDGET"
            }
            (_, Some(goal)) if measured.millis > goal => "ok (target not met yet)",
            _ => "ok",
        };
        let show = |n: Option<f64>| n.map_or("-".to_owned(), |n| format!("{n}"));
        say(format!(
            "{name:20} {:>10.2}  {:>8}  {:>8}  {} ({}) {verdict}",
            measured.millis,
            show(target),
            show(budget),
            measured.what,
            measured.note
        ));
        results.push(serde_json::json!({
            "scenario": name, "ms": measured.millis, "what": measured.what, "target_ms": target, "budget_ms": budget, "note": measured.note
        }));
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    }
    if !over.is_empty() {
        bail!("over budget:\n  {}", over.join("\n  "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documents_are_the_size_asked_for_and_the_same_every_time() {
        let a = document(10_000);
        assert!(a.len() >= 10_000 && a.len() < 10_400, "{}", a.len());
        assert_eq!(a, document(10_000));
        assert!(xmlcraft_syntax::parse(&a).diagnostics().is_empty(), "a well-formed document");
        let b = document_without_indentation(10_000);
        assert!(b.lines().all(|l| !l.starts_with(' ')));
        assert!(b.len() >= 10_000 && b.len() < 10_400, "built to the size asked for, not shrunk by the indentation left out: {}", b.len());
        assert!(xmlcraft_syntax::parse(&b).diagnostics().is_empty());
        let mut x = Lcg(7);
        let mut y = Lcg(7);
        assert_eq!((x.next(), x.next()), (y.next(), y.next()));
    }

    #[test]
    fn only_names_a_scenario_that_exists() {
        let args = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(only_scenario(&args(&["--json"])).expect("no --only"), None);
        assert_eq!(only_scenario(&args(&["--only", "reparse_1mb", "--json"])).expect("a scenario"), Some("reparse_1mb"));
        for bad in [&["--only"][..], &["--only", "reparse_1m"], &["--only", "--json"]] {
            assert!(only_scenario(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn percentiles_pick_the_sample_that_many_are_below() {
        let mut samples: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!((percentile(&mut samples, 0.5), percentile(&mut samples, 0.95), percentile(&mut samples, 1.0)), (50.0, 95.0, 100.0));
        assert_eq!(percentile(&mut [], 0.95), 0.0);
        assert_eq!(percentile(&mut [3.0], 0.95), 3.0);
    }

    #[test]
    fn the_baselines_name_scenarios_that_exist_and_no_budget_is_below_its_target() {
        let (budgets, targets) = (table_of("budget_ms").expect("budgets"), table_of("target_ms").expect("targets"));
        for name in budgets.keys().chain(targets.keys()) {
            assert!(SCENARIOS.iter().any(|(n, _, _)| n == name), "{name} is not a scenario");
        }
        for (name, target) in &targets {
            assert!(budgets.get(name).is_some_and(|b| b >= target), "{name}: the budget is below the target");
        }
        for (name, _, _) in SCENARIOS {
            assert!(budgets.contains_key(*name), "{name} has no budget");
        }
    }
}
