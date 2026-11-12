//! Comparing two documents on a worker thread: what the Compare window waits for. Two documents of one family (XML, or JSON/YAML, which
//! read into the same trees) are compared as trees, which says which element or member differs; others, or documents a tree cannot be
//! read from, are compared line by line. The comparison itself is [`xmlcraft_diff`].

use std::ops::Range;

use xmlcraft_diff::{Aligned, ChangeKind, Family, HunkKind, LineOptions, TreeOptions, Whitespace, compare_mixed, diff_lines, side_by_side};

use crate::language::Language;
use crate::worker::Worker;

/// The most differences an outcome lists (the rest are counted).
pub const MAX_LISTED: usize = 2000;

/// How to compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// As trees when both documents can be read as trees of a family that can be compared, else line by line.
    Auto,
    Tree,
    Lines,
}

/// What does not count as a difference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ignore {
    pub whitespace: bool,
    pub case: bool,
    pub comments: bool,
    pub child_order: bool,
    pub numbers_by_value: bool,
    pub prefixes: bool,
}

/// What a text is written in, for comparing.
pub fn family_of(language: Language) -> Family {
    match language {
        Language::Xml | Language::Html => Family::Xml,
        Language::Json(_) | Language::Bson => Family::Json,
        Language::Yaml => Family::Yaml,
    }
}

/// One request.
#[derive(Clone, Debug)]
pub struct CompareJob {
    pub serial: u64,
    pub old: String,
    pub old_language: Language,
    pub new: String,
    pub new_language: Language,
    pub mode: Mode,
    pub ignore: Ignore,
}

/// One difference, with where it is in each text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Difference {
    pub kind: ChangeKind,
    /// One line for a list: `changed /a/b: 1 → 2`, or `lines 3–5 → 4`.
    pub label: String,
    /// The place in the first text: the node or the lines, or (for an addition) where it would go.
    pub old: Range<usize>,
    /// The place in the second text, likewise.
    pub new: Range<usize>,
}

/// What a comparison found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compared {
    /// How the documents were compared (`Tree` or `Lines`, never `Auto`).
    pub mode: Mode,
    /// Why lines were compared although trees were wanted, when that is so.
    pub note: Option<String>,
    /// How many differences there are.
    pub count: usize,
    pub differences: Vec<Difference>,
    /// The two texts side by side (from their lines; in tree mode the hunks that hold a difference of the trees are the significant ones).
    pub aligned: Aligned,
}

/// What came of a request.
#[derive(Clone, Debug)]
pub struct CompareOutcome {
    pub serial: u64,
    pub result: Result<Compared, String>,
}

fn tree_options(ignore: Ignore) -> TreeOptions {
    TreeOptions {
        whitespace: if ignore.whitespace { Whitespace::Collapse } else { Whitespace::Exact },
        ignore_case: ignore.case,
        ignore_comments: ignore.comments,
        ignore_child_order: ignore.child_order,
        numbers_by_value: ignore.numbers_by_value,
        ignore_prefixes: ignore.prefixes,
        ..TreeOptions::default()
    }
}

fn line_options(ignore: Ignore) -> LineOptions {
    LineOptions {
        whitespace: if ignore.whitespace { Whitespace::Collapse } else { Whitespace::Exact },
        ignore_case: ignore.case,
        ignore_blank_lines: ignore.whitespace,
        ..LineOptions::default()
    }
}

/// `3` or `3–5`: the lines of a range (counted from 1).
fn lines_label(r: &Range<usize>) -> String {
    match r.len() {
        0 => format!("before line {}", r.start + 1),
        1 => format!("line {}", r.start + 1),
        _ => format!("lines {}–{}", r.start + 1, r.end),
    }
}

fn by_lines(job: &CompareJob, note: Option<String>) -> Compared {
    let diff = diff_lines(&job.old, &job.new, &line_options(job.ignore));
    let differences: Vec<Difference> = diff
        .hunks
        .iter()
        .take(MAX_LISTED)
        .map(|h| {
            let kind = match h.kind() {
                HunkKind::Added => ChangeKind::Added,
                HunkKind::Removed => ChangeKind::Removed,
                HunkKind::Changed => ChangeKind::Changed,
            };
            let label = match kind {
                ChangeKind::Added => format!("added {} of the second", lines_label(&h.new)),
                ChangeKind::Removed => format!("removed {} of the first", lines_label(&h.old)),
                ChangeKind::Changed => format!("changed {} → {}", lines_label(&h.old), lines_label(&h.new)),
            };
            Difference { kind, label, old: diff.old_span(h), new: diff.new_span(h) }
        })
        .collect();
    Compared { mode: Mode::Lines, note, count: diff.hunks.len(), differences, aligned: Aligned::from_diff(&diff, &job.old, &job.new) }
}

