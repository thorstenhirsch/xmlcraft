//! SchemaStore: finding the JSON Schema for a file by its name. The catalog (`catalog.json` of schemastore.org, a list of schemas each
//! with the file name patterns it is for) is fetched when a person asks (it is a document like any the resolver fetches, so the
//! switch to work offline and the download cache apply), and nothing of it is bundled; this module reads it and says which entries a
//! file belongs to.

use xmlcraft_json_syntax::{Data, Dialect, Options, parse};

/// Where the catalog is.
pub const CATALOG_URL: &str = "https://www.schemastore.org/api/json/catalog.json";
/// The most entries read from a catalog.
pub const MAX_ENTRIES: usize = 20_000;
/// The longest pattern or path looked at.
const MAX_LEN: usize = 1024;

/// One schema of the catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub description: String,
    /// Where the schema is.
    pub url: String,
    /// The file names it is for (`*.eslintrc.json`, `**/.github/workflows/*.yml`).
    pub patterns: Vec<String>,
}

/// Read the text of a catalog.
pub fn parse_catalog(text: &str) -> Result<Vec<Entry>, String> {
    let read = parse(text, &Options { dialect: Dialect::Jsonc, ..Options::default() });
    let root = read.root.ok_or_else(|| "the catalog is not JSON".to_owned())?;
    let Some(Data::Array(schemas)) = root.get("schemas").map(|v| &v.data) else {
        return Err("this is not a SchemaStore catalog: it has no `schemas` list".to_owned());
    };
    let text_of = |v: &xmlcraft_json_syntax::Value, key: &str| match v.get(key).map(|x| &x.data) {
        Some(Data::String(s)) => s.clone(),
        _ => String::new(),
    };
    let mut entries = Vec::new();
    for item in schemas.iter().take(MAX_ENTRIES) {
        let url = text_of(item, "url");
        let patterns: Vec<String> = match item.get("fileMatch").map(|v| &v.data) {
            Some(Data::Array(list)) => list.iter().filter_map(|p| if let Data::String(s) = &p.data { Some(s.clone()) } else { None }).collect(),
            _ => Vec::new(),
        };
        if url.is_empty() || patterns.is_empty() {
            continue;
        }
        entries.push(Entry { name: text_of(item, "name"), description: text_of(item, "description"), url, patterns });
    }
    Ok(entries)
}

/// One piece of a glob.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Piece {
    Literal(char),
    /// `?`: one character but `/`.
    One,
    /// `*`: any run of characters but `/`.
    Star,
    /// `**`: any run of characters.
    Deep,
    /// `**/`: any folders, or none.
    DeepSlash,
}

fn pieces(pattern: &str) -> Vec<Piece> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(c) = chars.get(i) {
        match c {
            '*' if chars.get(i + 1) == Some(&'*') => {
                if chars.get(i + 2) == Some(&'/') {
                    out.push(Piece::DeepSlash);
                    i += 3;
                } else {
                    out.push(Piece::Deep);
                    i += 2;
                }
            }
            '*' => {
                out.push(Piece::Star);
                i += 1;
            }
            '?' => {
                out.push(Piece::One);
                i += 1;
            }
            c => {
                out.push(Piece::Literal(*c));
                i += 1;
            }
        }
    }
    out
}

/// Whether `text` matches the glob `pattern`: `*` is any run of characters but `/`, `**` any run, `**/` any folders (or none), `?` one
/// character but `/`. (Worked out over every place in the pattern and the text, so no pattern makes it run long.)
pub fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern.len() > MAX_LEN || text.len() > MAX_LEN {
        return false;
    }
    let p = pieces(pattern);
    let t: Vec<char> = text.chars().collect();
    let (pn, tn) = (p.len(), t.len());
    // `ok[i][j]`: the pieces from `i` match the text from `j`.
    let mut ok = vec![vec![false; tn + 1]; pn + 1];
    ok[pn][tn] = true;
    for i in (0..pn).rev() {
        for j in (0..=tn).rev() {
            let here = t.get(j).copied();
            let value = match p[i] {
                Piece::Literal(c) => here == Some(c) && ok[i + 1][j + 1],
                Piece::One => here.is_some_and(|h| h != '/') && ok[i + 1][j + 1],
                Piece::Star => ok[i + 1][j] || here.is_some_and(|h| h != '/') && ok[i][j + 1],
                Piece::Deep => ok[i + 1][j] || here.is_some() && ok[i][j + 1],
                Piece::DeepSlash => ok[i + 1][j] || here.is_some() && ((here == Some('/') && ok[i + 1][j + 1]) || ok[i][j + 1]),
            };
            ok[i][j] = value;
        }
    }
    ok[0][0]
}

