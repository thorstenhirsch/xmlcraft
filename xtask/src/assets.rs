//! `cargo xtask assets`: enforce the asset policy (AGENTS.md §1).
//!
//! `ATTRIBUTION.toml` lists every asset: committed or vendored files (`[[asset]]`) and files that Cargo dependencies compile
//! into XmlCraft (`[[bundled]]`). This gate fails when:
//! - an asset-like file in the repository has no entry, or its SHA-256 is missing or differs (translation catalogs are project
//!   files and carry none);
//! - a licence is not on the allowlist (a separate, stricter one for bundled schemas, AGENTS.md §1.6), or a declared licence
//!   file is missing;
//! - a visual asset, schema or data file names Altova or XMLSpy as author, source or title (AGENTS.md §1.1);
//! - a `[[bundled]]` crate version is not the one in Cargo.lock;
//! - `ATTRIBUTION.md` is out of date (`--write` regenerates it).

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::gates::root;

/// Licences allowed for assets by AGENTS.md §1.2 (SPDX identifiers).
pub const ALLOWED_LICENCES: &[&str] = &[
    "ISC",
    "MIT",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "Apache-2.0",
    "OFL-1.1",
    "Ubuntu-font-1.0",
    "Bitstream-Vera",
    "CC0-1.0",
    "CC-BY-4.0",
    "CC-BY-SA-4.0",
    "LicenseRef-Public-Domain",
];

/// The closed licence allowlist for bundled schemas (`kind = "schema"`, AGENTS.md §1.6). It is deliberately narrower than
/// [`ALLOWED_LICENCES`] in what it adds and in what it leaves out: no CC-BY-SA, EPL or Ecma terms without the owner's approval.
pub const SCHEMA_LICENCES: &[&str] = &[
    "W3C-20150513",
    "LicenseRef-OASIS-Verbatim",
    "LicenseRef-DocBook",
    "Apache-2.0",
    "MIT",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "CC0-1.0",
    "LicenseRef-Unicode",
];

/// Kind of a bundled schema entry.
pub const SCHEMA_KIND: &str = "schema";

/// Where the UI translation catalogs live. `kind = "translation"` entries for `.tsv` files here are project files under the
/// project licence: they carry no `sha256`, because git history already records their provenance (AGENTS.md §1.3).
pub const TRANSLATION_DIR: &str = "crates/ui-egui/src/i18n/";

/// Directory holding the bundled schemas (AGENTS.md §1.6).
pub const SCHEMA_DIR: &str = "schemas/";

/// File extensions that count as assets wherever they appear in the repository.
const ASSET_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "svg", "ico", "icns", "webp", "bmp", "tif", "tiff", "avif", "heic", "ttf", "otf", "ttc", "woff", "woff2", "pfb",
    "pfa", "afm", "icc", "icm", "pdf", "eps", "ps", "ai", "psd", "mp3", "wav", "ogg", "flac", "mp4", "mov", "webm", "cur", "ani", "brotli", "tsv",
];

/// Extensions of schema files that are assets when they sit under [`SCHEMA_DIR`]. (Test fixtures elsewhere are contributor-written
/// project files, not assets.)
const SCHEMA_EXTENSIONS: &[&str] = &["xsd", "dtd", "ent", "mod", "rng", "rnc", "sch", "wsdl", "xsl", "xslt", "json"];

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(default)]
    pub asset: Vec<Asset>,
    #[serde(default)]
    pub bundled: Vec<Bundled>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub path: String,
    pub title: String,
    pub author: String,
    pub source: String,
    pub licence: String,
    pub licence_file: String,
    pub kind: String,
    pub usage: String,
    /// Required for every entry except a translation catalog ([`Asset::is_translation_catalog`]).
    #[serde(default)]
    pub sha256: Option<String>,
}

