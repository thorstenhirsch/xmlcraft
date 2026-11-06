//! Find across the files of a folder: the files under it that a name pattern admits are read (the same size limit and decoding as
//! opening one; a file that is not text is skipped) and searched with the matcher of the editor, so a text or a regular expression finds
//! the same things here as in one document. Files are only read, never changed, and the folder is walked as [`crate::walk`] says
//! (links are not followed). The search is bounded in files, hits and the text each hit shows.

use std::path::{Path, PathBuf};

use xmlcraft_editor::find::{Matcher, Query, QueryError};
use xmlcraft_engine::schemastore::glob_match;

/// What to look for and where.
#[derive(Clone, Debug)]
pub struct Options {
    pub query: Query,
    /// File name patterns separated by `;` or `,` (`*.xml;*.xsd`); empty: every file.
    pub glob: String,
    /// The most files searched.
    pub max_files: usize,
    /// The most hits listed in all.
    pub max_hits: usize,
}

impl Options {
    pub fn new(query: Query) -> Self {
        Self { query, glob: String::new(), max_files: 5_000, max_hits: 2_000 }
    }
}

/// One match: where it is (from 1; the column counts characters) and the line it is on (cut after [`MAX_LINE`] characters).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub line: usize,
    pub column: usize,
    pub text: String,
}

/// The hits of one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHits {
    pub path: PathBuf,
    pub hits: Vec<Hit>,
}

/// What the search found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// The files with hits, in the order of the walk.
    pub files: Vec<FileHits>,
    /// How many files were read.
    pub searched: usize,
    /// Files that were not text (or had bytes that are not valid in their encoding) or could not be read.
    pub skipped: usize,
    /// There is more than was listed: the walk or the number of hits reached its limit.
    pub truncated: bool,
}

/// The longest line shown with a hit, in characters.
pub const MAX_LINE: usize = 200;

/// The hits of `matcher` in `text`, at most `limit` (and whether there were more).
pub fn hits_in(matcher: &Matcher, text: &str, limit: usize) -> (Vec<Hit>, bool) {
    let (ranges, more) = matcher.find_all(text, 0..text.len(), limit);
    let mut hits = Vec::with_capacity(ranges.len());
    let (mut line, mut line_start, mut scanned) = (1usize, 0usize, 0usize);
    for range in ranges {
        // Count the line breaks between the last place looked at and this match.
        if let Some(chunk) = text.get(scanned..range.start) {
            for (i, b) in chunk.bytes().enumerate() {
                if b == b'\n' {
                    line += 1;
                    line_start = scanned + i + 1;
                }
            }
        }
        scanned = range.start;
        let column = text.get(line_start..range.start).map_or(1, |s| s.chars().count() + 1);
        let end = text.get(line_start..).and_then(|r| r.find('\n')).map_or(text.len(), |n| line_start + n);
        let shown: String = text.get(line_start..end).unwrap_or_default().trim_end_matches('\r').chars().take(MAX_LINE).collect();
        hits.push(Hit { line, column, text: shown });
    }
    (hits, more)
}

