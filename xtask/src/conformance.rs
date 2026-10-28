//! `cargo xtask conformance <suite>`: run a W3C/OASIS conformance suite against XmlCraft and hold the line.
//!
//! The suites are fetched into the gitignored `conformance/` directory, pinned by URL and SHA-256. A run compares the failures with
//! the list in `xtask/baselines/<suite>.txt`: a test that fails and is not listed fails the run (a regression); a listed test that
//! now passes also fails it, until the line is removed (the ratchet only tightens). `--write` rewrites the list from the run.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use xmlcraft_syntax::{SyntaxKind, SyntaxNode, parse};
use xmlcraft_text::encoding::{decode, sniff};

use crate::gates::root;

/// The XML Conformance Test Suites, 23 September 2013 (the last edition of the suite).
const XMLCONF_URL: &str = "https://www.w3.org/XML/Test/xmlts20130923.tar.gz";
const XMLCONF_SHA256: &str = "9b61db9f5dbffa545f4b8d78422167083a8568c59bd1129f94138f936cf6fc1f";
const XMLCONF_ARCHIVE: &str = "xmlts20130923.tar.gz";

pub fn run(args: &[String]) -> Result<()> {
    let mut suite = None;
    let (mut fetch, mut write, mut verbose) = (false, false, false);
    for arg in args {
        match arg.as_str() {
            "--fetch" => fetch = true,
            "--write" => write = true,
            "--verbose" | "-v" => verbose = true,
            other if other.starts_with('-') => bail!("unknown option `{other}`"),
            other => suite = Some(other),
        }
    }
    match suite {
        Some("xmlconf") => xmlconf(fetch, write, verbose),
        Some(other) => bail!("unknown suite `{other}` (available: xmlconf)"),
        None => bail!("usage: cargo xtask conformance xmlconf [--fetch] [--write] [--verbose]"),
    }
}

// --- fetching ---------------------------------------------------------------------------------------------------------------

fn suite_dir() -> PathBuf {
    root().join("conformance")
}