impl Asset {
    /// A UI translation catalog: a project file whose entry carries no hash.
    pub fn is_translation_catalog(&self) -> bool {
        self.kind == "translation" && self.path.starts_with(TRANSLATION_DIR) && self.path.ends_with(".tsv")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundled {
    #[serde(rename = "crate")]
    pub krate: String,
    pub version: String,
    pub path: String,
    pub title: String,
    pub author: String,
    pub source: String,
    pub licence: String,
    pub kind: String,
    pub usage: String,
    pub sha256: String,
}

pub fn load(root: &Path) -> Result<Manifest> {
    let path = root.join("ATTRIBUTION.toml");
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// `cargo xtask assets [--write]`.
pub fn run(args: &[String]) -> Result<()> {
    let root = root();
    let manifest = load(&root)?;
    let markdown = render_markdown(&manifest);
    if args.iter().any(|a| a == "--write") {
        std::fs::write(root.join("ATTRIBUTION.md"), &markdown)?;
        println!("assets: wrote ATTRIBUTION.md");
    }
    for a in manifest.asset.iter().filter(|a| a.is_translation_catalog() && a.sha256.is_some()) {
        println!("assets: note: {}: translation catalogs aren't hashed; its sha256 line can be deleted", a.path);
    }
    let mut problems = check(&root, &manifest, &repo_files(&root)?, &cargo_lock_versions(&root)?);
    match std::fs::read_to_string(root.join("ATTRIBUTION.md")) {
        Ok(existing) if existing == markdown => {}
        _ => problems.push("ATTRIBUTION.md is out of date: run `cargo xtask assets --write`".into()),
    }
    if problems.is_empty() {
        println!("assets: ok ({} repository assets, {} bundled by dependencies)", manifest.asset.len(), manifest.bundled.len());
        Ok(())
    } else {
        for p in &problems {
            eprintln!("  ✗ {p}");
        }
        bail!("{} asset policy violation(s); see AGENTS.md §1", problems.len())
    }
}

/// Files in the working tree that git tracks or would track (respects .gitignore).
fn repo_files(root: &Path) -> Result<Vec<String>> {
    let out = Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard", "-z"])
        .current_dir(root)
        .output()
        .context("running git ls-files")?;
    if !out.status.success() {
        bail!("git ls-files failed");
    }
    Ok(out.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect())
}

fn cargo_lock_versions(root: &Path) -> Result<BTreeSet<(String, String)>> {
    let text = std::fs::read_to_string(root.join("Cargo.lock")).context("reading Cargo.lock")?;
    let mut out = BTreeSet::new();
    let mut name = None;
    for line in text.lines() {
        if let Some(n) = line.strip_prefix("name = \"") {
            name = Some(n.trim_end_matches('"').to_string());
        } else if let (Some(v), Some(n)) = (line.strip_prefix("version = \""), name.take()) {
            out.insert((n, v.trim_end_matches('"').to_string()));
        }
    }
    Ok(out)
}

/// Whether a repository file is an asset that needs an `ATTRIBUTION.toml` entry.
pub fn is_asset_path(path: &str) -> bool {
    let ext = Path::new(path).extension().and_then(std::ffi::OsStr::to_str).map(str::to_ascii_lowercase);
    let Some(ext) = ext else { return false };
    ASSET_EXTENSIONS.contains(&ext.as_str())
        || (path.starts_with(SCHEMA_DIR) && SCHEMA_EXTENSIONS.contains(&ext.as_str()) && !is_own_schema_file(path))
}

/// Files under `schemas/` that we write ourselves (catalogs and the manifest) rather than redistribute.
fn is_own_schema_file(path: &str) -> bool {
    path == "schemas/manifest.toml" || path.rsplit('/').next() == Some("catalog.xml")
}

/// Every licence id in an SPDX expression (`MIT OR Apache-2.0`, `MIT AND Bitstream-Vera`).
fn licence_ids(expr: &str) -> impl Iterator<Item = &str> {
    expr.split(|c: char| c.is_whitespace() || c == '(' || c == ')').filter(|t| !t.is_empty() && *t != "OR" && *t != "AND" && *t != "WITH")
}

/// Altova and its products (AGENTS.md §1.1).
fn mentions_altova(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    s.contains("altova") || s.contains("xmlspy")
}

/// Kinds that may never come from Altova: visual design, schemas, data, and UI translations (which must be clean-room, never taken
/// from a product's string tables).
fn altova_forbidden(kind: &str) -> bool {
    matches!(kind, "icon" | "image" | "font" | "logo" | "cursor" | "video" | "translation" | SCHEMA_KIND | "data" | "document")
}

/// The licence allowlist that applies to an asset of `kind`.
fn allowed_for(kind: &str) -> &'static [&'static str] {
    if kind == SCHEMA_KIND { SCHEMA_LICENCES } else { ALLOWED_LICENCES }
}

/// All policy checks that don't depend on the filesystem layout beyond `root` (unit-testable).
pub fn check(root: &Path, m: &Manifest, repo_files: &[String], lock: &BTreeSet<(String, String)>) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    let licence_ok = |what: &str, kind: &str, licence: &str, problems: &mut Vec<String>| {
        let allowed = allowed_for(kind);
        let mut any = false;
        for id in licence_ids(licence) {
            any = true;
            if !allowed.contains(&id) {
                let rule = if kind == SCHEMA_KIND { "§1.6" } else { "§1.2" };
                problems.push(format!("{what}: licence `{id}` is not on the allowlist for {kind} (AGENTS.md {rule})"));
            }
        }
        if !any {
            problems.push(format!("{what}: no licence given"));
        }
    };
    for a in &m.asset {
        if !seen.insert(a.path.clone()) {
            problems.push(format!("{}: listed twice", a.path));
        }
        licence_ok(&a.path, &a.kind, &a.licence, &mut problems);
        if !root.join(&a.licence_file).is_file() {
            problems.push(format!("{}: licence file {} is missing", a.path, a.licence_file));
        }
        if a.kind == SCHEMA_KIND && !a.path.starts_with(SCHEMA_DIR) {
            problems.push(format!("{}: a schema asset must live under {SCHEMA_DIR} (AGENTS.md §1.6)", a.path));
        }
        if a.path.starts_with(SCHEMA_DIR) && is_asset_path(&a.path) && a.kind != SCHEMA_KIND {
            problems.push(format!("{}: files under {SCHEMA_DIR} must be kind = \"{SCHEMA_KIND}\" (AGENTS.md §1.6)", a.path));
        }
        if altova_forbidden(&a.kind) && (mentions_altova(&a.author) || mentions_altova(&a.source) || mentions_altova(&a.title)) {
            problems.push(format!("{}: {} from Altova is forbidden (AGENTS.md §1.1)", a.path, a.kind));
        }
        let catalog = a.is_translation_catalog();
        match (std::fs::read(root.join(&a.path)), &a.sha256) {
            (Err(_), _) => problems.push(format!("{}: listed but missing", a.path)),
            // A leftover hash line (e.g. kept while resolving a rebase) is not checked; `run` notes it.
            (Ok(_), _) if catalog => {}
            (Ok(_), None) => problems.push(format!("{}: sha256 is missing (only translation catalogs go without one)", a.path)),
            (Ok(bytes), Some(hash)) if sha256_hex(&bytes) == *hash => {}
            (Ok(_), Some(_)) => {
                problems.push(format!("{}: SHA-256 does not match ATTRIBUTION.toml (re-verify provenance, then update the entry)", a.path))
            }
        }
    }
    for f in repo_files {
        if is_asset_path(f) && !seen.contains(f) && root.join(f).is_file() {
            problems.push(format!("{f}: asset has no ATTRIBUTION.toml entry"));
        }
    }
    for b in &m.bundled {
        let what = format!("{}@{} {}", b.krate, b.version, b.path);
        licence_ok(&what, &b.kind, &b.licence, &mut problems);
        if !lock.contains(&(b.krate.clone(), b.version.clone())) {
            problems.push(format!("{what}: crate version not in Cargo.lock (dependency changed: re-audit its bundled assets)"));
        }
        if altova_forbidden(&b.kind) && (mentions_altova(&b.author) || mentions_altova(&b.source) || mentions_altova(&b.title)) {
            problems.push(format!("{what}: {} from Altova is forbidden (AGENTS.md §1.1)", b.kind));
        }
        if b.sha256.len() != 64 {
            problems.push(format!("{what}: sha256 must be 64 hex digits"));
        }
    }
    problems
}

/// ATTRIBUTION.md, generated from the manifest.
pub fn render_markdown(m: &Manifest) -> String {
    let mut s = String::new();
    s.push_str("# Attribution\n\n");
    s.push_str("<!-- Generated from ATTRIBUTION.toml by `cargo xtask assets --write`. Do not edit by hand. -->\n\n");
    s.push_str("Every asset XmlCraft includes, bundles or uses to build its published material, with its author, source and licence. ");
    s.push_str(
        "The policy is in [AGENTS.md](AGENTS.md) §1. The machine-readable list, with the SHA-256 of every asset but the translation catalogs, is [ATTRIBUTION.toml](ATTRIBUTION.toml). ",
    );
    s.push_str("Licence texts are kept beside the assets and summarised in [NOTICE](NOTICE).\n\n");
    let esc = |t: &str| t.replace('|', "\\|");
    s.push_str(&format!(
        "## In this repository ({})\n\n| Asset | Title | Author | Licence | Source | Used for |\n|---|---|---|---|---|---|\n",
        m.asset.len()
    ));
    for a in &m.asset {
        let _ = writeln!(s, "| `{}` | {} | {} | {} | {} | {} |", a.path, esc(&a.title), esc(&a.author), a.licence, esc(&a.source), esc(&a.usage));
    }
    s.push_str(&format!(
        "\n## Compiled in through dependencies ({})\n\n| Crate | File | Title | Author | Licence | Source | Used for |\n|---|---|---|---|---|---|---|\n",
        m.bundled.len()
    ));
    for b in &m.bundled {
        let _ = writeln!(
            s,
            "| `{}` {} | `{}` | {} | {} | {} | {} | {} |",
            b.krate,
            b.version,
            b.path,
            esc(&b.title),
            esc(&b.author),
            b.licence,
            esc(&b.source),
            esc(&b.usage)
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(path: &str, author: &str, kind: &str, licence: &str) -> Asset {
        Asset {
            path: path.into(),
            title: "t".into(),
            author: author.into(),
            source: "s".into(),
            licence: licence.into(),
            licence_file: "Cargo.toml".into(),
            kind: kind.into(),
            usage: "u".into(),
            sha256: Some(sha256_hex(&std::fs::read(root().join(path)).unwrap_or_default())),
        }
    }

    fn bundled(krate: &str, path: &str, author: &str, kind: &str, licence: &str) -> Bundled {
        Bundled {
            krate: krate.into(),
            version: "1.0.0".into(),
            path: path.into(),
            title: "t".into(),
            author: author.into(),
            source: "s".into(),
            licence: licence.into(),
            kind: kind.into(),
            usage: "u".into(),
            sha256: "0".repeat(64),
        }
    }

    fn lock(entries: &[(&str, &str)]) -> BTreeSet<(String, String)> {
        entries.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    fn check_assets(assets: Vec<Asset>) -> Vec<String> {
        check(&root(), &Manifest { asset: assets, ..Default::default() }, &[], &lock(&[]))
    }

    #[test]
    fn the_real_manifest_passes() {
        let root = root();
        let m = load(&root).unwrap();
        let problems = check(&root, &m, &repo_files(&root).unwrap(), &cargo_lock_versions(&root).unwrap());
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn unlisted_asset_is_rejected() {
        let p = check(&root(), &Manifest::default(), &["assets/icons/x.svg".into()], &lock(&[]));
        assert!(p.iter().any(|p| p.contains("no ATTRIBUTION.toml entry")), "{p:?}");
    }

    #[test]
    fn schema_files_under_schemas_are_assets_but_our_own_catalogs_are_not() {
        for yes in ["schemas/w3c/XMLSchema.xsd", "schemas/oasis/docbook/docbook.dtd", "schemas/w3c/xhtml/xhtml-lat1.ent", "schemas/json/x.json"] {
            assert!(is_asset_path(yes), "{yes}");
        }
        for no in ["schemas/manifest.toml", "schemas/catalog.xml", "schemas/w3c/catalog.xml", "crates/validate/tests/fixtures/a.xsd", "docs/x.dtd"] {
            assert!(!is_asset_path(no), "{no}");
        }
    }

    #[test]
    fn schemas_have_their_own_licence_allowlist() {
        // Allowed for assets in general, but not for a bundled schema without the owner's approval (AGENTS.md §1.6).
        for licence in ["CC-BY-SA-4.0", "OFL-1.1", "EPL-2.0", "ISC"] {
            let p = check_assets(vec![asset("schemas/x.xsd", "someone", "schema", licence)]);
            assert!(p.iter().any(|p| p.contains("not on the allowlist for schema")), "{licence}: {p:?}");
        }
        for licence in ["W3C-20150513", "LicenseRef-OASIS-Verbatim", "LicenseRef-DocBook", "Apache-2.0", "MIT"] {
            let p = check_assets(vec![asset("schemas/x.xsd", "someone", "schema", licence)]);
            assert!(p.iter().all(|p| !p.contains("allowlist")), "{licence}: {p:?}");
        }
        // And the schema-only licences are not available to other kinds.
        let p = check_assets(vec![asset("assets/icons/x.svg", "someone", "icon", "W3C-20150513")]);
        assert!(p.iter().any(|p| p.contains("not on the allowlist for icon")), "{p:?}");
    }

    #[test]
    fn schemas_must_live_under_the_schema_directory_and_be_marked_as_such() {
        let p = check_assets(vec![asset("assets/x.xsd", "someone", "schema", "MIT")]);
        assert!(p.iter().any(|p| p.contains("must live under schemas/")), "{p:?}");
        let p = check_assets(vec![asset("schemas/x.xsd", "someone", "data", "MIT")]);
        assert!(p.iter().any(|p| p.contains("must be kind = \"schema\"")), "{p:?}");
    }

    #[test]
    fn altova_material_is_rejected_whatever_its_licence() {
        for (kind, author) in [("icon", "Altova GmbH"), ("image", "XMLSpy team"), ("font", "altova"), ("schema", "Altova GmbH"), ("data", "Altova")] {
            let p = check_assets(vec![asset("assets/icons/x.svg", author, kind, "MIT")]);
            assert!(p.iter().any(|p| p.contains("from Altova is forbidden")), "{kind}: {p:?}");
        }
        // The source and the title count too.
        let mut a = asset("assets/icons/x.svg", "someone", "icon", "MIT");
        a.source = "https://www.altova.com/x".into();
        assert!(check_assets(vec![a]).iter().any(|p| p.contains("from Altova is forbidden")));
        let mut a = asset("assets/icons/x.svg", "someone", "icon", "MIT");
        a.title = "XMLSpy toolbar icon".into();
        assert!(check_assets(vec![a]).iter().any(|p| p.contains("from Altova is forbidden")));
        // A bundled asset is held to the same rule.
        let m = Manifest { bundled: vec![bundled("c", "x.ttf", "Altova", "font", "OFL-1.1")], ..Default::default() };
        assert!(check(&root(), &m, &[], &lock(&[("c", "1.0.0")])).iter().any(|p| p.contains("from Altova is forbidden")));
    }

    #[test]
    fn disallowed_licences_are_rejected() {
        let p = check_assets(vec![asset("assets/icons/x.svg", "Someone", "icon", "LicenseRef-Proprietary OR MIT")]);
        assert!(p.iter().any(|p| p.contains("not on the allowlist")), "{p:?}");
        let p = check_assets(vec![asset("assets/icons/x.svg", "Someone", "icon", "")]);
        assert!(p.iter().any(|p| p.contains("no licence given")), "{p:?}");
    }

    #[test]
    fn changed_file_fails_hash_check() {
        let mut a = asset("assets/icons/x.svg", "Lucide", "icon", "ISC");
        a.sha256 = Some("0".repeat(64));
        let p = check_assets(vec![a]);
        assert!(p.iter().any(|p| p.contains("SHA-256 does not match")), "{p:?}");
    }

    /// One `[[asset]]` entry as ATTRIBUTION.toml spells it, with `extra` lines appended.
    fn parsed(path: &str, kind: &str, extra: &str) -> Manifest {
        let text = format!(
            "[[asset]]\npath = \"{path}\"\ntitle = \"t\"\nauthor = \"a\"\nsource = \"s\"\nlicence = \"MIT\"\nlicence_file = \"Cargo.toml\"\nkind = \"{kind}\"\nusage = \"u\"\n{extra}"
        );
        toml::from_str(&text).unwrap()
    }

    /// Catalogs change in most UI PRs; a pinned hash would make every pair of them conflict.
    #[test]
    fn translation_catalogs_carry_no_hash_and_everything_else_does() {
        let check_one = |m: Manifest| check(&root(), &m, &[], &lock(&[]));
        let ja = "crates/ui-egui/src/i18n/ja.tsv";
        // The catalog file does not exist yet, so "listed but missing" is the only possible complaint for it.
        let p = check_one(parsed(ja, "translation", ""));
        assert!(p.iter().all(|p| !p.contains("sha256")), "{p:?}");
        for (path, kind) in [("assets/fonts/Inter-Regular.ttf", "font"), (ja, "data"), ("Cargo.toml", "translation")] {
            let p = check_one(parsed(path, kind, ""));
            assert!(p.iter().any(|p| p.contains("sha256 is missing") || p.contains("listed but missing")), "{path} {kind}: {p:?}");
        }
        assert!(check_one(parsed("assets/fonts/Inter-Regular.ttf", "font", "")).iter().any(|p| p.contains("sha256 is missing")));
    }

    #[test]
    fn dependency_upgrade_forces_reaudit() {
        let m = Manifest { bundled: vec![bundled("other", "a.ttf", "Someone", "font", "OFL-1.1")], ..Default::default() };
        assert!(check(&root(), &m, &[], &lock(&[("other", "2.0.0")])).iter().any(|p| p.contains("not in Cargo.lock")));
        assert!(check(&root(), &m, &[], &lock(&[("other", "1.0.0")])).is_empty());
    }

    #[test]
    fn unknown_fields_in_the_manifest_are_rejected() {
        let text = "[[asset]]\npath = \"a\"\ntitle = \"t\"\nauthor = \"a\"\nsource = \"s\"\nlicence = \"MIT\"\nlicence_file = \"Cargo.toml\"\nkind = \"icon\"\nusage = \"u\"\nadobe_data = true\n";
        assert!(toml::from_str::<Manifest>(text).is_err());
    }

    #[test]
    fn spdx_expressions_split() {
        assert_eq!(licence_ids("(MIT OR Apache-2.0) AND OFL-1.1").collect::<Vec<_>>(), ["MIT", "Apache-2.0", "OFL-1.1"]);
    }
}
