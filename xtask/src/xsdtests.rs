//! `cargo xtask conformance xsdtests`: the W3C XML Schema Test Suite (XSTS, 2007 release for XML Schema 1.0) against XmlCraft's schema
//! engine, with the ratchet of `xtask/baselines/xsdtests.txt`.
//!
//! A *test group* has a schema (one or more schema documents) and instances. A schema test passes when the schema has no error exactly
//! when the suite says it is valid; an instance test passes when the instance has no error against the schema exactly when the suite says
//! it is valid. Only tests the working group has *accepted* are run; those with other verdicts than valid and invalid are skipped.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use anyhow::{Context, Result, bail};
use xmlcraft_syntax::{Severity, parse};
use xmlcraft_text::encoding::{decode, sniff};
use xmlcraft_validate::validate_against_schema;
use xmlcraft_xsd::dom::{self, Dom, El};
use xmlcraft_xsd::schema::Schema;
use xmlcraft_xsd::{LoadRequest, Loaded, SchemaLoader, assemble, load};

use crate::conformance::{Tally, fetch_archive, read_baseline, suite_dir, write_baseline_of};
use crate::gates::root;

/// The XML Schema Test Suite, release of 20 June 2007 (for XML Schema 1.0 second edition), as the W3C publishes it.
const XSTS_URL: &str = "https://www.w3.org/XML/2004/xml-schema-test-suite/xmlschema2006-11-06/xsts-2007-06-20.tar.gz";
const XSTS_SHA256: &str = "902176b25e4111cf96b08663107521a4992e8ea67aad6b815592a6a5b4b9ea06";
const XSTS_ARCHIVE: &str = "xsts-2007-06-20.tar.gz";
/// The folder the archive unpacks to.
const XSTS_FOLDER: &str = "xmlschema2006-11-06";

/// What a test is expected to give.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expected {
    Valid,
    Invalid,
}

#[derive(Debug)]
struct SchemaTest {
    documents: Vec<String>,
    expected: Expected,
}

#[derive(Debug)]
struct InstanceTest {
    name: String,
    document: String,
    expected: Expected,
}

#[derive(Debug)]
struct Group {
    name: String,
    schema: Option<SchemaTest>,
    instances: Vec<InstanceTest>,
}

/// The attribute `local` of `el` (whatever its namespace: `xlink:href` is read as `href`).
fn attribute<'a>(el: &'a El, local: &str) -> Option<&'a str> {
    el.attrs.iter().find(|a| a.local == local).map(|a| a.value.as_str())
}

fn children<'a>(dom: &'a Dom, el: &'a El, local: &'a str) -> impl Iterator<Item = &'a El> + 'a {
    el.children.iter().filter_map(|&i| dom.elements.get(i)).filter(move |c| c.local == local)
}

/// The expected validity, when the test is one that was accepted and says valid or invalid.
fn expectation(dom: &Dom, test: &El) -> Option<Expected> {
    let accepted = children(dom, test, "current").any(|c| attribute(c, "status") == Some("accepted"));
    if !accepted {
        return None;
    }
    match children(dom, test, "expected").find_map(|e| attribute(e, "validity")) {
        Some("valid") => Some(Expected::Valid),
        Some("invalid") => Some(Expected::Invalid),
        _ => None,
    }
}

/// The test groups of a test set, and how many tests were left out (not accepted, or neither valid nor invalid).
fn groups_of(text: &str) -> (Vec<Group>, usize) {
    let parsed = parse(text);
    let dom = dom::build(&parsed.syntax());
    let mut skipped = 0usize;
    let mut groups = Vec::new();
    let Some(root) = dom.elements.first() else { return (groups, 0) };
    for group in children(&dom, root, "testGroup") {
        let name = attribute(group, "name").unwrap_or_default().to_owned();
        let mut schema = None;
        for test in children(&dom, group, "schemaTest") {
            match expectation(&dom, test) {
                Some(expected) if schema.is_none() => {
                    let documents = children(&dom, test, "schemaDocument").filter_map(|d| attribute(d, "href")).map(str::to_owned).collect();
                    schema = Some(SchemaTest { documents, expected });
                }
                Some(_) => {}
                None => skipped += 1,
            }
        }
        let mut instances = Vec::new();
        for test in children(&dom, group, "instanceTest") {
            let document = children(&dom, test, "instanceDocument").find_map(|d| attribute(d, "href"));
            match (expectation(&dom, test), document) {
                (Some(expected), Some(document)) => {
                    instances.push(InstanceTest {
                        name: attribute(test, "name").unwrap_or_default().to_owned(),
                        document: document.to_owned(),
                        expected,
                    });
                }
                _ => skipped += 1,
            }
        }
        groups.push(Group { name, schema, instances });
    }
    (groups, skipped)
}