fn fetch_xmlconf(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let archive = dir.join(XMLCONF_ARCHIVE);
    eprintln!("fetching {XMLCONF_URL}");
    let status = Command::new("curl")
        .args(["--proto", "=https", "--tlsv1.2", "--fail", "--silent", "--show-error", "--location", "--output"])
        .arg(&archive)
        .arg(XMLCONF_URL)
        .status()
        .context("running curl (is it installed?)")?;
    if !status.success() {
        bail!("curl failed with {status}");
    }
    let bytes = fs::read(&archive).with_context(|| format!("reading {}", archive.display()))?;
    let digest = hex(&Sha256::digest(&bytes));
    if digest != XMLCONF_SHA256 {
        let _ = fs::remove_file(&archive);
        bail!("the downloaded archive has SHA-256 {digest}, expected {XMLCONF_SHA256}; it was deleted");
    }
    let status = Command::new("tar").arg("xzf").arg(&archive).arg("-C").arg(dir).status().context("running tar (is it installed?)")?;
    if !status.success() {
        bail!("tar failed with {status}");
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// --- the catalog ------------------------------------------------------------------------------------------------------------

/// One test of the suite.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Case {
    /// The test file, relative to the suite directory, with `/` separators.
    path: String,
    kind: String,
    version: Option<String>,
    edition: Option<String>,
    recommendation: Option<String>,
    namespace: Option<String>,
    /// Which entities the test needs a processor to read: `none`, `general`, `parameter` or `both`.
    entities: Option<String>,
    /// The sections of the specification it tests.
    sections: Option<String>,
}

/// The general entities `<!ENTITY name SYSTEM "file">` declared in the catalog's internal subset.
fn system_entities(catalog: &str) -> HashMap<String, String> {
    let mut entities = HashMap::new();
    for (at, _) in catalog.match_indices("<!ENTITY") {
        let rest = catalog.get(at + "<!ENTITY".len()..).unwrap_or_default().trim_start();
        let name_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let (name, rest) = rest.split_at(name_end);
        let Some(rest) = rest.trim_start().strip_prefix("SYSTEM") else { continue };
        let rest = rest.trim_start();
        let Some(quote) = rest.chars().next().filter(|c| matches!(c, '"' | '\'')) else { continue };
        let inner = rest.get(1..).unwrap_or_default();
        if let Some(end) = inner.find(quote) {
            entities.insert(name.to_owned(), inner.get(..end).unwrap_or_default().to_owned());
        }
    }
    entities
}

/// The catalog as one document: the part after the DOCTYPE, with each `&name;` replaced by the content of the file it names.
fn expand_catalog(catalog: &str, read: &dyn Fn(&str) -> Option<String>) -> String {
    let entities = system_entities(catalog);
    let body = catalog.split_once("]>").map_or(catalog, |(_, body)| body);
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(at) = rest.find('&') {
        out.push_str(rest.get(..at).unwrap_or_default());
        let tail = rest.get(at..).unwrap_or_default();
        let end = tail.find(';').map_or(tail.len(), |e| e + 1);
        let reference = tail.get(..end).unwrap_or_default();
        let name = reference.trim_start_matches('&').trim_end_matches(';');
        match entities.get(name).and_then(|file| read(file)) {
            Some(content) => out.push_str(strip_declaration(&content)),
            None => out.push_str(reference),
        }
        rest = tail.get(end..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// `text` without a leading `<?xml ...?>` (an external entity's text declaration).
fn strip_declaration(text: &str) -> &str {
    let trimmed = text.trim_start_matches('\u{FEFF}');
    if trimmed.starts_with("<?xml")
        && let Some(end) = trimmed.find("?>")
    {
        return trimmed.get(end + 2..).unwrap_or_default();
    }
    trimmed
}

fn element_name(element: &SyntaxNode) -> Option<String> {
    let tag = element.children().find(|n| n.kind() == SyntaxKind::StartTag)?;
    tag.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == SyntaxKind::Name).map(|t| t.text().to_owned())
}

fn attributes(element: &SyntaxNode) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(tag) = element.children().find(|n| n.kind() == SyntaxKind::StartTag) else { return out };
    for attribute in tag.children().filter(|n| n.kind() == SyntaxKind::Attribute) {
        let name = attribute.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == SyntaxKind::Name);
        let value = attribute.children().find(|n| n.kind() == SyntaxKind::AttrValue);
        if let (Some(name), Some(value)) = (name, value) {
            let text = value.text().to_string();
            out.insert(name.text().to_owned(), text.get(1..text.len().saturating_sub(1)).unwrap_or_default().to_owned());
        }
    }
    out
}

/// The `xml:base` values from the outermost element down to `element` itself: a `URI` attribute is relative to the base of the element
/// that carries it.
fn bases(element: &SyntaxNode) -> Vec<String> {
    let mut chain: Vec<String> =
        element.ancestors().filter(|n| n.kind() == SyntaxKind::Element).filter_map(|n| attributes(&n).remove("xml:base")).collect();
    chain.reverse();
    chain
}

/// `base` segments and `uri` joined into a normalised path relative to the suite directory; `None` if it climbs out of it.
fn join_path(bases: &[String], uri: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for piece in bases.iter().map(String::as_str).chain(std::iter::once(uri)) {
        for part in piece.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop()?;
                }
                other => parts.push(other),
            }
        }
    }
    Some(parts.join("/"))
}

fn cases(expanded: &str) -> Vec<Case> {
    let parsed = parse(expanded);
    let mut out = Vec::new();
    for element in parsed.syntax().descendants().filter(|n| n.kind() == SyntaxKind::Element) {
        if element_name(&element).as_deref() != Some("TEST") {
            continue;
        }
        let mut attrs = attributes(&element);
        let (Some(uri), Some(kind)) = (attrs.remove("URI"), attrs.remove("TYPE")) else { continue };
        let Some(path) = join_path(&bases(&element), &uri) else { continue };
        out.push(Case {
            path,
            kind,
            version: attrs.remove("VERSION"),
            edition: attrs.remove("EDITION"),
            recommendation: attrs.remove("RECOMMENDATION"),
            namespace: attrs.remove("NAMESPACE"),
            entities: attrs.remove("ENTITIES"),
            sections: attrs.remove("SECTIONS"),
        });
    }
    out
}

