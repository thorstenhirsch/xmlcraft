//! `cargo xtask schemas`: the schemas that ship with XmlCraft (AGENTS.md §1.6).
//!
//! `schemas/manifest.toml` lists every bundled file with the URL it comes from, its SHA-256 and the day it was fetched. The default run
//! (part of `cargo xtask ci`) checks that:
//! - every listed file is there, byte for byte (SHA-256), and no schema file is there unlisted;
//! - each set's licence is on the closed schema allowlist and its licence file exists;
//! - there are at most [`MAX_FILES`] files;
//! - the schema entries of `ATTRIBUTION.toml` are the ones the manifest makes (they are generated, not written);
//! - every file the bundled catalogs point to is bundled.
//!
//! `--fetch` downloads the files that are missing (and checks them against the manifest); `--update [SET…]` fetches the entries
//! that have no hash yet, or every entry of the named sets again, and records what the publisher serves now.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use xmlcraft_catalog::{Catalog, Entry};

use crate::assets::{SCHEMA_DIR, SCHEMA_LICENCES, sha256_hex};
use crate::gates::root;

/// The most files the bundle may hold (docs/architecture.md).
pub const MAX_FILES: usize = 250;
/// The URI the bundled catalog is read from at run time; its relative entries are resolved against it.
const BUNDLE_URI: &str = "bundled:/";
const MANIFEST: &str = "schemas/manifest.toml";
const BEGIN: &str = "# BEGIN bundled schemas: generated from schemas/manifest.toml by `cargo xtask schemas --update`; do not edit by hand";
const END: &str = "# END bundled schemas";
/// File extensions of the files we redistribute, as in the assets gate.
const SCHEMA_EXTENSIONS: &[&str] = &["xsd", "dtd", "ent", "mod", "rng", "rnc", "sch", "wsdl", "xsl", "xslt", "json"];
/// The largest file `--fetch` and `--update` accept.
const MAX_DOWNLOAD: u64 = 16 * 1024 * 1024;

#[derive(Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(default)]
    pub set: Vec<Set>,
    #[serde(default)]
    pub file: Vec<File>,
}

/// Files that share an author and a licence.
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Set {
    pub name: String,
    pub title: String,
    pub author: String,
    pub licence: String,
    pub licence_file: String,
}

#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub set: String,
    /// Relative to `schemas/`; it starts with the name of the set.
    pub path: String,
    pub title: String,
    pub url: String,
    /// Empty until `--update` has fetched the file.
    pub sha256: String,
    /// The day the file was fetched (`YYYY-MM-DD`); empty until then.
    pub retrieved: String,
}

const HEADER: &str =
    "# The schemas that ship with XmlCraft (AGENTS.md section 1.6), one entry per file. `cargo xtask schemas` checks every file against it
# and `cargo xtask schemas --update` fills in what the publisher serves now (hash, date) and writes the schema entries of
# ATTRIBUTION.toml. The files are byte for byte the publisher's: a fix is a catalog rewrite, never an edit.
# Written by `cargo xtask schemas --update`; edit by adding an entry with an empty `sha256`, then run it.
";

fn quote(text: &str) -> String {
    toml::Value::String(text.to_owned()).to_string()
}

/// The manifest as it is written to `schemas/manifest.toml`.
pub fn render_manifest(m: &Manifest) -> String {
    let mut out = String::from(HEADER);
    for s in &m.set {
        out.push_str(&format!(
            "\n[[set]]\nname = {}\ntitle = {}\nauthor = {}\nlicence = {}\nlicence_file = {}\n",
            quote(&s.name),
            quote(&s.title),
            quote(&s.author),
            quote(&s.licence),
            quote(&s.licence_file)
        ));
    }
    for f in &m.file {
        out.push_str(&format!(
            "\n[[file]]\nset = {}\npath = {}\ntitle = {}\nurl = {}\nsha256 = {}\nretrieved = {}\n",
            quote(&f.set),
            quote(&f.path),
            quote(&f.title),
            quote(&f.url),
            quote(&f.sha256),
            quote(&f.retrieved)
        ));
    }
    out
}

