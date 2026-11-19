//! `cargo xtask conformance xquery`: the W3C XQuery and XPath test suite (QT3, `github.com/w3c/qt3tests`, under the W3C software and
//! document licence) run through XmlCraft's XQuery engine (`xmlcraft-xquery`), with the ratchet of `xtask/baselines/xquery.txt`.
//!
//! The suite is downloaded into `conformance/` (never committed), pinned to one commit and checked by its SHA-256. The test sets of
//! `prod/` (the grammar's productions) and `fn/` (the functions) are run. A test case runs when its test set and itself depend on
//! XQuery 3.1 (`XQ10+`, `XQ30+`, `XQ31+`, `XQ31`; a test only for XPath, or only for 1.0 or 3.0, is not ours) and on nothing the
//! engine leaves out: schema awareness, static typing, Full-Text, Update, module import, serialization tests, XML 1.1, the namespace
//! axis, collections, and the other optional features (a dependency marked `satisfied="false"` is for engines without the feature, so
//! that test runs). Its environment may give a context document and documents for `fn:doc`, and prefixes; one with a schema, a
//! collection, parameters, a context item expression, collations or decimal formats is skipped. The assertions are those of the
//! suite: `error` (the same code), `assert-eq`, `assert-deep-eq`, `assert`, `assert-true`, `assert-false`, `assert-empty`,
//! `assert-count`, `assert-type`, `assert-string-value`, `assert-xml`, `assert-permutation`, `any-of`, `all-of`, `not`; the
//! serialization assertions fail.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use xmlcraft_xquery::{Check, Options, evaluate_and_check};
use xot::{Node, Xot};

use crate::conformance::{Tally, fetch_archive, read_baseline, suite_dir, write_baseline_of};
use crate::gates::root;

const COMMIT: &str = "201a6e466940cdfc727f4babfedcde5332b9f578";
const URL: &str = "https://github.com/w3c/qt3tests/archive/201a6e466940cdfc727f4babfedcde5332b9f578.tar.gz";
const SHA256: &str = "1b65f66069224fec92445f9d68f85df1f6cde3b75452f26bd4797d9006bd7b65";
const ARCHIVE: &str = "qt3tests-201a6e46.tar.gz";
const FOLDER: &str = "qt3tests-201a6e466940cdfc727f4babfedcde5332b9f578";
/// The test sets run: their files start with one of these.
const SETS: &[&str] = &["prod/", "fn/"];
/// The most interpreter steps one test may take.
const MAX_STEPS: u64 = 5_000_000;
/// The largest file of the suite read.
const MAX_FILE: u64 = 16 * 1024 * 1024;

/// Features a test may need that the engine does not have (a test that needs one is skipped).
const MISSING_FEATURES: &[&str] = &[
    "schemaImport",
    "schemaValidation",
    "schemaAware",
    "staticTyping",
    "serialization",
    "moduleImport",
    "namespace-axis",
    "xpath-1.0-compatibility",
    "typedData",
    "schema-location-hint",
    "collection-stability",
    "directory-as-collection-uri",
    "infoset-dtd",
    "non_unicode_codepoint_collation",
    "non_empty_sequence_collection",
    "olson-timezone",
    "remote_http",
    "simple-uca-fallback",
    "advanced-uca-fallback",
    "fn-transform-XSLT",
    "fn-transform-XSLT30",
    "fn-load-xquery-module",
    "fn-format-integer-CLDR",
    "XQUpdate",
    "fullText",
    "arbitraryPrecisionDecimal",
];

/// Dependency types whose tests are skipped whatever their value (except where noted in `runs`).
const SKIPPED_DEPENDENCIES: &[&str] =
    &["calendar", "default-language", "format-integer-sequence", "language", "limits", "unicode-normalization-form", "unicode-version", "collation"];

