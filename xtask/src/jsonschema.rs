//! `cargo xtask conformance jsonschema`: the JSON Schema Test Suite (MIT, by the JSON Schema organisation) against XmlCraft's validator,
//! with the ratchet of `xtask/baselines/jsonschema.txt`.
//!
//! Each file of `tests/<draft>/` (the required tests, not the `optional/` ones) is a list of groups: a schema and tests, each with an
//! instance and whether it is valid. A test passes when the validator says what the suite says. The remote documents the tests refer to
//! (`http://localhost:1234/…`) are the files of the suite's `remotes/` folder; nothing else is loaded.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Component, Path};

use anyhow::{Context, Result, bail};
use xmlcraft_json::{Draft, SchemaSet};
use xmlcraft_json_syntax::{Data, Options, Severity, Value, parse};

use crate::conformance::{Tally, fetch_archive, read_baseline, suite_dir, write_baseline_of};
use crate::gates::root;

const URL: &str = "https://github.com/json-schema-org/JSON-Schema-Test-Suite/archive/refs/tags/Test-JSON-Schema-Acceptance-1.039.tar.gz";
const SHA256: &str = "80b84f2c12b1cfb41e00a762aaccc3511542753cc7ecbaafd3d9c5f3dd2e0fa3";
const ARCHIVE: &str = "json-schema-test-suite-1.039.tar.gz";
const FOLDER: &str = "JSON-Schema-Test-Suite-Test-JSON-Schema-Acceptance-1.039";
const REMOTE_BASE: &str = "http://localhost:1234/";

const DRAFTS: &[(&str, Draft)] =
    &[("draft4", Draft::D04), ("draft6", Draft::D06), ("draft7", Draft::D07), ("draft2019-09", Draft::D2019_09), ("draft2020-12", Draft::D2020_12)];

fn read_json(path: &Path) -> Option<Value> {
    let text = String::from_utf8_lossy(&fs::read(path).ok()?).into_owned();
    let read = parse(&text, &Options::default());
    read.root.filter(|_| read.diagnostics.iter().all(|d| d.severity() != Severity::Error))
}

fn text_of(value: Option<&Value>) -> String {
    match value.map(|v| &v.data) {
        Some(Data::String(s)) => s.clone(),
        _ => String::new(),
    }
}

pub fn run(fetch: bool, write: bool, verbose: bool) -> Result<()> {
    let dir = suite_dir();
    let suite = dir.join(FOLDER);
    if fetch && !suite.join("tests").exists() {
        fetch_archive(&dir, URL, SHA256, ARCHIVE)?;
    }
    if !suite.join("tests").exists() {
        eprintln!("jsonschema is not fetched; run `cargo xtask conformance jsonschema --fetch` (skipping)");
        return Ok(());
    }
    let remotes = suite.join("remotes");
    let loader = |uri: &str| -> Option<String> {
        let relative = Path::new(uri.strip_prefix(REMOTE_BASE)?);
        // Only names below the folder: no root, no drive, no `..`.
        if !relative.components().all(|c| matches!(c, Component::Normal(_))) {
            return None;
        }
        fs::read(remotes.join(relative)).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
    };

    // A panic is a failure like any other, listed in the baseline, not a reason to stop (and not noise).
    let _quiet = QuietPanics::new();
    let mut tallies: BTreeMap<&str, Tally> = BTreeMap::new();
    let mut failed: BTreeSet<String> = BTreeSet::new();
    let mut details: BTreeMap<String, String> = BTreeMap::new();
    for (folder, draft) in DRAFTS {
        let mut files: Vec<_> = fs::read_dir(suite.join("tests").join(folder))
            .with_context(|| format!("reading the tests of {folder}"))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        files.sort();
        for path in files {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let Some(Value { data: Data::Array(groups), .. }) = read_json(&path) else { bail!("{} is not a list of test groups", path.display()) };
            for group in &groups {
                let description = text_of(group.get("description"));
                let (Some(schema), Some(Value { data: Data::Array(tests), .. })) = (group.get("schema"), group.get("tests")) else { continue };
                for test in tests {
                    let id = format!("{folder}/{name} | {description} | {}", text_of(test.get("description")));
                    let (Some(data), Some(Value { data: Data::Bool(valid), .. })) = (test.get("data"), test.get("valid")) else { continue };
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        let set = SchemaSet::new(schema.clone(), "", *draft, &loader);
                        let outcome = set.validate(data);
                        (outcome.is_valid(), outcome.violations.first().map(|v| format!("{} at {}: {}", v.keyword, v.instance, v.message)))
                    }));
                    let tally = tallies.entry(folder).or_default();
                    tally.run += 1;
                    match result {
                        Ok((ok, first)) if ok == *valid => {
                            let _ = first;
                            tally.passed += 1;
                        }
                        Ok((ok, first)) => {
                            failed.insert(id.clone());
                            details.insert(id, format!("expected {}, got {} {}", verdict(*valid), verdict(ok), first.unwrap_or_default()));
                        }
                        Err(_) => {
                            failed.insert(id.clone());
                            details.insert(id, "PANIC".to_owned());
                        }
                    }
                }
            }
        }
    }

    println!("jsonschema (JSON Schema Test Suite 1.039, required tests):");
    for (folder, _) in DRAFTS {
        let t = tallies.get(folder).map_or((0, 0), |t| (t.passed, t.run));
        println!("  {folder:<13} {:>5} / {:<5} {:6.2}%", t.0, t.1, 100.0 * t.0 as f64 / t.1.max(1) as f64);
    }
    let (run, ok) = tallies.values().fold((0, 0), |(r, p), t| (r + t.run, p + t.passed));
    println!("  all           {ok:>5} / {run:<5} {:6.2}%", 100.0 * ok as f64 / run.max(1) as f64);

    let baseline_path = root().join("xtask/baselines/jsonschema.txt");
    let baseline = read_baseline(&baseline_path)?;
    if write {
        write_baseline_of(&baseline_path, &failed, "jsonschema", "test per line (the draft and file, the group, and the test)")?;
        println!("wrote {} ({} failing tests)", baseline_path.display(), failed.len());
        return Ok(());
    }
    let regressions: Vec<&String> = failed.difference(&baseline).collect();
    let fixed: Vec<&String> = baseline.difference(&failed).collect();
    for id in regressions.iter().take(200) {
        eprintln!("  NEW FAILURE {id}: {}", details.get(*id).map_or("", String::as_str));
    }
    for id in fixed.iter().take(200) {
        eprintln!("  now passes  {id}: remove it from xtask/baselines/jsonschema.txt (or run with --write)");
    }
    if verbose {
        for id in failed.intersection(&baseline) {
            println!("  known       {id}: {}", details.get(id).map_or("", String::as_str));
        }
    }
    if !regressions.is_empty() || !fixed.is_empty() {
        bail!("{} new failure(s), {} fixed test(s) not yet removed from the baseline", regressions.len(), fixed.len());
    }
    println!("  baseline holds: {} known failures", baseline.len());
    Ok(())
}

/// Panics are silent while this lives and the hook is back when it ends, however the run ends.
type Hook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;

struct QuietPanics(Option<Hook>);

impl QuietPanics {
    fn new() -> Self {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        Self(Some(previous))
    }
}

impl Drop for QuietPanics {
    fn drop(&mut self) {
        if let Some(previous) = self.0.take() {
            std::panic::set_hook(previous);
        }
    }
}

fn verdict(valid: bool) -> &'static str {
    if valid { "valid" } else { "invalid" }
}