/// Why a test does not apply to XmlCraft (a non-validating XML 1.0, fifth edition, namespace-aware checker), if it does not.
fn skip_reason(case: &Case) -> Option<&'static str> {
    if !matches!(case.kind.as_str(), "valid" | "invalid" | "not-wf") {
        return Some("a processor may report either way");
    }
    if case.version.as_deref().is_some_and(|v| !v.split_whitespace().any(|x| x == "1.0")) {
        return Some("XML 1.1 only");
    }
    if case.edition.as_deref().is_some_and(|e| !e.split_whitespace().any(|x| x == "5")) {
        return Some("not for the fifth edition");
    }
    if case.recommendation.as_deref().is_some_and(|r| r.starts_with("NS1.1") || r.starts_with("XML1.1")) {
        return Some("XML or Namespaces 1.1");
    }
    if case.namespace.as_deref() == Some("no") {
        return Some("for processors that ignore namespaces");
    }
    None
}

/// Where a test file is on disk. The catalog's `xml:base` for the "misc" group of the Edinburgh tests says `eduni/namespaces/misc/`,
/// but the files are in `eduni/misc/` (an error in the published suite).
fn upstream_path(suite: &Path, path: &str) -> String {
    match path.strip_prefix("eduni/namespaces/misc/") {
        Some(rest) if !suite.join(path).exists() => format!("eduni/misc/{rest}"),
        _ => path.to_owned(),
    }
}

// --- running ----------------------------------------------------------------------------------------------------------------

/// Reads the external parts of a DTD from the suite's files: a system identifier is a path relative to the file that has it, and
/// nothing outside the suite (and no URL) is read.
struct SuiteFiles<'a> {
    suite: &'a Path,
    /// Why a file that was found cannot be text of the document (it is not valid in its encoding): the document is not well-formed for
    /// it, which is not the same as a part that is missing.
    rejected: Option<String>,
}

/// The most one external text of the suite may be.
const MAX_SUITE_FILE: u64 = 64 * 1024 * 1024;

/// Whether a system identifier starts with a URI scheme (`http:`, `file:`, …): it names no file of the suite.
fn has_scheme(system: &str) -> bool {
    let Some((scheme, _)) = system.split_once(':') else { return false };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

impl xmlcraft_dtd::SubsetLoader for SuiteFiles<'_> {
    fn load(&mut self, request: &xmlcraft_dtd::LoadRequest<'_>) -> Result<xmlcraft_dtd::Loaded, String> {
        if request.system.starts_with('/') || has_scheme(request.system) {
            return Err("not a path inside the suite".to_owned());
        }
        let base = Path::new(request.base);
        let path = base.parent().unwrap_or(Path::new("")).join(request.system);
        // Normalise `..` without touching the disk, and stay inside the suite.
        let mut parts: Vec<std::ffi::OsString> = Vec::new();
        for part in path.components() {
            match part {
                std::path::Component::ParentDir => {
                    parts.pop();
                }
                std::path::Component::CurDir => {}
                other => parts.push(other.as_os_str().to_owned()),
            }
        }
        let resolved: PathBuf = parts.iter().collect();
        // Where the file really is (a link is followed) must be inside the suite too.
        let real = fs::canonicalize(&resolved).map_err(|e| e.to_string())?;
        let suite = fs::canonicalize(self.suite).map_err(|e| e.to_string())?;
        if !real.starts_with(&suite) {
            return Err("outside the suite".to_owned());
        }
        // The size first: a file over the limit is not read into memory only to be refused.
        if fs::metadata(&real).map_err(|e| e.to_string())?.len() > MAX_SUITE_FILE {
            return Err(format!("over {MAX_SUITE_FILE} bytes"));
        }
        let bytes = fs::read(&real).map_err(|e| e.to_string())?;
        let detected = sniff(&bytes);
        let decoded = decode(&bytes, &detected);
        let wrong = if decoded.lossy {
            Some(format!("not valid {:?}", detected.encoding))
        } else if !detected.declaration_matches() {
            Some(format!("declares {:?} but is {:?}", detected.declared.clone().unwrap_or_default(), detected.encoding))
        } else {
            None
        };
        if let Some(reason) = wrong {
            let reason = format!("{}: {reason}", resolved.display());
            self.rejected.get_or_insert_with(|| reason.clone());
            return Err(reason);
        }
        Ok(xmlcraft_dtd::Loaded { text: decoded.text, location: resolved.to_string_lossy().into_owned() })
    }
}