/// Whether the pattern of a catalog entry is for the file at `path` (with `/` or `\` between its folders). A pattern with no `/` is for
/// the file's name; one with a `/` is for the end of its path (`**/` before it is implied, so `.github/workflows/*.yml` finds the files of
/// any repository).
pub fn pattern_matches(pattern: &str, path: &str) -> bool {
    let path = path.replace('\\', "/");
    let pattern = pattern.trim_start_matches("./");
    if !pattern.contains('/') {
        let name = path.rsplit('/').next().unwrap_or(&path);
        return glob_match(pattern, name);
    }
    let pattern = pattern.trim_start_matches('/');
    let anchored = if pattern.starts_with("**/") { pattern.to_owned() } else { format!("**/{pattern}") };
    glob_match(&anchored, &path) || glob_match(pattern, &path)
}

/// The entries that are for the file at `path`, those whose pattern is a better fit (a longer, more specific one) first.
pub fn matching<'a>(entries: &'a [Entry], path: &str) -> Vec<&'a Entry> {
    let mut found: Vec<(usize, &Entry)> = entries
        .iter()
        .filter_map(|e| e.patterns.iter().filter(|p| pattern_matches(p, path)).map(|p| p.len()).max().map(|score| (score, e)))
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
    found.into_iter().map(|(_, e)| e).collect()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    const CATALOG: &str = r#"{
      "$schema": "https://json.schemastore.org/schema-store.json", "version": 1,
      "schemas": [
        {"name": "package.json", "description": "NPM", "fileMatch": ["package.json"], "url": "https://json.schemastore.org/package.json"},
        {"name": "GitHub Workflow", "description": "CI", "fileMatch": [".github/workflows/*.yml", ".github/workflows/*.yaml"], "url": "https://json.schemastore.org/github-workflow.json"},
        {"name": "tsconfig", "fileMatch": ["tsconfig.json", "tsconfig.*.json"], "url": "https://json.schemastore.org/tsconfig.json"},
        {"name": "Any JSON", "fileMatch": ["*.json"], "url": "https://example.com/any.json"},
        {"name": "no files", "url": "https://example.com/none.json"},
        {"name": "no url", "fileMatch": ["x.json"]}
      ]
    }"#;

    #[test]
    fn the_catalog_gives_each_schema_its_url_and_the_files_it_is_for_and_leaves_out_the_rest() {
        let entries = parse_catalog(CATALOG).expect("reads");
        assert_eq!(entries.len(), 4, "an entry with no files or no url is not one");
        assert_eq!(entries[1].patterns, [".github/workflows/*.yml", ".github/workflows/*.yaml"]);
        assert!(parse_catalog("{}").is_err() && parse_catalog("nope").is_err());
    }

    #[test]
    fn globs_match_names_and_paths_the_way_a_catalog_means() {
        for (p, t, ok) in [
            ("*.json", "a.json", true),
            ("*.json", "dir/a.json", false),
            ("**/*.json", "dir/sub/a.json", true),
            ("**/*.json", "a.json", true),
            ("tsconfig.*.json", "tsconfig.build.json", true),
            ("tsconfig.*.json", "tsconfig.json", false),
            ("a?c", "abc", true),
            ("a?c", "a/c", false),
            ("**", "x/y/z", true),
            ("a/**/z", "a/z", true),
            ("a/**/z", "a/b/z", true),
            ("a/**/z", "ab/z", false),
            ("a/**/z", "a/b/c/z", true),
            ("*", "", true),
            ("", "", true),
            ("", "a", false),
        ] {
            assert_eq!(glob_match(p, t), ok, "{p} against {t}");
        }
        assert!(!glob_match(&"*".repeat(2000), "a") && !glob_match("a", &"a".repeat(2000)), "long ones are not matched");
    }

    #[test]
    fn a_file_is_matched_by_its_name_or_by_the_end_of_its_path_and_the_better_fit_comes_first() {
        let entries = parse_catalog(CATALOG).expect("reads");
        let names = |p: &str| matching(&entries, p).iter().map(|e| e.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names("/home/me/app/package.json"), ["package.json", "Any JSON"]);
        assert_eq!(names("C:\\work\\repo\\.github\\workflows\\ci.yml"), ["GitHub Workflow"]);
        assert_eq!(names("tsconfig.build.json"), ["tsconfig", "Any JSON"]);
        assert_eq!(names("/x/other.yaml"), Vec::<&str>::new());
        assert_eq!(names(""), Vec::<&str>::new());
        assert!(pattern_matches("/.github/workflows/*.yml", "r/.github/workflows/a.yml"));
    }

    #[test]
    fn no_pattern_or_path_makes_the_matcher_run_away() {
        let evil = "*a".repeat(40) + "b";
        let text = "a".repeat(900);
        let started = std::time::Instant::now();
        let _ = glob_match(&evil, &text);
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    }
}