/// Compare the two texts of a job.
pub fn compare(job: &CompareJob) -> Result<Compared, String> {
    if job.mode == Mode::Lines {
        return Ok(by_lines(job, None));
    }
    let (old_family, new_family) = (family_of(job.old_language), family_of(job.new_language));
    // XML against data has nothing in common as trees.
    let comparable = (old_family == Family::Xml) == (new_family == Family::Xml);
    if !comparable {
        return if job.mode == Mode::Tree {
            Err("an XML document and a JSON or YAML document cannot be compared as trees".to_owned())
        } else {
            Ok(by_lines(job, Some("The documents are not of one kind, so they are compared line by line.".to_owned())))
        };
    }
    match compare_mixed(&job.old, old_family, &job.new, new_family, &tree_options(job.ignore)) {
        Ok(changes) => {
            let differences = changes
                .iter()
                .take(MAX_LISTED)
                .map(|c| Difference {
                    kind: c.kind,
                    label: c.describe(),
                    old: c.old.clone().unwrap_or_else(|| c.old_parent.clone()),
                    new: c.new.clone().unwrap_or_else(|| c.new_parent.clone()),
                })
                .collect();
            let mut aligned = side_by_side(&job.old, &job.new, &line_options(job.ignore));
            let left: Vec<Range<usize>> = changes.iter().filter_map(|c| c.old.clone()).collect();
            let right: Vec<Range<usize>> = changes.iter().filter_map(|c| c.new.clone()).collect();
            aligned.mark(&left, &right);
            Ok(Compared { mode: Mode::Tree, note: None, count: changes.len(), differences, aligned })
        }
        Err(e) if job.mode == Mode::Tree => Err(format!("a document cannot be compared as a tree: {e}")),
        Err(e) => Ok(by_lines(job, Some(format!("A document cannot be read as a tree ({e}), so they are compared line by line.")))),
    }
}

/// Compares documents on a worker thread.
pub struct Comparer {
    worker: Worker<CompareJob, CompareOutcome>,
}

fn run(job: CompareJob) -> CompareOutcome {
    let serial = job.serial;
    let result = match crate::guard(|| compare(&job)) {
        Ok(done) => done,
        Err(panic) => {
            log::error!("comparing documents failed: {panic}");
            Err("the comparison failed (a bug); nothing was changed".to_owned())
        }
    };
    CompareOutcome { serial, result }
}

impl Comparer {
    pub fn new() -> Self {
        Self { worker: Worker::new("comparison", run) }
    }

    /// A comparer with no thread: `submit` compares at once.
    pub fn inline() -> Self {
        Self { worker: Worker::inline(run) }
    }

    pub fn on_done(&self, notify: impl Fn() + Send + Sync + 'static) {
        self.worker.on_done(notify);
    }

    pub fn submit(&self, job: CompareJob) {
        self.worker.submit(0, job);
    }

    pub fn poll(&self) -> Vec<CompareOutcome> {
        self.worker.poll().into_iter().map(|(_, outcome)| outcome).collect()
    }
}