/// What XmlCraft says about a document's bytes: `None` for well-formed, else the first problem. `path` is where the document is, for the
/// external parts of its DTD.
fn reject_reason(bytes: &[u8], path: &Path, suite: &Path) -> Option<String> {
    let detected = sniff(bytes);
    let decoded = decode(bytes, &detected);
    if decoded.lossy {
        return Some(format!("not valid {:?}", detected.encoding));
    }
    if !detected.declaration_matches() {
        return Some(format!("declares {:?} but is {:?}", detected.declared.unwrap_or_default(), detected.encoding));
    }
    let parsed = parse(&decoded.text);
    // The problems of the document and of its DTD: the internal subset is read by the DTD reader, the external parts come from the suite.
    let parts = xmlcraft_dtd::doctype_parts(&parsed.syntax());
    let location = path.to_string_lossy();
    let mut files = SuiteFiles { suite, rejected: None };
    let problems = xmlcraft_dtd::problems_of_document(&decoded.text, parsed.diagnostics(), parts.as_ref(), &location, &mut files);
    problems
        .iter()
        .find(|d| d.diagnostic.severity() == xmlcraft_syntax::Severity::Error)
        .map(|d| if d.source == 0 { d.diagnostic.message() } else { format!("in external text {}: {}", d.source, d.diagnostic.message()) })
        .or(files.rejected)
}

#[derive(Default)]
struct Tally {
    run: usize,
    passed: usize,
}

fn xmlconf(fetch: bool, write: bool, verbose: bool) -> Result<()> {
    let dir = suite_dir();
    let suite = dir.join("xmlconf");
    if fetch && !suite.join("xmlconf.xml").exists() {
        fetch_xmlconf(&dir)?;
    }
    let catalog_path = suite.join("xmlconf.xml");
    if !catalog_path.exists() {
        eprintln!("xmlconf is not fetched; run `cargo xtask conformance xmlconf --fetch` (skipping)");
        return Ok(());
    }
    let catalog = String::from_utf8_lossy(&fs::read(&catalog_path)?).into_owned();
    let read = |file: &str| -> Option<String> {
        let path = join_path(&[], file)?;
        fs::read(suite.join(path)).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
    };
    let expanded = expand_catalog(&catalog, &read);
    let all = cases(&expanded);
    if all.is_empty() {
        bail!("no tests found in {}", catalog_path.display());
    }

    let mut skipped: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut tallies: BTreeMap<(String, String), Tally> = BTreeMap::new();
    let mut failed: BTreeSet<String> = BTreeSet::new();
    let mut details: BTreeMap<String, String> = BTreeMap::new();
    for case in &all {
        if let Some(reason) = skip_reason(case) {
            *skipped.entry(reason).or_default() += 1;
            continue;
        }
        let Some(bytes) = fs::read(suite.join(upstream_path(&suite, &case.path))).ok() else {
            bail!("the test file {} is missing", case.path);
        };
        let file = suite.join(upstream_path(&suite, &case.path));
        let verdict = reject_reason(&bytes, &file, &suite);
        let expect_wf = case.kind != "not-wf";
        let passed = verdict.is_none() == expect_wf;
        let group = case.path.split('/').next().unwrap_or_default().to_owned();
        let tally = tallies.entry((case.kind.clone(), group)).or_default();
        tally.run += 1;
        if passed {
            tally.passed += 1;
        } else {
            failed.insert(case.path.clone());
            let what = verdict.unwrap_or_else(|| "accepted".to_owned());
            details.insert(
                case.path.clone(),
                format!(
                    "{} test (entities: {}, §{}): {what}",
                    case.kind,
                    case.entities.as_deref().unwrap_or("-"),
                    case.sections.as_deref().unwrap_or("?")
                ),
            );
        }
    }

    println!("xmlconf (XML 1.0 fifth edition, non-validating, namespace-aware):");
    for kind in ["not-wf", "valid", "invalid"] {
        let (run, passed) = tallies.iter().filter(|((k, _), _)| k == kind).fold((0, 0), |(r, p), (_, t)| (r + t.run, p + t.passed));
        let percent = if run == 0 { 100.0 } else { 100.0 * passed as f64 / run as f64 };
        println!("  {kind:<7} {passed:>5} / {run:<5} {percent:6.2}%");
        if verbose {
            for ((k, group), t) in &tallies {
                if k == kind {
                    println!("    {group:<10} {:>5} / {}", t.passed, t.run);
                }
            }
        }
    }
    for (reason, count) in &skipped {
        println!("  skipped {count:>4}: {reason}");
    }

    let baseline_path = root().join("xtask/baselines/xmlconf.txt");
    let baseline = read_baseline(&baseline_path)?;
    if write {
        write_baseline(&baseline_path, &failed)?;
        println!("wrote {} ({} failing tests)", baseline_path.display(), failed.len());
        return Ok(());
    }
    let regressions: Vec<&String> = failed.difference(&baseline).collect();
    let fixed: Vec<&String> = baseline.difference(&failed).collect();
    for path in &regressions {
        eprintln!("  NEW FAILURE {path}: {}", details.get(*path).map_or("", String::as_str));
    }
    for path in &fixed {
        eprintln!("  now passes  {path}: remove it from xtask/baselines/xmlconf.txt (or run with --write)");
    }
    if verbose {
        for path in failed.intersection(&baseline) {
            println!("  known       {path}: {}", details.get(path).map_or("", String::as_str));
        }
    }
    if !regressions.is_empty() || !fixed.is_empty() {
        bail!("{} new failure(s), {} fixed test(s) not yet removed from the baseline", regressions.len(), fixed.len());
    }
    println!("  baseline holds: {} known failures", baseline.len());
    Ok(())
}

