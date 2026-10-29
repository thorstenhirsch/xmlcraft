//! `cargo xtask parity`: validate `parity/xmlspy-features.toml` and report progress.
//!
//! Checks:
//! - schema: unique ids (`area.kebab-name`), known area letters, tiers, milestones and statuses, and `na` status exactly
//!   when the tier is `N/A`;
//! - for `shipped` and `partial` features, every listed command exists in the engine registry and
//!   every tool exists in the automation table (so the claim is reachable from the UI and
//!   headlessly);
//! - `shipped` features cite at least one test, and every cited `path::fn` exists.
//!
//! The report gives the shipped share per tier and per area (partial counts half in the
//! weighted figure). `--json` prints the numbers; `--partial` lists partial features with their notes. Fails on any
//! validation error.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::gates::root;

const AREAS: &[(&str, &str)] = &[
    ("A", "Text editing"),
    ("B", "Views and layout"),
    ("C", "DTD"),
    ("D", "XSD"),
    ("E", "Validation and diagnostics"),
    ("F", "Generators and conversion"),
    ("G", "Catalogs, schemas, resolution"),
    ("H", "XPath, XQuery, XSLT"),
    ("I", "JSON and YAML"),
    ("J", "Projects and comparison"),
    ("K", "Application and platform"),
];
const TIERS: &[&str] = &["P0", "P1", "P2", "P3", "N/A"];
const STATUSES: &[&str] = &["planned", "partial", "shipped", "na"];

#[derive(Debug, Deserialize)]
struct File {
    schema: Schema,
    #[serde(rename = "feature")]
    features: Vec<Feature>,
}

#[derive(Debug, Deserialize)]
struct Schema {
    version: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Feature {
    pub id: String,
    pub title: String,
    pub area: String,
    pub tier: String,
    pub milestone: String,
    pub status: String,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub notes: String,
}

/// Quoted ids that follow `prefix(` in a source file: `c("page.rotate", …)` → `page.rotate`.
fn quoted_after(src: &str, prefixes: &[&str]) -> HashSet<String> {
    let mut out = HashSet::new();
    for p in prefixes {
        let pat = format!("{p}(");
        let mut from = 0;
        while let Some(i) = src[from..].find(&pat).map(|i| i + from) {
            from = i + pat.len();
            // Whole identifiers only: `t(` must not match inside `ct(`.
            let boundary = src[..i].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            // The id may start on the next line (`t(\n    "doc_save",`).
            let rest = src[from..].trim_start();
            if boundary
                && let Some(body) = rest.strip_prefix('"')
                && let Some(end) = body.find('"')
            {
                out.insert(body[..end].to_string());
            }
        }
    }
    out
}

/// The files that define tools (`ToolDef { name: "…", … }`): the table itself and the tool modules it collects.
const TOOL_FILES: &[&str] = &[
    "crates/automation/src/tools.rs",
    "crates/automation/src/documents.rs",
    "crates/automation/src/catalog.rs",
    "crates/automation/src/validate.rs",
    "crates/automation/src/entry_helpers.rs",
    "crates/automation/src/assign.rs",
    "crates/automation/src/infer.rs",
    "crates/automation/src/sample.rs",
];

/// The names of every tool in the automation table.
fn tool_names(read: &dyn Fn(&str) -> String) -> HashSet<String> {
    TOOL_FILES.iter().flat_map(|file| quoted_field(&read(file), "name")).collect()
}

/// Quoted values of a struct field in a source file: `name: "app_info"` → `app_info`. Whole field names only.
fn quoted_field(src: &str, field: &str) -> HashSet<String> {
    let pat = format!("{field}:");
    let mut out = HashSet::new();
    let mut from = 0;
    while let Some(i) = src[from..].find(&pat).map(|i| i + from) {
        from = i + pat.len();
        let boundary = src[..i].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        if boundary
            && let Some(body) = src[from..].trim_start().strip_prefix('"')
            && let Some(end) = body.find('"')
        {
            out.insert(body[..end].to_string());
        }
    }
    out
}

fn valid_id(id: &str) -> bool {
    let Some((area, name)) = id.split_once('.') else { return false };
    !area.is_empty()
        && area.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && !name.is_empty()
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
}

/// The highest milestone number of docs/roadmap.md.
const LAST_MILESTONE: u32 = 9;

fn valid_milestone(m: &str) -> bool {
    m == "none" || m.strip_prefix('M').and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| n <= LAST_MILESTONE)
}