impl Default for Comparer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::JsonFlavor;

    fn job(old: &str, old_language: Language, new: &str, new_language: Language, mode: Mode) -> CompareJob {
        CompareJob { serial: 1, old: old.to_owned(), old_language, new: new.to_owned(), new_language, mode, ignore: Ignore::default() }
    }

    const JSON: Language = Language::Json(JsonFlavor::Json);

    #[test]
    fn xml_documents_are_compared_as_trees_and_each_difference_is_at_its_place() {
        let (a, b) = ("<r><i n=\"1\"/><i n=\"2\"/></r>", "<r><i n=\"1\"/><i n=\"3\"/></r>");
        let done = compare(&job(a, Language::Xml, b, Language::Xml, Mode::Auto)).expect("compares");
        assert_eq!((done.mode, done.count), (Mode::Tree, 1));
        let d = &done.differences[0];
        assert_eq!(d.kind, ChangeKind::Changed);
        assert_eq!(&a[d.old.clone()], "n=\"2\"");
        assert_eq!(&b[d.new.clone()], "n=\"3\"");
        assert!(d.label.contains("@n"), "{}", d.label);
    }

    #[test]
    fn the_side_by_side_rows_count_only_the_hunks_with_a_tree_difference() {
        // The second line differs in a value, the fourth only in the order of its attributes (no difference of the trees).
        let (a, b) = ("<r>\n<a>1</a>\n<m/>\n<b x=\"1\" y=\"2\"/>\n</r>\n", "<r>\n<a>2</a>\n<m/>\n<b y=\"2\" x=\"1\"/>\n</r>\n");
        let tree = compare(&job(a, Language::Xml, b, Language::Xml, Mode::Auto)).expect("compares");
        assert_eq!((tree.aligned.hunks.len(), tree.aligned.significant_count()), (2, 1));
        let lines = compare(&job(a, Language::Xml, b, Language::Xml, Mode::Lines)).expect("compares");
        assert_eq!((lines.aligned.hunks.len(), lines.aligned.significant_count()), (2, 2));
    }

    #[test]
    fn a_json_document_and_a_yaml_document_with_the_same_data_are_the_same() {
        let done = compare(&job("{\"a\": [1, 2]}", JSON, "a:\n  - 1\n  - 2\n", Language::Yaml, Mode::Auto)).expect("compares");
        assert_eq!((done.mode, done.count), (Mode::Tree, 0));
    }

    #[test]
    fn what_cannot_be_read_as_a_tree_is_compared_by_lines_and_says_so() {
        let done = compare(&job("{\"a\": 1,\n", JSON, "{\"a\": 2}\n", JSON, Mode::Auto)).expect("compares");
        assert_eq!(done.mode, Mode::Lines);
        assert!(done.note.as_deref().is_some_and(|n| n.contains("line by line")), "{:?}", done.note);
        assert!(compare(&job("{\"a\": 1,\n", JSON, "{\"a\": 2}\n", JSON, Mode::Tree)).is_err(), "asked for trees, it says why not");
    }

    #[test]
    fn xml_against_data_is_compared_by_lines() {
        let done = compare(&job("<a/>", Language::Xml, "{}", JSON, Mode::Auto)).expect("compares");
        assert_eq!(done.mode, Mode::Lines);
        assert!(compare(&job("<a/>", Language::Xml, "{}", JSON, Mode::Tree)).is_err());
    }

    #[test]
    fn lines_say_which_lines_and_where() {
        let (a, b) = ("one\ntwo\nthree\n", "one\n2\nthree\nfour\n");
        let done = compare(&job(a, Language::Xml, b, Language::Xml, Mode::Lines)).expect("compares");
        assert_eq!(done.count, 2);
        assert_eq!(done.differences[0].label, "changed line 2 → line 2");
        assert_eq!(&a[done.differences[0].old.clone()], "two");
        assert_eq!(&b[done.differences[0].new.clone()], "2");
        assert_eq!(done.differences[1].kind, ChangeKind::Added);
    }

    #[test]
    fn the_ignore_options_reach_the_comparison() {
        let (a, b) = ("<r>a  b</r>", "<r>a b</r>");
        let mut j = job(a, Language::Xml, b, Language::Xml, Mode::Auto);
        assert_eq!(compare(&j).expect("compares").count, 1);
        j.ignore.whitespace = true;
        assert_eq!(compare(&j).expect("compares").count, 0);
        let mut j = job("{\"a\": 1.0}", JSON, "{\"a\": 1}", JSON, Mode::Auto);
        assert_eq!(compare(&j).expect("compares").count, 1);
        j.ignore.numbers_by_value = true;
        assert_eq!(compare(&j).expect("compares").count, 0);
    }

    #[test]
    fn the_list_is_capped_but_the_count_is_not() {
        let n = MAX_LISTED * 2 + 100;
        let same: String = (0..n).map(|i| format!("{i}\n")).collect();
        let other: String = (0..n).map(|i| if i % 2 == 0 { format!("x{i}\n") } else { format!("{i}\n") }).collect();
        let done = compare(&job(&same, Language::Xml, &other, Language::Xml, Mode::Lines)).expect("compares");
        assert!(done.count > MAX_LISTED, "{}", done.count);
        assert_eq!(done.differences.len(), MAX_LISTED);
    }

    #[test]
    fn a_threaded_comparer_delivers() {
        let c = Comparer::new();
        c.submit(job("<a/>", Language::Xml, "<b/>", Language::Xml, Mode::Auto));
        let started = std::time::Instant::now();
        let mut got = Vec::new();
        while got.is_empty() {
            assert!(started.elapsed() < std::time::Duration::from_secs(10), "no outcome");
            got = c.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(got[0].serial, 1);
        assert_eq!(got[0].result.as_ref().expect("compares").count, 2, "a removed element and an added one");
    }
}