/// The listed failures. A missing file is an empty baseline (every failure is then new); a file that cannot be read is an error, so the
/// ratchet is never checked against nothing by accident.
fn read_baseline(path: &Path) -> Result<BTreeSet<String>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    Ok(text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_owned).collect())
}

fn write_baseline(path: &Path, failed: &BTreeSet<String>) -> Result<()> {
    let mut text = String::from(
        "# xmlconf tests XmlCraft does not pass yet, one path per line (relative to the suite).\n\
         # A new failure fails `cargo xtask conformance xmlconf`; so does a test that now passes and is still listed.\n\
         # Regenerate with `cargo xtask conformance xmlconf --write`.\n",
    );
    for path in failed {
        text.push_str(path);
        text.push('\n');
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(files: &mut SuiteFiles<'_>, base: &Path, system: &str) -> Result<xmlcraft_dtd::Loaded, String> {
        xmlcraft_dtd::SubsetLoader::load(
            files,
            &xmlcraft_dtd::LoadRequest { kind: "external subset", public: None, system, base: &base.to_string_lossy() },
        )
    }

    #[test]
    fn only_files_of_the_suite_are_read_for_a_dtd() {
        for scheme in ["http://x/a.dtd", "file:/tmp/a.dtd", "FILE:a.dtd", "ftp:a", "x+y-z.1:a"] {
            assert!(has_scheme(scheme), "{scheme}");
        }
        for path in ["a.dtd", "dir/a.dtd", "../a.dtd", "a/b:c.dtd", ":a", "1x:a", ""] {
            assert!(!has_scheme(path), "{path}");
        }
        let dir = tempfile::tempdir().expect("dir");
        let suite = dir.path().join("suite");
        std::fs::create_dir_all(&suite).expect("suite");
        std::fs::write(suite.join("in.dtd"), "<!ELEMENT a EMPTY>").expect("writes");
        std::fs::write(dir.path().join("out.dtd"), "<!ELEMENT secret EMPTY>").expect("writes");
        let base = suite.join("doc.xml");
        let mut files = SuiteFiles { suite: &suite, rejected: None };
        assert_eq!(ask(&mut files, &base, "in.dtd").map(|l| l.text), Ok("<!ELEMENT a EMPTY>".to_owned()));
        for system in ["../out.dtd", "file:/etc/hosts", "/etc/hosts", "http://x/y.dtd", "missing.dtd"] {
            assert!(ask(&mut files, &base, system).is_err(), "{system}");
        }
        // A name that looks like a scheme is not a file of the suite, even when there is one by that name.
        #[cfg(unix)]
        {
            std::fs::write(suite.join("file:in.dtd"), "<!ELEMENT a EMPTY>").expect("writes");
            assert!(ask(&mut files, &base, "file:in.dtd").is_err());
        }
        // A link inside the suite to a file outside it is not followed out.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path().join("out.dtd"), suite.join("link.dtd")).expect("link");
            assert_eq!(ask(&mut files, &base, "link.dtd").map(|l| l.text), Err("outside the suite".to_owned()));
        }
        // A file that is not valid in its encoding is not text of the document: the document is rejected for it.
        std::fs::write(suite.join("latin.dtd"), b"<?xml encoding='UTF-8'?><!ELEMENT a EMPTY> \xE9").expect("writes");
        assert!(ask(&mut files, &base, "latin.dtd").is_err());
        assert!(files.rejected.as_deref().is_some_and(|why| why.contains("latin.dtd")), "{:?}", files.rejected);
    }

    #[test]
    fn entities_are_read_from_the_doctype() {
        let catalog = "<!DOCTYPE T SYSTEM \"t.dtd\" [\n <!ENTITY a\tSYSTEM \"x/a.xml\">\n <!ENTITY b SYSTEM 'b.xml'>\n <!ENTITY c \"text\">\n]>";
        let entities = system_entities(catalog);
        assert_eq!(entities.get("a").map(String::as_str), Some("x/a.xml"));
        assert_eq!(entities.get("b").map(String::as_str), Some("b.xml"));
        assert_eq!(entities.get("c"), None, "an internal entity is not a file");
    }

    #[test]
    fn the_catalog_expands_into_one_document_with_xml_base_inheritance() {
        let catalog = "<?xml version=\"1.0\"?><!DOCTYPE T [<!ENTITY a SYSTEM \"a.xml\"><!ENTITY b SYSTEM \"b.xml\">]>\n\
             <T><C xml:base=\"dir/\">&a;</C><C xml:base=\"other/\">&b;&missing;</C></T>";
        let read = |file: &str| match file {
            "a.xml" => Some("<?xml version=\"1.0\" encoding=\"UTF-8\"?><TEST TYPE=\"valid\" URI=\"x.xml\">one</TEST>".to_owned()),
            "b.xml" => Some("<C xml:base=\"sub/\"><TEST TYPE=\"not-wf\" URI=\"../y.xml\" EDITION=\"5\">two</TEST></C>".to_owned()),
            _ => None,
        };
        let expanded = expand_catalog(catalog, &read);
        assert!(!expanded.contains("<!DOCTYPE") && expanded.contains("&missing;"));
        let found = cases(&expanded);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].path.as_str(), found[0].kind.as_str()), ("dir/x.xml", "valid"));
        assert_eq!((found[1].path.as_str(), found[1].kind.as_str(), found[1].edition.as_deref()), ("other/y.xml", "not-wf", Some("5")));
    }

    #[test]
    fn a_unreadable_baseline_is_an_error_not_an_empty_one() {
        // A file that is not text: reading it fails with something other than "not found", on every platform (reading a directory does
        // not: Windows says different things for different kinds of directory).
        let path = std::env::temp_dir().join(format!("xmlcraft-baseline-{}.txt", std::process::id()));
        std::fs::write(&path, [0xFF, 0xFE, 0x00]).expect("writes");
        let read = read_baseline(&path);
        std::fs::remove_file(&path).expect("removes");
        assert!(read.is_err(), "{read:?}");
        // One that is not there is an empty baseline.
        assert_eq!(read_baseline(&path).expect("a missing file is empty"), BTreeSet::new());
    }

    #[test]
    fn a_test_element_is_relative_to_its_own_xml_base() {
        let expanded = "<T xml:base=\"a/\"><TEST xml:base=\"b/\" TYPE=\"valid\" URI=\"x.xml\"/></T>";
        let found = cases(expanded);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, "a/b/x.xml");
    }

    #[test]
    fn paths_are_joined_and_kept_inside_the_suite() {
        assert_eq!(join_path(&["a/".into(), "b/".into()], "c/d.xml").as_deref(), Some("a/b/c/d.xml"));
        assert_eq!(join_path(&["a/b/".into()], "../c.xml").as_deref(), Some("a/c.xml"));
        assert_eq!(join_path(&[], "./x.xml").as_deref(), Some("x.xml"));
        assert_eq!(join_path(&["a/".into()], "../../etc/passwd"), None);
        assert_eq!(join_path(&[], "../x"), None);
    }

    #[test]
    fn tests_that_do_not_apply_are_skipped_with_a_reason() {
        let case = |kind: &str, version: Option<&str>, edition: Option<&str>, recommendation: Option<&str>, namespace: Option<&str>| Case {
            path: "p".into(),
            kind: kind.into(),
            version: version.map(Into::into),
            edition: edition.map(Into::into),
            recommendation: recommendation.map(Into::into),
            namespace: namespace.map(Into::into),
            entities: None,
            sections: None,
        };
        assert_eq!(skip_reason(&case("valid", None, None, None, None)), None);
        assert_eq!(skip_reason(&case("not-wf", Some("1.0 1.1"), Some("5"), Some("XML1.0"), None)), None);
        assert!(skip_reason(&case("error", None, None, None, None)).is_some());
        assert!(skip_reason(&case("valid", Some("1.1"), None, None, None)).is_some());
        assert!(skip_reason(&case("valid", None, Some("1 2 3 4"), None, None)).is_some());
        assert!(skip_reason(&case("valid", None, None, Some("NS1.1"), None)).is_some());
        assert!(skip_reason(&case("valid", None, None, None, Some("no"))).is_some());
    }

    #[test]
    fn baselines_round_trip_and_ignore_comments() {
        let dir = std::env::temp_dir().join(format!("xmlcraft-baseline-test-{}", std::process::id()));
        let path = dir.join("b.txt");
        let failed: BTreeSet<String> = ["b/2.xml", "a/1.xml"].iter().map(|s| (*s).to_owned()).collect();
        write_baseline(&path, &failed).expect("writes");
        assert_eq!(read_baseline(&path).expect("reads"), failed);
        assert!(fs::read_to_string(&path).expect("reads").lines().filter(|l| !l.starts_with('#')).eq(["a/1.xml", "b/2.xml"]), "sorted");
        let _ = fs::remove_dir_all(&dir);
        assert!(read_baseline(&dir.join("missing.txt")).expect("a missing baseline is empty").is_empty());
    }

    #[test]
    fn verdicts_cover_encoding_and_syntax() {
        let verdict = |bytes: &[u8]| reject_reason(bytes, Path::new("test.xml"), Path::new("."));
        assert_eq!(verdict(b"<a/>"), None);
        assert!(verdict(b"<a>").is_some());
        assert!(verdict(b"<a>\xFF</a>").is_some(), "invalid UTF-8");
        assert!(verdict(b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a>\xE9</a>").is_none());
        assert!(verdict(b"\xEF\xBB\xBF<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a/>").is_some(), "the mark says UTF-8");
    }

    #[test]
    fn the_dtd_is_read_and_its_mistakes_reject_the_document() {
        let verdict = |text: &str| reject_reason(text.as_bytes(), Path::new("test.xml"), Path::new("."));
        assert_eq!(verdict("<!DOCTYPE a [<!ELEMENT a EMPTY>]><a/>"), None);
        let reason = verdict("<!DOCTYPE a [<!ELEMENT a>]><a/>").expect("a mistake in a declaration");
        assert!(reason.contains("content model"), "{reason}");
        assert!(verdict("<!DOCTYPE a [<!ATTLIST a b BOGUS #IMPLIED>]><a/>").is_some());
        // An external subset that cannot be read is not a mistake of the document.
        assert_eq!(verdict("<!DOCTYPE a SYSTEM \"missing.dtd\"><a/>"), None);
    }
}