/// The schema entries of `ATTRIBUTION.toml`, between the markers.
pub fn render_attribution(m: &Manifest) -> String {
    let mut out = format!("{BEGIN}\n");
    let mut files: Vec<&File> = m.file.iter().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    for f in files {
        let Some(set) = m.set.iter().find(|s| s.name == f.set) else { continue };
        out.push_str(&format!(
            "\n[[asset]]\npath = {}\ntitle = {}\nauthor = {}\nsource = {}\nlicence = {}\nlicence_file = {}\nkind = \"schema\"\nusage = {}\nsha256 = {}\n",
            quote(&format!("{SCHEMA_DIR}{}", f.path)),
            quote(&f.title),
            quote(&set.author),
            quote(&format!("{} (retrieved {})", f.url, f.retrieved)),
            quote(&set.licence),
            quote(&set.licence_file),
            quote("Bundled standard schema, found through the bundled catalog (schemas/catalog.xml) so that documents validate offline"),
            quote(&f.sha256)
        ));
    }
    out.push_str(END);
    out.push('\n');
    out
}

/// `ATTRIBUTION.toml` with its generated schema entries replaced (or appended when it has none yet).
pub fn with_attribution(existing: &str, m: &Manifest) -> String {
    let generated = render_attribution(m);
    match (existing.find(BEGIN), existing.find(END)) {
        (Some(from), Some(to)) if to > from => {
            let end = to + END.len();
            let end = if existing.get(end..).is_some_and(|rest| rest.starts_with('\n')) { end + 1 } else { end };
            format!("{}{}{}", &existing[..from], generated, &existing[end..])
        }
        _ => format!("{}\n{generated}", existing.trim_end_matches('\n')),
    }
}

/// The generated block as it stands in `ATTRIBUTION.toml`.
fn attribution_block(existing: &str) -> Option<&str> {
    let from = existing.find(BEGIN)?;
    let to = existing.find(END)?;
    let end = to + END.len();
    let end = if existing.get(end..).is_some_and(|rest| rest.starts_with('\n')) { end + 1 } else { end };
    existing.get(from..end)
}