fn parse(xot: &mut Xot, path: &Path) -> Result<Node> {
    let len = fs::metadata(path).with_context(|| format!("reading {}", path.display()))?.len();
    if len > MAX_FILE {
        bail!("{} is larger than {MAX_FILE} bytes", path.display());
    }
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    xot.parse(&text).with_context(|| format!("parsing {}", path.display()))
}

fn local(xot: &Xot, node: Node) -> &str {
    xot.element(node).map_or("", |e| xot.local_name_str(e.name()))
}

fn children<'a>(xot: &'a Xot, node: Node, name: &'a str) -> impl Iterator<Item = Node> + 'a {
    xot.children(node).filter(move |c| xot.is_element(*c) && local(xot, *c) == name)
}

fn attr(xot: &Xot, node: Node, name: &str) -> Option<String> {
    xot.attributes(node).iter().find(|(n, _)| xot.local_name_str(*n) == name).map(|(_, v)| v.clone())
}

/// A path of the suite, relative to `dir`, kept inside the suite.
fn inside(suite: &Path, dir: &Path, relative: &str) -> Option<PathBuf> {
    let joined = dir.join(relative);
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out.starts_with(suite).then_some(out)
}

/// What an environment gives a test, or why it cannot run.
#[derive(Clone, Debug, Default)]
struct Environment {
    context: Option<PathBuf>,
    documents: Vec<(String, PathBuf)>,
    namespaces: Vec<(String, String)>,
    unsupported: Option<&'static str>,
}

fn environment(xot: &Xot, node: Node, suite: &Path, dir: &Path) -> Environment {
    let mut env = Environment::default();
    for c in xot.children(node).filter(|c| xot.is_element(*c)) {
        match local(xot, c) {
            "source" => {
                let Some(file) = attr(xot, c, "file").and_then(|f| inside(suite, dir, &f)) else {
                    env.unsupported = Some("a source with no file");
                    continue;
                };
                if attr(xot, c, "validation").is_some_and(|v| v != "skip") {
                    env.unsupported = Some("a validated source");
                }
                match (attr(xot, c, "role").as_deref(), attr(xot, c, "uri")) {
                    (Some("."), _) => env.context = Some(file),
                    (_, Some(uri)) => env.documents.push((uri, file)),
                    _ => env.unsupported = Some("a source with a role"),
                }
            }
            "namespace" => {
                if let (Some(p), Some(u)) = (attr(xot, c, "prefix"), attr(xot, c, "uri")) {
                    env.namespaces.push((p, u));
                }
            }
            "schema" => env.unsupported = Some("a schema"),
            "collection" => env.unsupported = Some("a collection"),
            "param" => env.unsupported = Some("parameters"),
            "context-item" => env.unsupported = Some("a context item expression"),
            "collation" => env.unsupported = Some("a collation"),
            "decimal-format" => env.unsupported = Some("a decimal format"),
            "resource" => env.unsupported = Some("resources"),
            _ => {}
        }
    }
    env
}

/// Why the dependencies of a test set or test case keep it from running here, if they do.
fn skip_reason(xot: &Xot, node: Node) -> Option<String> {
    for d in children(xot, node, "dependency") {
        let kind = attr(xot, d, "type").unwrap_or_default();
        let value = attr(xot, d, "value").unwrap_or_default();
        let satisfied = attr(xot, d, "satisfied").is_none_or(|s| s != "false");
        match kind.as_str() {
            "spec" => {
                let ours = value.split_whitespace().any(|v| matches!(v, "XQ10+" | "XQ30+" | "XQ31+" | "XQ31"));
                if !ours {
                    return Some(format!("spec {value}"));
                }
            }
            "feature" => {
                let missing = value.split_whitespace().any(|v| MISSING_FEATURES.contains(&v));
                if missing && satisfied {
                    return Some(format!("feature {value}"));
                }
                if !missing && !satisfied {
                    return Some(format!("feature {value} (for engines without it)"));
                }
            }
            "xml-version" | "xsd-version" if value.contains("1.1") && satisfied => return Some(format!("{kind} {value}")),
            k if SKIPPED_DEPENDENCIES.contains(&k) => return Some(format!("{kind} {value}")),
            _ => {}
        }
    }
    None
}