/// Why an `evidence` entry (`path.rs::test_fn`) does not resolve to a test in the source tree, if it doesn't.
fn evidence_error(ev: &str, repo: &Path) -> Option<String> {
    let Some((path, func)) = ev.split_once("::") else {
        return Some(format!("evidence {ev:?} must be `path.rs::test_fn`"));
    };
    match std::fs::read_to_string(repo.join(path)) {
        Ok(src) if src.contains(&format!("fn {func}(")) => None,
        Ok(_) => Some(format!("evidence: no `fn {func}` in {path}")),
        Err(_) => Some(format!("evidence: {path} does not exist")),
    }
}

/// Validate features against the registry, tool table and source tree; returns the errors.
pub fn validate(features: &[Feature], commands: &HashSet<String>, tools: &HashSet<String>, repo: &Path) -> Vec<String> {
    let mut errors = Vec::new();
    let mut ids = HashSet::new();
    for f in features {
        let e = |m: String| format!("{}: {m}", f.id);
        if !ids.insert(f.id.as_str()) {
            errors.push(e("duplicate id".into()));
        }
        if !valid_id(&f.id) {
            errors.push(e("id must look like `area.kebab-name`".into()));
        }
        if f.title.trim().is_empty() {
            errors.push(e("empty title".into()));
        }
        if !AREAS.iter().any(|(a, _)| *a == f.area) {
            errors.push(e(format!("unknown area {:?} (A–K)", f.area)));
        }
        if !TIERS.contains(&f.tier.as_str()) {
            errors.push(e(format!("unknown tier {:?}", f.tier)));
        }
        if !STATUSES.contains(&f.status.as_str()) {
            errors.push(e(format!("unknown status {:?}", f.status)));
        }
        if !valid_milestone(&f.milestone) {
            errors.push(e(format!("milestone {:?} must be M0–M{LAST_MILESTONE} or none", f.milestone)));
        }
        if (f.status == "na") != (f.tier == "N/A") {
            errors.push(e("status `na` and tier `N/A` go together (out-of-scope features are excluded from the percentages)".into()));
        }
        if matches!(f.status.as_str(), "shipped" | "partial") {
            for c in f.commands.iter().filter(|c| !commands.contains(*c)) {
                errors.push(e(format!("command {c:?} is not in the engine registry")));
            }
            for t in f.tools.iter().filter(|t| !tools.contains(*t)) {
                errors.push(e(format!("tool {t:?} is not in the automation table")));
            }
        }
        if f.status == "shipped" && f.evidence.is_empty() {
            errors.push(e("shipped features must cite at least one test in `evidence`".into()));
        }
        for ev in &f.evidence {
            if let Some(m) = evidence_error(ev, repo) {
                errors.push(e(m));
            }
        }
    }
    errors
}