fn is_schema_file(path: &str) -> bool {
    Path::new(path).extension().and_then(std::ffi::OsStr::to_str).is_some_and(|e| SCHEMA_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn is_own_file(path: &str) -> bool {
    path == "manifest.toml" || path.rsplit('/').next() == Some("catalog.xml")
}

fn is_hex_hash(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_date(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 10 && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

/// What is wrong with the bundle. `files` is every file under `schemas/` (path relative to it, content); `attribution` is the text of
/// `ATTRIBUTION.toml`.
pub fn problems(m: &Manifest, files: &BTreeMap<String, Vec<u8>>, attribution: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for s in &m.set {
        if !SCHEMA_LICENCES.contains(&s.licence.as_str()) {
            out.push(format!("set {}: the licence {} is not on the closed allowlist for bundled schemas (AGENTS.md §1.6)", s.name, s.licence));
        }
        if !files.contains_key(s.licence_file.strip_prefix(SCHEMA_DIR).unwrap_or(&s.licence_file)) {
            out.push(format!("set {}: licence file {} is missing", s.name, s.licence_file));
        }
    }
    for f in &m.file {
        if !seen.insert(f.path.as_str()) {
            out.push(format!("{}: listed twice", f.path));
        }
        if !m.set.iter().any(|s| s.name == f.set) {
            out.push(format!("{}: names the set {}, which is not declared", f.path, f.set));
        }
        if !f.path.starts_with(&format!("{}/", f.set)) || f.path.split('/').any(|part| part.is_empty() || part == "..") {
            out.push(format!("{}: the path must be inside the folder of its set ({}/) and plain", f.path, f.set));
        }
        if !(f.url.starts_with("https://") || f.url.starts_with("http://")) {
            out.push(format!("{}: the url must be http or https", f.path));
        }
        if f.sha256.is_empty() || f.retrieved.is_empty() {
            out.push(format!("{}: not fetched yet: run `cargo xtask schemas --update`", f.path));
        } else {
            if !is_hex_hash(&f.sha256) {
                out.push(format!("{}: sha256 is not 64 lower-case hex digits", f.path));
            }
            if !is_date(&f.retrieved) {
                out.push(format!("{}: retrieved must be a date (YYYY-MM-DD)", f.path));
            }
        }
        match files.get(&f.path) {
            None => out.push(format!("{}: listed but missing (`cargo xtask schemas --fetch`)", f.path)),
            Some(bytes) if !f.sha256.is_empty() && sha256_hex(bytes) != f.sha256 => {
                out.push(format!("{}: the file is not the one the manifest names (SHA-256 differs): files are never modified", f.path))
            }
            Some(_) => {}
        }
    }
    for path in files.keys() {
        if is_schema_file(path) && !is_own_file(path) && !seen.contains(path.as_str()) {
            out.push(format!("{SCHEMA_DIR}{path}: a schema file that is not in the manifest"));
        }
    }
    if m.file.len() > MAX_FILES {
        out.push(format!("{} files are listed, at most {MAX_FILES} may ship (docs/architecture.md)", m.file.len()));
    }
    let expected = render_attribution(m);
    if attribution_block(attribution) != Some(expected.as_str()) {
        out.push("the schema entries of ATTRIBUTION.toml are not the ones the manifest makes: run `cargo xtask schemas --update`".into());
    }
    out.extend(catalog_problems(files));
    out
}

/// Whether the catalogs that ship say anything that does not hold: a catalog that cannot be read, an entry that leads outside the
/// bundle or to a file it does not have.
pub fn catalog_problems(files: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    let mut out = Vec::new();
    let mut queue = vec!["catalog.xml".to_owned()];
    let mut done = BTreeSet::new();
    let exists = |path: &str| files.contains_key(path);
    let target = |name: &str, uri: &str, out: &mut Vec<String>| match uri.strip_prefix(BUNDLE_URI) {
        Some(path) if exists(path) => {}
        Some(path) => out.push(format!("{name}: leads to {path}, which is not bundled")),
        None => out.push(format!("{name}: leads to {uri}, outside the bundle")),
    };
    while let Some(path) = queue.pop() {
        if !done.insert(path.clone()) {
            continue;
        }
        let Some(bytes) = files.get(&path) else {
            out.push(format!("{path}: a catalog that is named is not bundled"));
            continue;
        };
        let location = format!("{BUNDLE_URI}{path}");
        let catalog = match String::from_utf8(bytes.clone())
            .map_err(|e| e.to_string())
            .and_then(|t| Catalog::parse(&t, &location).map_err(|e| e.to_string()))
        {
            Ok(c) => c,
            Err(why) => {
                out.push(format!("{path}: {why}"));
                continue;
            }
        };
        for warning in catalog.warnings() {
            out.push(format!("{path}: {warning}"));
        }
        for entry in catalog.entries() {
            match entry {
                Entry::Public { id, uri, .. } => target(&format!("{path}: public {id}"), uri, &mut out),
                Entry::System { id, uri } => target(&format!("{path}: system {id}"), uri, &mut out),
                Entry::SystemSuffix { suffix, uri } | Entry::UriSuffix { suffix, uri } => target(&format!("{path}: suffix {suffix}"), uri, &mut out),
                Entry::Uri { name, uri } => target(&format!("{path}: uri {name}"), uri, &mut out),
                Entry::RewriteSystem { start, prefix } | Entry::RewriteUri { start, prefix } => {
                    if !prefix.starts_with(BUNDLE_URI) || !files.keys().any(|f| format!("{BUNDLE_URI}{f}").starts_with(prefix.as_str())) {
                        out.push(format!("{path}: rewrite {start} leads to {prefix}, where nothing is bundled"));
                    }
                }
                Entry::NextCatalog { catalog }
                | Entry::DelegatePublic { catalog, .. }
                | Entry::DelegateSystem { catalog, .. }
                | Entry::DelegateUri { catalog, .. } => match catalog.strip_prefix(BUNDLE_URI) {
                    Some(next) => queue.push(next.to_owned()),
                    None => out.push(format!("{path}: names the catalog {catalog}, outside the bundle")),
                },
            }
        }
    }
    out
}

fn read_all(dir: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) -> Result<()> {
        let Ok(entries) = std::fs::read_dir(dir) else { return Ok(()) };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = format!("{prefix}{name}");
            if entry.file_type()?.is_dir() {
                walk(&entry.path(), &format!("{path}/"), out)?;
            } else {
                out.insert(path, std::fs::read(entry.path()).with_context(|| format!("reading {}", entry.path().display()))?);
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(dir, "", &mut out)?;
    Ok(out)
}

fn load_manifest(root: &Path) -> Result<Manifest> {
    let path = root.join(MANIFEST);
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Download `url` into `to` with curl (as the conformance suites are fetched). Returns the bytes.
fn download(url: &str, to: &Path) -> Result<Vec<u8>> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let status = Command::new("curl")
        .args(["-sS", "-L", "-f", "--max-time", "120", "--max-filesize", &MAX_DOWNLOAD.to_string(), "-o"])
        .arg(to)
        .arg(url)
        .status()
        .context("running curl (is it installed?)")?;
    if !status.success() {
        let _ = std::fs::remove_file(to);
        bail!("curl failed for {url} with {status}");
    }
    std::fs::read(to).with_context(|| format!("reading {}", to.display()))
}

pub fn run(args: &[String]) -> Result<()> {
    let root = root();
    let mut manifest = load_manifest(&root)?;
    let dir = root.join(SCHEMA_DIR);
    let flag = |name: &str| args.iter().any(|a| a == name);
    if flag("--fetch") {
        for f in &manifest.file {
            let to = dir.join(&f.path);
            if to.is_file() {
                continue;
            }
            let bytes = download(&f.url, &to)?;
            if !f.sha256.is_empty() && sha256_hex(&bytes) != f.sha256 {
                let _ = std::fs::remove_file(&to);
                bail!("{}: the publisher serves a different file now (SHA-256 differs); `cargo xtask schemas --update` records it", f.path);
            }
            println!("schemas: fetched {}", f.path);
        }
    }
    if let Some(at) = args.iter().position(|a| a == "--update") {
        let named: BTreeSet<&str> = args.iter().skip(at + 1).map(String::as_str).take_while(|a| !a.starts_with("--")).collect();
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        for f in &mut manifest.file {
            if !(f.sha256.is_empty() || named.contains(f.set.as_str())) {
                continue;
            }
            let bytes = download(&f.url, &dir.join(&f.path))?;
            let hash = sha256_hex(&bytes);
            if hash != f.sha256 {
                println!("schemas: {} {}", if f.sha256.is_empty() { "added" } else { "changed" }, f.path);
                f.sha256 = hash;
                f.retrieved = today.clone();
            }
        }
        std::fs::write(root.join(MANIFEST), render_manifest(&manifest))?;
        let attribution_path = root.join("ATTRIBUTION.toml");
        let existing = std::fs::read_to_string(&attribution_path).context("reading ATTRIBUTION.toml")?;
        std::fs::write(&attribution_path, with_attribution(&existing, &manifest))?;
        println!("schemas: wrote {MANIFEST} and the schema entries of ATTRIBUTION.toml; run `cargo xtask assets --write` for ATTRIBUTION.md");
    }
    let files = read_all(&dir)?;
    let attribution = std::fs::read_to_string(root.join("ATTRIBUTION.toml")).context("reading ATTRIBUTION.toml")?;
    let found = problems(&manifest, &files, &attribution);
    if found.is_empty() {
        println!("schemas: ok ({} files in {} set(s), at most {MAX_FILES})", manifest.file.len(), manifest.set.len());
        Ok(())
    } else {
        for p in &found {
            eprintln!("  ✗ {p}");
        }
        bail!("{} problem(s) with the bundled schemas; see AGENTS.md §1.6", found.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        let bytes = b"<!ELEMENT a EMPTY>";
        Manifest {
            set: vec![Set {
                name: "w3c".into(),
                title: "W3C".into(),
                author: "W3C".into(),
                licence: "W3C-20150513".into(),
                licence_file: "schemas/w3c/LICENSE.html".into(),
            }],
            file: vec![File {
                set: "w3c".into(),
                path: "w3c/a.dtd".into(),
                title: "A".into(),
                url: "https://example.org/a.dtd".into(),
                sha256: sha256_hex(bytes),
                retrieved: "2026-10-28".into(),
            }],
        }
    }

    const CATALOG: &str =
        "<catalog xmlns=\"urn:oasis:names:tc:entity:xmlns:xml:catalog\"><public publicId=\"-//X//A//EN\" uri=\"w3c/a.dtd\"/></catalog>";

    fn files() -> BTreeMap<String, Vec<u8>> {
        BTreeMap::from([
            ("w3c/a.dtd".to_owned(), b"<!ELEMENT a EMPTY>".to_vec()),
            ("w3c/LICENSE.html".to_owned(), b"licence".to_vec()),
            ("catalog.xml".to_owned(), CATALOG.as_bytes().to_vec()),
        ])
    }

    fn attribution(m: &Manifest) -> String {
        with_attribution("# header\n", m)
    }

    #[test]
    fn a_consistent_bundle_has_no_problems() {
        let m = manifest();
        assert_eq!(problems(&m, &files(), &attribution(&m)), Vec::<String>::new());
    }

    #[test]
    fn a_file_that_is_not_the_one_in_the_manifest_is_refused() {
        let m = manifest();
        let mut f = files();
        f.insert("w3c/a.dtd".into(), b"<!ELEMENT a ANY>".to_vec());
        assert!(problems(&m, &f, &attribution(&m)).iter().any(|p| p.contains("files are never modified")));
        f.remove("w3c/a.dtd");
        assert!(problems(&m, &f, &attribution(&m)).iter().any(|p| p.contains("listed but missing")));
    }

    #[test]
    fn a_schema_file_that_is_not_listed_is_refused_but_our_own_files_are_not_schemas() {
        let m = manifest();
        let mut f = files();
        f.insert("w3c/extra.xsd".into(), b"<x/>".to_vec());
        f.insert("w3c/catalog.xml".into(), CATALOG.as_bytes().to_vec());
        let p = problems(&m, &f, &attribution(&m));
        assert!(p.iter().any(|p| p.contains("w3c/extra.xsd") && p.contains("not in the manifest")), "{p:?}");
        assert!(!p.iter().any(|p| p.contains("w3c/catalog.xml") && p.contains("not in the manifest")), "{p:?}");
    }

    #[test]
    fn the_licence_must_be_on_the_closed_list() {
        let mut m = manifest();
        m.set[0].licence = "CC-BY-SA-4.0".into();
        let p = problems(&m, &files(), &attribution(&m));
        assert!(p.iter().any(|p| p.contains("not on the closed allowlist")), "{p:?}");
    }

    #[test]
    fn paths_stay_inside_their_set_and_are_plain() {
        for path in ["a.dtd", "other/a.dtd", "w3c/../a.dtd", "w3c//a.dtd"] {
            let mut m = manifest();
            m.file[0].path = path.into();
            let p = problems(&m, &files(), &attribution(&m));
            assert!(p.iter().any(|p| p.contains("inside the folder of its set")), "{path}: {p:?}");
        }
    }

    #[test]
    fn a_file_listed_twice_is_refused() {
        let mut m = manifest();
        m.file.push(m.file[0].clone());
        let p = problems(&m, &files(), &attribution(&m));
        assert!(p.iter().any(|p| p.contains("w3c/a.dtd: listed twice")), "{p:?}");
    }

    #[test]
    fn an_entry_without_a_hash_asks_for_an_update() {
        let mut m = manifest();
        m.file[0].sha256.clear();
        m.file[0].retrieved.clear();
        let p = problems(&m, &files(), &attribution(&m));
        assert!(p.iter().any(|p| p.contains("not fetched yet")), "{p:?}");
    }

    #[test]
    fn the_hash_and_the_date_have_a_shape() {
        let mut m = manifest();
        m.file[0].sha256 = "ABC".into();
        m.file[0].retrieved = "yesterday".into();
        let p = problems(&m, &files(), &attribution(&m));
        assert!(p.iter().any(|p| p.contains("64 lower-case hex")) && p.iter().any(|p| p.contains("YYYY-MM-DD")), "{p:?}");
    }

    #[test]
    fn at_most_the_limit_of_files_may_be_listed() {
        let mut m = manifest();
        let first = m.file[0].clone();
        m.file = (0..=MAX_FILES).map(|i| File { path: format!("w3c/f{i}.dtd"), ..first.clone() }).collect();
        let p = problems(&m, &files(), &attribution(&m));
        assert!(p.iter().any(|p| p.contains("at most 250 may ship")), "{p:?}");
        // Exactly the limit is allowed.
        m.file.pop();
        let p = problems(&m, &files(), &attribution(&m));
        assert!(!p.iter().any(|p| p.contains("may ship")), "{p:?}");
    }

    #[test]
    fn the_attribution_entries_are_the_ones_the_manifest_makes() {
        let m = manifest();
        let generated = attribution(&m);
        assert!(generated.contains("kind = \"schema\"") && generated.contains("schemas/w3c/a.dtd") && generated.contains(&m.file[0].sha256));
        // A hand edit is found, and regenerating replaces the block instead of adding to it.
        let edited = generated.replace("kind = \"schema\"", "kind = \"data\"");
        assert!(problems(&m, &files(), &edited).iter().any(|p| p.contains("ATTRIBUTION.toml")));
        assert_eq!(with_attribution(&generated, &m), generated);
        assert_eq!(with_attribution(&generated, &m).matches(BEGIN).count(), 1);
        // Text around the block is kept.
        let around = format!("# top\n{}# bottom\n", generated.trim_start_matches("# header\n"));
        assert_eq!(with_attribution(&around, &m), around);
    }

    #[test]
    fn the_catalogs_may_lead_only_to_what_is_bundled() {
        let mut f = files();
        assert!(catalog_problems(&f).is_empty());
        f.insert("catalog.xml".into(), CATALOG.replace("w3c/a.dtd", "w3c/missing.dtd").into_bytes());
        assert!(catalog_problems(&f).iter().any(|p| p.contains("w3c/missing.dtd") && p.contains("not bundled")));
        f.insert("catalog.xml".into(), CATALOG.replace("w3c/a.dtd", "http://example.org/a.dtd").into_bytes());
        assert!(catalog_problems(&f).iter().any(|p| p.contains("outside the bundle")));
        f.insert(
            "catalog.xml".into(),
            b"<catalog xmlns=\"urn:oasis:names:tc:entity:xmlns:xml:catalog\"><nextCatalog catalog=\"nowhere.xml\"/></catalog>".to_vec(),
        );
        assert!(catalog_problems(&f).iter().any(|p| p.contains("nowhere.xml") && p.contains("not bundled")));
        f.insert("catalog.xml".into(), b"<not-a-catalog/>".to_vec());
        assert!(!catalog_problems(&f).is_empty());
    }

    #[test]
    fn a_catalog_that_names_itself_ends() {
        let mut f = files();
        f.insert(
            "catalog.xml".into(),
            b"<catalog xmlns=\"urn:oasis:names:tc:entity:xmlns:xml:catalog\"><nextCatalog catalog=\"catalog.xml\"/></catalog>".to_vec(),
        );
        assert!(catalog_problems(&f).is_empty());
    }

    #[test]
    fn the_manifest_is_written_as_it_is_read() {
        let m = manifest();
        let text = render_manifest(&m);
        assert_eq!(toml::from_str::<Manifest>(&text).expect("parses"), m);
        assert_eq!(render_manifest(&toml::from_str::<Manifest>(&text).expect("parses")), text);
    }
}