/// Search the files under `root`.
pub fn search_folder(root: &Path, options: &Options) -> Result<Found, QueryError> {
    let matcher = Matcher::new(&options.query)?;
    let patterns: Vec<&str> = options.glob.split([';', ',']).map(str::trim).filter(|p| !p.is_empty()).collect();
    let include = |name: &str| patterns.is_empty() || patterns.iter().any(|p| glob_match(p, name));
    let walked = crate::walk::files(root, &include, options.max_files);
    let mut found = Found { truncated: walked.truncated, skipped: walked.unreadable, ..Found::default() };
    let mut total = 0usize;
    for path in &walked.files {
        let Ok(loaded) = crate::files::read_file(path) else {
            found.skipped += 1;
            continue;
        };
        let decoded = xmlcraft_text::encoding::decode(&loaded.bytes, &xmlcraft_text::encoding::sniff(&loaded.bytes));
        // A file that is not text (bytes that are not valid in its encoding, or NUL characters) has nothing to find.
        if decoded.lossy || decoded.text.contains('\0') {
            found.skipped += 1;
            continue;
        }
        found.searched += 1;
        let room = options.max_hits.saturating_sub(total);
        if room == 0 {
            found.truncated = true;
            break;
        }
        let (hits, more) = hits_in(&matcher, &decoded.text, room);
        total += hits.len();
        if !hits.is_empty() {
            found.files.push(FileHits { path: path.clone(), hits });
        }
        if more {
            found.truncated = true;
            break;
        }
    }
    Ok(found)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::fs;

    use super::*;

    fn write(dir: &Path, name: &str, bytes: &[u8]) {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        fs::write(path, bytes).expect("writes");
    }

    #[test]
    fn a_hit_says_its_line_its_column_in_characters_and_the_line_it_is_on() {
        let matcher = Matcher::new(&Query::text("needle")).expect("matcher");
        let text = "first\r\n  é needle here\nthird needle needle\n";
        let (hits, more) = hits_in(&matcher, text, 10);
        assert!(!more);
        assert_eq!(
            hits,
            [
                Hit { line: 2, column: 5, text: "  é needle here".to_owned() },
                Hit { line: 3, column: 7, text: "third needle needle".to_owned() },
                Hit { line: 3, column: 14, text: "third needle needle".to_owned() },
            ]
        );
        let (cut, more) = hits_in(&matcher, text, 2);
        assert_eq!((cut.len(), more), (2, true));
        let long = format!("{} needle", "x".repeat(500));
        assert_eq!(hits_in(&matcher, &long, 5).0[0].text.chars().count(), MAX_LINE);
        assert!(hits_in(&matcher, "", 5).0.is_empty());
    }

    #[test]
    fn a_folder_is_searched_by_file_name_pattern_and_what_is_not_text_is_skipped() {
        let dir = tempfile::tempdir().expect("dir");
        write(dir.path(), "a.xml", b"<a>needle</a>\n");
        write(dir.path(), "b.txt", b"needle too\n");
        write(dir.path(), "sub/c.xml", b"nothing\nneedle\n");
        write(dir.path(), "bin.xml", b"\x00\x01needle\x00");
        write(dir.path(), "bad.xml", b"needle \xff\xfe broken");
        let mut options = Options::new(Query::text("needle"));
        let all = search_folder(dir.path(), &options).expect("searches");
        let names: Vec<String> = all.files.iter().map(|f| f.path.file_name().expect("name").to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["a.xml", "b.txt", "c.xml"]);
        assert_eq!((all.searched, all.skipped, all.truncated), (5 - 2, 2, false), "the binary and the invalid are skipped");
        options.glob = "*.xml; *.xsd".to_owned();
        let xml = search_folder(dir.path(), &options).expect("searches");
        assert_eq!(xml.files.iter().map(|f| f.hits.len()).sum::<usize>(), 2);
        assert_eq!(xml.files[1].hits[0], Hit { line: 2, column: 1, text: "needle".to_owned() });
        let mut regex = Options::new(Query { pattern: "^ne+dle$".to_owned(), regex: true, case_sensitive: true, whole_word: false });
        regex.glob = "c.xml".to_owned();
        assert_eq!(search_folder(dir.path(), &regex).expect("searches").files.len(), 1, "^ and $ match at every line");
    }

    #[test]
    fn the_search_stops_at_its_limits_and_a_bad_pattern_is_an_error() {
        let dir = tempfile::tempdir().expect("dir");
        for i in 0..6 {
            write(dir.path(), &format!("f{i}.txt"), b"x x x\n");
        }
        let mut options = Options::new(Query::text("x"));
        options.max_hits = 7;
        let found = search_folder(dir.path(), &options).expect("searches");
        assert_eq!((found.files.iter().map(|f| f.hits.len()).sum::<usize>(), found.truncated), (7, true));
        options.max_hits = 1000;
        options.max_files = 2;
        let found = search_folder(dir.path(), &options).expect("searches");
        assert_eq!((found.searched, found.truncated), (2, true));
        assert_eq!(search_folder(dir.path(), &Options::new(Query::text(""))).expect_err("empty"), QueryError::Empty);
        assert!(matches!(
            search_folder(dir.path(), &Options::new(Query { pattern: "(".to_owned(), regex: true, case_sensitive: true, whole_word: false })),
            Err(QueryError::Invalid(_))
        ));
    }
}