/// Test groups whose verdict depends on whether the file system of the machine folds case, so that a ratchet that has to say the same on
/// a Mac, on Windows and on CI cannot count them. `schZ012_a` includes `schZ012_A.xsd`, which is the file itself on a Mac or on Windows and
/// no file on Linux (the suite tests that "file URLs are compared case-insensitively": that comparison is the file system's). Left out,
/// like the tests that are not accepted. (`_b` and `_c` ask for a file by another case than its own for an import, which is one file or none:
/// the loader knows a file by its real path, so they pass everywhere.)
const DEPEND_ON_THE_FILE_SYSTEM: &[&str] = &["msMeta/Schema_w3c.xml#schZ012_a"];

/// Whether the verdict on the group `id` (`set#name`) is not the same on every machine.
fn depends_on_the_file_system(id: &str) -> bool {
    DEPEND_ON_THE_FILE_SYSTEM.contains(&id)
}

/// The test sets the suite lists, as paths relative to the suite folder.
fn test_sets(suite_xml: &str) -> Vec<String> {
    let parsed = parse(suite_xml);
    let dom = dom::build(&parsed.syntax());
    let Some(root) = dom.elements.first() else { return Vec::new() };
    children(&dom, root, "testSetRef").filter_map(|r| attribute(r, "href")).map(str::to_owned).collect()
}

fn read_text(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(decode(&bytes, &sniff(&bytes)).text)
}

/// Finds the schema documents a schema includes and imports: files next to the one that asks, and the W3C's `xml.xsd` and the like from
/// the schemas that ship with the app (the suite names them by a URL that is not fetched).
struct Files;

impl SchemaLoader for Files {
    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String> {
        let wanted = request.location.ok_or_else(|| "it names no location".to_owned())?;
        let joined = xmlcraft_resolve::join(request.base, wanted);
        if xmlcraft_resolve::scheme_of(&joined).is_some() {
            let file = joined.rsplit('/').next().unwrap_or(&joined);
            let path = xmlcraft_schemas::paths()
                .find(|p| p.ends_with(&format!("/{file}")))
                .ok_or_else(|| format!("{joined} is not among the schemas that ship"))?;
            return Ok(Loaded { text: xmlcraft_schemas::get_text(path).ok_or("no text")?, location: xmlcraft_schemas::uri_of(path) });
        }
        let text = read_text(Path::new(&joined)).ok_or_else(|| format!("{joined}: cannot be read"))?;
        Ok(Loaded { text, location: joined })
    }

    /// The real path of a file, so that one asked for under two names (`Types.xsd` and `types.xsd` on a file system that folds case) is one
    /// document, as in the app.
    fn identity(&self, location: &str) -> String {
        std::fs::canonicalize(location).map_or_else(|_| location.to_owned(), |real| real.to_string_lossy().into_owned())
    }
}

/// A schema built from the documents of a test, and its errors.
struct Built {
    schema: Schema,
    errors: Vec<String>,
}