/// An XQuery string literal of `s`.
fn literal(s: &str) -> String {
    format!("\"{}\"", s.replace('&', "&amp;").replace('"', "\"\""))
}

/// How a test's `<result>` is judged.
#[derive(Clone, Debug)]
enum Expect {
    /// One of these error codes (`*`: any error).
    Error(String),
    /// A check of the result.
    Holds(Check),
    /// Something this harness does not judge (a failure).
    Unjudged(String),
    AnyOf(Vec<Expect>),
    AllOf(Vec<Expect>),
    Not(Box<Expect>),
}

fn expectation(xot: &Xot, node: Node) -> Expect {
    let text = xot.text_content_str(node).unwrap_or("").to_owned();
    let text = if text.is_empty() { string_value(xot, node) } else { text };
    let holds = |s: String| Expect::Holds(Check::Holds(s));
    match local(xot, node) {
        "error" => Expect::Error(attr(xot, node, "code").unwrap_or_else(|| "*".into())),
        "assert-eq" => holds(format!("$result eq ({text})")),
        "assert-deep-eq" => holds(format!("deep-equal($result, ({text}))")),
        "assert" => holds(text),
        "assert-true" => holds("$result instance of xs:boolean and $result".into()),
        "assert-false" => holds("$result instance of xs:boolean and not($result)".into()),
        "assert-empty" => holds("empty($result)".into()),
        "assert-count" => holds(format!("count($result) eq {}", text.trim())),
        "assert-type" => holds(format!("$result instance of {text}")),
        "assert-permutation" => holds(format!("deep-equal(sort($result), sort(({text})))")),
        "assert-string-value" => {
            let normalize = attr(xot, node, "normalize-space").is_some_and(|v| v == "true" || v == "1");
            let value = "string-join(for $r in $result return string($r), ' ')";
            if normalize {
                holds(format!("normalize-space({value}) eq normalize-space({})", literal(&text)))
            } else {
                holds(format!("{value} eq {}", literal(&text)))
            }
        }
        "assert-xml" => {
            if let Some(file) = attr(xot, node, "file") {
                return Expect::Unjudged(format!("assert-xml from {file}"));
            }
            Expect::Holds(Check::SerializesAs(text))
        }
        "any-of" => Expect::AnyOf(xot.children(node).filter(|c| xot.is_element(*c)).map(|c| expectation(xot, c)).collect()),
        "all-of" => Expect::AllOf(xot.children(node).filter(|c| xot.is_element(*c)).map(|c| expectation(xot, c)).collect()),
        "not" => match xot.children(node).find(|c| xot.is_element(*c)) {
            Some(c) => Expect::Not(Box::new(expectation(xot, c))),
            None => Expect::Unjudged("an empty not".into()),
        },
        other => Expect::Unjudged(other.to_owned()),
    }
}

/// The text of an element, with its descendants' (an `assert-xml` holds markup as text or as elements).
fn string_value(xot: &Xot, node: Node) -> String {
    let mut out = String::new();
    for c in xot.children(node) {
        if xot.is_element(c) {
            out.push_str(&xot.to_string(c).unwrap_or_default());
        } else if let Some(t) = xot.text_str(c) {
            out.push_str(t);
        }
    }
    out
}

fn checks_of(e: &Expect, out: &mut Vec<Check>) {
    match e {
        Expect::Holds(c) => out.push(c.clone()),
        Expect::AnyOf(v) | Expect::AllOf(v) => v.iter().for_each(|x| checks_of(x, out)),
        Expect::Not(x) => checks_of(x, out),
        Expect::Error(_) | Expect::Unjudged(_) => {}
    }
}