pub fn run(args: &[String]) -> Result<()> {
    let repo = root();
    let path = repo.join("parity/xmlspy-features.toml");
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let file: File = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    if file.schema.version != 1 {
        bail!("unsupported parity schema version {}", file.schema.version);
    }
    // The registries arrive with milestones M0.8 and M0.9; until then nothing can be reached from them, so a missing file is an
    // empty registry (and any `shipped` or `partial` feature that names a command or tool is reported).
    let read = |p: &str| std::fs::read_to_string(repo.join(p)).unwrap_or_default();
    let commands = quoted_after(&read("crates/engine/src/commands.rs"), &["c", "ct"]);
    let tools = tool_names(&read);
    let errors = validate(&file.features, &commands, &tools, &repo);

    // Progress: per tier and per area. N/A tier and `na` status are excluded from percentages.
    let counted: Vec<&Feature> = file.features.iter().filter(|f| f.status != "na" && f.tier != "N/A").collect();
    let score = |fs: &[&Feature]| {
        let shipped = fs.iter().filter(|f| f.status == "shipped").count();
        let partial = fs.iter().filter(|f| f.status == "partial").count();
        let pct = |x: f64| if fs.is_empty() { 0.0 } else { 100.0 * x / fs.len() as f64 };
        (fs.len(), shipped, partial, pct(shipped as f64), pct(shipped as f64 + partial as f64 / 2.0))
    };
    let mut by_tier = BTreeMap::new();
    for t in TIERS.iter().filter(|t| **t != "N/A") {
        let fs: Vec<&Feature> = counted.iter().copied().filter(|f| f.tier == *t).collect();
        by_tier.insert(*t, score(&fs));
    }
    let mut by_area = Vec::new();
    for (a, name) in AREAS {
        let fs: Vec<&Feature> = counted.iter().copied().filter(|f| f.area == *a).collect();
        by_area.push((*a, *name, score(&fs)));
    }
    let total = score(&counted);
    let without_headless: Vec<&str> = file.features.iter().filter(|f| f.status == "shipped" && f.tools.is_empty()).map(|f| f.id.as_str()).collect();

    if args.iter().any(|a| a == "--json") {
        let json = serde_json::json!({
            "features": file.features.len(),
            "total": { "counted": total.0, "shipped": total.1, "partial": total.2, "shipped_pct": total.3, "weighted_pct": total.4 },
            "tiers": by_tier.iter().map(|(t, s)| (t.to_string(), serde_json::json!({ "counted": s.0, "shipped": s.1, "partial": s.2, "shipped_pct": s.3 }))).collect::<serde_json::Map<_, _>>(),
            "errors": errors,
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        println!("parity: {} features ({} counted; {} na)", file.features.len(), counted.len(), file.features.len() - counted.len());
        println!("\n  tier  features  shipped  partial  shipped%  weighted%");
        for (t, s) in &by_tier {
            println!("  {t:<4}  {:>8}  {:>7}  {:>7}  {:>7.1}%  {:>8.1}%", s.0, s.1, s.2, s.3, s.4);
        }
        println!("  all   {:>8}  {:>7}  {:>7}  {:>7.1}%  {:>8.1}%", total.0, total.1, total.2, total.3, total.4);
        println!("\n  area                                features  shipped  partial  weighted%");
        for (a, name, s) in &by_area {
            println!("  {a} {name:<33} {:>8}  {:>7}  {:>7}  {:>8.1}%", s.0, s.1, s.2, s.4);
        }
        if args.iter().any(|a| a == "--partial") {
            println!("\npartial features and what is missing:");
            for f in file.features.iter().filter(|f| f.status == "partial") {
                println!("  {:<40} {}", f.id, f.notes);
            }
        }
        if !without_headless.is_empty() {
            println!(
                "\nnote: {} shipped feature(s) have no automation tool yet (AGENTS.md §3): {}",
                without_headless.len(),
                without_headless.join(", ")
            );
        }
    }
    if !errors.is_empty() {
        for e in &errors {
            eprintln!("parity error: {e}");
        }
        bail!("{} parity error(s)", errors.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(id: &str, status: &str) -> Feature {
        Feature {
            id: id.into(),
            title: "T".into(),
            area: "D".into(),
            tier: "P0".into(),
            milestone: "M4".into(),
            status: status.into(),
            commands: vec![],
            tools: vec![],
            evidence: vec![],
            notes: String::new(),
        }
    }

    fn committed() -> File {
        let path = root().join("parity/xmlspy-features.toml");
        let text = std::fs::read_to_string(&path).expect("read the committed parity manifest");
        toml::from_str(&text).expect("the committed parity manifest must be valid TOML")
    }

    #[test]
    fn committed_manifest_is_well_formed_and_makes_no_false_claims() {
        let file = committed();
        assert_eq!(file.schema.version, 1);
        assert!(file.features.len() > 100, "the checklist should cover the whole feature set");
        let read = |p: &str| std::fs::read_to_string(root().join(p)).unwrap_or_default();
        let commands = quoted_after(&read("crates/engine/src/commands.rs"), &["c", "ct"]);
        let tools = tool_names(&read);
        let errors = validate(&file.features, &commands, &tools, &root());
        assert!(errors.is_empty(), "{errors:#?}");
    }

    #[test]
    fn every_known_area_is_used() {
        let file = committed();
        for (letter, name) in AREAS {
            assert!(file.features.iter().any(|f| f.area == *letter), "area {letter} ({name}) has no features");
        }
    }

    #[test]
    fn registry_ids_are_extracted_from_source() {
        let src =
            "c(\"file.open\", \"Open…\", FILE, None), ct(\"edit.undo\", \"Undo\"), t(\"doc_open\", \"Open\"), t(\n    \"doc_save\",), format(x)";
        assert_eq!(quoted_after(src, &["c", "ct"]), HashSet::from(["file.open".to_string(), "edit.undo".to_string()]));
        assert_eq!(quoted_after(src, &["t"]), HashSet::from(["doc_open".to_string(), "doc_save".to_string()]));
    }

    #[test]
    fn tool_names_are_read_from_the_name_fields_of_the_table() {
        let src = "ToolDef { name: \"app_info\", title: \"x\" }, ToolDef {\n  name:   \"command_list\", other_name: \"no\", name: 3 }";
        assert_eq!(quoted_field(src, "name"), HashSet::from(["app_info".to_string(), "command_list".to_string()]));
        assert!(quoted_field("", "name").is_empty());
    }

    #[test]
    fn validation_catches_false_claims() {
        let repo = root();
        let cmds = HashSet::from(["page.rotate".to_string()]);
        let tools = HashSet::from(["page_rotate".to_string()]);
        let mut ok = f("organize.rotate", "shipped");
        ok.commands = vec!["page.rotate".into()];
        ok.tools = vec!["page_rotate".into()];
        ok.evidence = vec!["xtask/src/parity.rs::validation_catches_false_claims".into()];
        assert!(validate(&[ok], &cmds, &tools, &repo).is_empty());

        let mut bad = f("organize.Rotate", "shipped");
        bad.commands = vec!["page.spin".into()];
        bad.evidence = vec!["xtask/src/parity.rs::no_such_test".into(), "nowhere.rs::x".into(), "nopath".into()];
        let errs = validate(&[bad, f("organize.x", "planned"), f("organize.x", "bogus")], &cmds, &tools, &repo);
        let all = errs.join("\n");
        for needle in ["kebab", "registry", "no `fn no_such_test`", "does not exist", "path.rs::test_fn", "duplicate id", "unknown status"] {
            assert!(all.contains(needle), "missing {needle:?} in:\n{all}");
        }
        assert!(validate(&[f("organize.y", "shipped")], &cmds, &tools, &repo)[0].contains("at least one test"));
    }

    #[test]
    fn na_status_and_n_a_tier_go_together() {
        let (cmds, tools, repo) = (HashSet::new(), HashSet::new(), root());
        let mut na_wrong_tier = f("scope.x", "na");
        na_wrong_tier.tier = "P1".into();
        assert!(validate(&[na_wrong_tier], &cmds, &tools, &repo).iter().any(|e| e.contains("go together")));
        let mut tier_wrong_status = f("scope.y", "planned");
        tier_wrong_status.tier = "N/A".into();
        assert!(validate(&[tier_wrong_status], &cmds, &tools, &repo).iter().any(|e| e.contains("go together")));
        let mut fine = f("scope.z", "na");
        fine.tier = "N/A".into();
        assert!(validate(&[fine], &cmds, &tools, &repo).is_empty());
    }

    #[test]
    fn milestones_are_bounded_by_the_roadmap() {
        for ok in ["M0", "M9", "none"] {
            assert!(valid_milestone(ok), "{ok}");
        }
        for bad in ["M10", "m1", "9", "", "M-1", "Mx"] {
            assert!(!valid_milestone(bad), "{bad}");
        }
    }
}