fn build(documents: &[String], set: &Path) -> Option<Built> {
    let main = documents.first()?;
    let location = xmlcraft_resolve::join(&set.to_string_lossy(), main);
    let text = read_text(Path::new(&location))?;
    let loaded = load(&text, &location, &mut Files);
    let assembly = assemble(&loaded);
    let errors = loaded
        .diagnostics
        .iter()
        .chain(&assembly.diagnostics)
        .filter(|d| d.diagnostic.severity() == Severity::Error)
        .map(|d| format!("{}: {}", d.diagnostic.code.key(), d.diagnostic.message()))
        .collect();
    Some(Built { schema: assembly.schema, errors })
}

/// The first problem of an instance against a schema, or none.
fn instance_problem(schema: &Schema, text: &str) -> Option<String> {
    let parsed = parse(text);
    if let Some(d) = parsed.diagnostics().iter().find(|d| d.severity() == Severity::Error) {
        return Some(format!("{}: {}", d.code.key(), d.message()));
    }
    validate_against_schema(&parsed.syntax(), schema)
        .iter()
        .find(|d| d.severity() == Severity::Error)
        .map(|d| format!("{}: {}", d.code.key(), d.message()))
}

fn passed(expected: Expected, problem: &Option<String>) -> bool {
    match expected {
        Expected::Valid => problem.is_none(),
        Expected::Invalid => problem.is_some(),
    }
}