/// Whether the expectation is met: `outcome` is the query's error code, or the answers of the checks (in `checks_of` order).
fn met(e: &Expect, outcome: &Result<Vec<bool>, String>, next: &mut usize) -> bool {
    match e {
        Expect::Error(code) => matches!(outcome, Err(c) if code == "*" || code == c),
        Expect::Holds(_) => {
            let at = *next;
            *next += 1;
            matches!(outcome, Ok(answers) if answers.get(at) == Some(&true))
        }
        Expect::Unjudged(_) => false,
        // (Every branch is judged, not only up to the first that decides: each check it holds moves `next` on.)
        Expect::AnyOf(v) => v.iter().map(|x| met(x, outcome, next)).collect::<Vec<_>>().into_iter().any(|m| m),
        Expect::AllOf(v) => v.iter().map(|x| met(x, outcome, next)).collect::<Vec<_>>().into_iter().all(|m| m),
        Expect::Not(x) => !met(x, outcome, next),
    }
}

fn read_text(path: &Path) -> Option<String> {
    fs::metadata(path).ok().filter(|m| m.len() <= MAX_FILE)?;
    fs::read(path).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Run one test case: `Ok(())` when it passes, else what happened.
fn run_case(query: &str, env: &Environment, expect: &Expect) -> Result<(), String> {
    if let Expect::Unjudged(what) = expect {
        return Err(format!("not judged here: {what}"));
    }
    let context = match &env.context {
        Some(p) => Some(read_text(p).ok_or_else(|| format!("cannot read {}", p.display()))?),
        None => None,
    };
    let mut documents = Vec::new();
    for (uri, p) in &env.documents {
        documents.push((uri.clone(), read_text(p).ok_or_else(|| format!("cannot read {}", p.display()))?));
    }
    let options = Options { namespaces: env.namespaces.clone(), documents, max_steps: MAX_STEPS, ..Options::default() };
    let mut checks = Vec::new();
    checks_of(expect, &mut checks);
    let outcome = match evaluate_and_check(query, context.as_deref(), &options, &checks) {
        Ok(answers) => Ok(answers.into_iter().map(|a| a.unwrap_or(false)).collect::<Vec<_>>()),
        Err(e) => Err(e.code),
    };
    let mut next = 0;
    if met(expect, &outcome, &mut next) {
        Ok(())
    } else {
        Err(match outcome {
            Ok(answers) => format!("result does not meet the assertion ({answers:?})"),
            Err(code) => format!("error {code}"),
        })
    }
}

pub fn run(fetch: bool, write: bool, verbose: bool) -> Result<()> {
    let dir = suite_dir();
    let suite = dir.join(FOLDER);
    if fetch && !suite.join("catalog.xml").exists() {
        fetch_archive(&dir, URL, SHA256, ARCHIVE)?;
    }
    if !suite.join("catalog.xml").exists() {
        eprintln!("xquery (QT3) is not fetched; run `cargo xtask conformance xquery --fetch` (skipping)");
        return Ok(());
    }
    let mut xot = Xot::new();
    let catalog = parse(&mut xot, &suite.join("catalog.xml"))?;
    let catalog_root = xot.document_element(catalog).context("the catalog has no root")?;
    let mut shared: HashMap<String, Environment> = HashMap::new();
    for e in children(&xot, catalog_root, "environment").collect::<Vec<_>>() {
        if let Some(name) = attr(&xot, e, "name") {
            shared.insert(name, environment(&xot, e, &suite, &suite));
        }
    }
    let sets: Vec<(String, String)> = children(&xot, catalog_root, "test-set")
        .filter_map(|s| Some((attr(&xot, s, "name")?, attr(&xot, s, "file")?)))
        .filter(|(_, f)| SETS.iter().any(|p| f.starts_with(p)))
        .collect();

    let _quiet = crate::jsonschema::QuietPanics::new();
    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    let mut skipped = 0usize;
    let mut failed: BTreeSet<String> = BTreeSet::new();
    let mut details: BTreeMap<String, String> = BTreeMap::new();
    for (set_name, file) in sets {
        let Some(path) = inside(&suite, &suite, &file) else { continue };
        let set_dir = path.parent().map(Path::to_path_buf).unwrap_or_else(|| suite.clone());
        let mut set_xot = Xot::new();
        let doc = parse(&mut set_xot, &path)?;
        let set = set_xot.document_element(doc).context("a test set has no root")?;
        let set_skip = skip_reason(&set_xot, set);
        let mut local_envs: HashMap<String, Environment> = HashMap::new();
        for e in children(&set_xot, set, "environment").collect::<Vec<_>>() {
            if let Some(name) = attr(&set_xot, e, "name") {
                local_envs.insert(name, environment(&set_xot, e, &suite, &set_dir));
            }
        }
        let group = file.split('/').next().unwrap_or("").to_owned();
        for case in children(&set_xot, set, "test-case").collect::<Vec<_>>() {
            let name = attr(&set_xot, case, "name").unwrap_or_default();
            let id = format!("{set_name}/{name}");
            if set_skip.is_some() || skip_reason(&set_xot, case).is_some() {
                skipped += 1;
                continue;
            }
            let env = match children(&set_xot, case, "environment").next() {
                None => Environment::default(),
                Some(e) => match attr(&set_xot, e, "ref") {
                    Some(r) => local_envs
                        .get(&r)
                        .or_else(|| shared.get(&r))
                        .cloned()
                        .unwrap_or(Environment { unsupported: Some("an unknown environment"), ..Environment::default() }),
                    None => environment(&set_xot, e, &suite, &set_dir),
                },
            };
            if env.unsupported.is_some() {
                skipped += 1;
                continue;
            }
            let Some(test) = children(&set_xot, case, "test").next() else { continue };
            let query = match attr(&set_xot, test, "file") {
                Some(f) => match inside(&suite, &set_dir, &f).and_then(|p| read_text(&p)) {
                    Some(q) => q,
                    None => {
                        skipped += 1;
                        continue;
                    }
                },
                None => set_xot.text_content_str(test).map(str::to_owned).unwrap_or_else(|| string_value(&set_xot, test)),
            };
            let Some(result) = children(&set_xot, case, "result").next() else { continue };
            let Some(first) = set_xot.children(result).find(|c| set_xot.is_element(*c)) else { continue };
            let expect = expectation(&set_xot, first);
            let tally = tallies.entry(format!("{group}/{set_name}")).or_default();
            tally.run += 1;
            match catch_unwind(AssertUnwindSafe(|| run_case(&query, &env, &expect))) {
                Ok(Ok(())) => tally.passed += 1,
                Ok(Err(why)) => {
                    failed.insert(id.clone());
                    details.insert(id, why);
                }
                Err(_) => {
                    failed.insert(id.clone());
                    details.insert(id, "PANIC".to_owned());
                }
            }
        }
    }

    println!("xquery (QT3 {}, prod/ and fn/, XQuery 3.1 test cases this engine's scope covers):", &COMMIT[..8]);
    let mut groups: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (set, t) in &tallies {
        let g = set.split('/').next().unwrap_or("").to_owned();
        let e = groups.entry(g).or_default();
        e.0 += t.passed;
        e.1 += t.run;
        if verbose {
            println!("  {set:<40} {:>5} / {:<5}", t.passed, t.run);
        }
    }
    for (g, (ok, run)) in &groups {
        println!("  {g:<6} {ok:>6} / {run:<6} {:6.2}%", 100.0 * *ok as f64 / (*run).max(1) as f64);
    }
    let (run, ok) = tallies.values().fold((0, 0), |(r, p), t| (r + t.run, p + t.passed));
    println!(
        "  all    {ok:>6} / {run:<6} {:6.2}%   ({skipped} skipped: out of scope or unsupported environment)",
        100.0 * ok as f64 / run.max(1) as f64
    );

    let baseline_path = root().join("xtask/baselines/xquery.txt");
    let baseline = read_baseline(&baseline_path)?;
    if write {
        write_baseline_of(&baseline_path, &failed, "xquery", "test case per line (test set/test case)")?;
        println!("wrote {} ({} failing tests)", baseline_path.display(), failed.len());
        return Ok(());
    }
    let regressions: Vec<&String> = failed.difference(&baseline).collect();
    let fixed: Vec<&String> = baseline.difference(&failed).collect();
    for id in regressions.iter().take(200) {
        eprintln!("  NEW FAILURE {id}: {}", details.get(*id).map_or("", String::as_str));
    }
    for id in fixed.iter().take(200) {
        eprintln!("  now passes  {id}: remove it from xtask/baselines/xquery.txt (or run with --write)");
    }
    if !regressions.is_empty() || !fixed.is_empty() {
        bail!("{} new failure(s), {} fixed test(s) not yet removed from the baseline", regressions.len(), fixed.len());
    }
    println!("  baseline holds: {} known failures", baseline.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(xot: &mut Xot, xml: &str) -> Node {
        let doc = xot.parse(xml).expect("parses");
        xot.document_element(doc).expect("a root")
    }

    #[test]
    fn dependencies_decide_what_runs() {
        let mut xot = Xot::new();
        let ours = element(&mut xot, r#"<t><dependency type="spec" value="XP30+ XQ10+"/></t>"#);
        assert_eq!(skip_reason(&xot, ours), None);
        let xpath = element(&mut xot, r#"<t><dependency type="spec" value="XP31+"/></t>"#);
        assert!(skip_reason(&xot, xpath).is_some());
        let schema = element(&mut xot, r#"<t><dependency type="spec" value="XQ10+"/><dependency type="feature" value="schemaImport"/></t>"#);
        assert!(skip_reason(&xot, schema).is_some());
        let without = element(&mut xot, r#"<t><dependency type="feature" value="schemaImport" satisfied="false"/></t>"#);
        assert_eq!(skip_reason(&xot, without), None, "a test for engines without a feature runs");
    }

    #[test]
    fn assertions_are_judged() {
        let mut xot = Xot::new();
        let any = element(&mut xot, r#"<any-of><error code="FOAR0001"/><assert-eq>1</assert-eq></any-of>"#);
        let e = expectation(&xot, any);
        let mut next = 0;
        assert!(met(&e, &Err("FOAR0001".into()), &mut next));
        let mut next = 0;
        assert!(met(&e, &Ok(vec![true]), &mut next));
        let mut next = 0;
        assert!(!met(&e, &Ok(vec![false]), &mut next));
        assert_eq!(run_case("1 + 1", &Environment::default(), &Expect::Holds(Check::Holds("$result eq 2".into()))), Ok(()));
        assert!(
            run_case("1 + 1", &Environment::default(), &Expect::Holds(Check::Holds("$result eq 3".into()))).is_err(),
            "a check that does not hold fails"
        );
        assert!(run_case("1 div 0", &Environment::default(), &Expect::Error("XPTY0004".into())).is_err(), "another error code fails");
        assert!(run_case("<a/>", &Environment::default(), &Expect::Holds(Check::SerializesAs("<b/>".into()))).is_err());
        assert!(run_case("1 div 0", &Environment::default(), &Expect::Error("FOAR0001".into())).is_ok());
        assert!(run_case("<a b='1'/>", &Environment::default(), &Expect::Holds(Check::SerializesAs("<a b=\"1\"/>".into()))).is_ok());
    }

    #[test]
    fn paths_stay_inside_the_suite() {
        let suite = Path::new("/s");
        assert_eq!(inside(suite, Path::new("/s/prod"), "a.xml"), Some(PathBuf::from("/s/prod/a.xml")));
        assert_eq!(inside(suite, Path::new("/s/prod"), "../../etc/passwd"), None);
    }
}