pub fn run(fetch: bool, write: bool, verbose: bool) -> Result<()> {
    let dir = suite_dir();
    let suite = dir.join(XSTS_FOLDER);
    if fetch && !suite.join("suite.xml").exists() {
        fetch_archive(&dir, XSTS_URL, XSTS_SHA256, XSTS_ARCHIVE)?;
    }
    let suite_xml = suite.join("suite.xml");
    if !suite_xml.exists() {
        eprintln!("xsdtests is not fetched; run `cargo xtask conformance xsdtests --fetch` (skipping)");
        return Ok(());
    }
    let sets = test_sets(&read_text(&suite_xml).context("reading suite.xml")?);
    if sets.is_empty() {
        bail!("no test sets in {}", suite_xml.display());
    }

    // A panic is a failure like any other, listed in the baseline, not a reason to stop (and not noise).
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));

    let mut tallies: BTreeMap<(&'static str, String), Tally> = BTreeMap::new();
    let mut failed: BTreeSet<String> = BTreeSet::new();
    let mut details: BTreeMap<String, String> = BTreeMap::new();
    let mut skipped = 0usize;
    for set in &sets {
        let set_path = suite.join(set);
        let Some(text) = read_text(&set_path) else { bail!("the test set {set} cannot be read") };
        let contributor = set.split('/').next().unwrap_or_default().trim_end_matches("Meta").to_owned();
        let (groups, left_out) = groups_of(&text);
        skipped += left_out;
        skipped +=
            groups.iter().filter(|g| !g.schema.as_ref().is_some_and(|s| s.expected == Expected::Valid)).map(|g| g.instances.len()).sum::<usize>();
        for group in groups {
            let id = format!("{set}#{}", group.name);
            if depends_on_the_file_system(&id) {
                skipped += usize::from(group.schema.is_some()) + group.instances.len();
                continue;
            }
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                let mut results: Vec<(&'static str, String, bool, String)> = Vec::new();
                let built = group.schema.as_ref().and_then(|s| build(&s.documents, &set_path));
                if let (Some(test), Some(built)) = (&group.schema, &built) {
                    let problem = built.errors.first().cloned();
                    results.push(("schema", id.clone(), passed(test.expected, &problem), problem.unwrap_or_else(|| "no error".to_owned())));
                } else if group.schema.is_some() {
                    results.push(("schema", id.clone(), false, "the schema document cannot be read".to_owned()));
                }
                // The instances of a schema that is not in the suite as an accepted valid one have nothing to be valid against.
                let usable = group.schema.as_ref().is_some_and(|s| s.expected == Expected::Valid);
                for instance in group.instances.iter().filter(|_| usable) {
                    let test_id = format!("{id}/{}", instance.name);
                    let Some(built) = &built else {
                        results.push(("instance", test_id, false, "there is no schema to validate against".to_owned()));
                        continue;
                    };
                    let location = xmlcraft_resolve::join(&set_path.to_string_lossy(), &instance.document);
                    let Some(text) = read_text(Path::new(&location)) else {
                        results.push(("instance", test_id, false, "the instance cannot be read".to_owned()));
                        continue;
                    };
                    let problem = instance_problem(&built.schema, &text);
                    results.push(("instance", test_id, passed(instance.expected, &problem), problem.unwrap_or_else(|| "no error".to_owned())));
                }
                results
            }));
            match outcome {
                Ok(results) => {
                    for (kind, test_id, ok, detail) in results {
                        let tally = tallies.entry((kind, contributor.clone())).or_default();
                        tally.run += 1;
                        if ok {
                            tally.passed += 1;
                        } else {
                            failed.insert(test_id.clone());
                            details.insert(test_id, detail);
                        }
                    }
                }
                Err(_) => {
                    let tally = tallies.entry(("schema", contributor.clone())).or_default();
                    tally.run += 1;
                    failed.insert(id.clone());
                    details.insert(id, "PANIC".to_owned());
                }
            }
        }
    }
    std::panic::set_hook(previous_hook);

    println!("xsdtests (XML Schema Test Suite 2007, XML Schema 1.0, accepted tests):");
    for kind in ["schema", "instance"] {
        let (run, ok) = tallies.iter().filter(|((k, _), _)| *k == kind).fold((0, 0), |(r, p), (_, t)| (r + t.run, p + t.passed));
        let percent = if run == 0 { 100.0 } else { 100.0 * ok as f64 / run as f64 };
        println!("  {kind:<8} {ok:>6} / {run:<6} {percent:6.2}%");
        if verbose {
            for ((k, who), t) in &tallies {
                if *k == kind {
                    println!("    {who:<8} {:>6} / {}", t.passed, t.run);
                }
            }
        }
    }
    let (run, ok) = tallies.values().fold((0, 0), |(r, p), t| (r + t.run, p + t.passed));
    println!(
        "  all      {ok:>6} / {run:<6} {:6.2}%   ({skipped} tests left out: not accepted, or neither valid nor invalid)",
        100.0 * ok as f64 / run.max(1) as f64
    );

    let baseline_path = root().join("xtask/baselines/xsdtests.txt");
    let baseline = read_baseline(&baseline_path)?;
    if write {
        write_baseline_of(&baseline_path, &failed, "xsdtests", "test per line (the test set, the group, and the instance test)")?;
        println!("wrote {} ({} failing tests)", baseline_path.display(), failed.len());
        return Ok(());
    }
    let regressions: Vec<&String> = failed.difference(&baseline).collect();
    let fixed: Vec<&String> = baseline.difference(&failed).collect();
    for id in regressions.iter().take(200) {
        eprintln!("  NEW FAILURE {id}: {}", details.get(*id).map_or("", String::as_str));
    }
    for id in fixed.iter().take(200) {
        eprintln!("  now passes  {id}: remove it from xtask/baselines/xsdtests.txt (or run with --write)");
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

#[cfg(test)]
mod tests {
    use super::*;

    const SET: &str = r#"<testSet xmlns="http://www.w3.org/XML/2004/xml-schema-test-suite/" xmlns:xlink="http://www.w3.org/1999/xlink" name="s">
      <testGroup name="g1">
        <schemaTest name="s1"><schemaDocument xlink:href="../d/a.xsd"/><schemaDocument xlink:href="../d/b.xsd"/><expected validity="valid"/><current status="accepted" date="2005-01-01"/></schemaTest>
        <instanceTest name="i1"><instanceDocument xlink:href="../d/a.xml"/><expected validity="valid"/><current status="accepted" date="2005-01-01"/></instanceTest>
        <instanceTest name="i2"><instanceDocument xlink:href="../d/b.xml"/><expected validity="invalid"/><current status="accepted" date="2005-01-01"/></instanceTest>
        <instanceTest name="queried"><instanceDocument xlink:href="../d/c.xml"/><expected validity="valid"/><current status="queried" date="2005-01-01"/></instanceTest>
        <instanceTest name="open"><instanceDocument xlink:href="../d/d.xml"/><expected validity="indeterminate"/><current status="accepted" date="2005-01-01"/></instanceTest>
      </testGroup>
      <testGroup name="g2">
        <schemaTest name="s2"><schemaDocument xlink:href="../d/x.xsd"/><expected validity="invalid"/><current status="accepted" date="2005-01-01"/></schemaTest>
      </testGroup>
      <testGroup name="g3">
        <schemaTest name="s3"><schemaDocument xlink:href="../d/y.xsd"/><expected validity="valid"/><current status="disputedTest" date="2005-01-01"/></schemaTest>
      </testGroup>
    </testSet>"#;

    #[test]
    fn groups_that_depend_on_the_file_system_are_left_out_and_nothing_else() {
        assert!(depends_on_the_file_system("msMeta/Schema_w3c.xml#schZ012_a"));
        // The same name in another set, or a neighbour in the same one, is run.
        assert!(!depends_on_the_file_system("sunMeta/Schema_w3c.xml#schZ012_a"));
        assert!(!depends_on_the_file_system("msMeta/Schema_w3c.xml#schZ012_b"));
        assert!(!depends_on_the_file_system("msMeta/Schema_w3c.xml#schZ012_a/instance"));
        assert!(DEPEND_ON_THE_FILE_SYSTEM.iter().all(|id| id.split_once('#').is_some_and(|(set, group)| set.ends_with(".xml") && !group.is_empty())));
    }

    #[test]
    fn only_accepted_tests_that_say_valid_or_invalid_are_kept() {
        let (groups, skipped) = groups_of(SET);
        assert_eq!(groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["g1", "g2", "g3"]);
        let g1 = &groups[0];
        let schema = g1.schema.as_ref().expect("a schema");
        assert_eq!((schema.documents.clone(), schema.expected), (vec!["../d/a.xsd".to_owned(), "../d/b.xsd".to_owned()], Expected::Valid));
        assert_eq!(
            g1.instances.iter().map(|i| (i.name.as_str(), i.expected)).collect::<Vec<_>>(),
            [("i1", Expected::Valid), ("i2", Expected::Invalid)]
        );
        assert_eq!(groups[1].schema.as_ref().map(|s| s.expected), Some(Expected::Invalid));
        assert!(groups[2].schema.is_none(), "a test that is not accepted is not run");
        assert_eq!(skipped, 3, "the queried instance, the indeterminate one and the disputed schema");
    }

    #[test]
    fn the_test_sets_are_those_the_suite_lists() {
        let suite = r#"<ts:testSuite xmlns:ts="http://www.w3.org/XML/2004/xml-schema-test-suite/" xmlns:xlink="http://www.w3.org/1999/xlink">
          <ts:testSetRef xlink:href="sunMeta/A.testSet"/><ts:testSetRef xlink:href="msMeta/B.xml"/></ts:testSuite>"#;
        assert_eq!(test_sets(suite), ["sunMeta/A.testSet", "msMeta/B.xml"]);
        assert!(test_sets("not xml at all").is_empty());
    }

    #[test]
    fn a_verdict_agrees_with_the_expectation_exactly() {
        assert!(passed(Expected::Valid, &None) && !passed(Expected::Valid, &Some("e".to_owned())));
        assert!(passed(Expected::Invalid, &Some("e".to_owned())) && !passed(Expected::Invalid, &None));
    }

    #[test]
    fn an_instance_with_a_problem_says_which() {
        let schema = {
            let text = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"><xs:element name=\"a\" type=\"xs:int\"/></xs:schema>";
            let loaded = load(text, "mem:a.xsd", &mut xmlcraft_xsd::NoLoader);
            assemble(&loaded).schema
        };
        assert_eq!(instance_problem(&schema, "<a>1</a>"), None);
        assert!(instance_problem(&schema, "<a>x</a>").is_some_and(|p| p.starts_with("cvc.")), "a validity problem");
        assert!(instance_problem(&schema, "<a>").is_some(), "a document that is not well-formed is a problem too");
    }
}
