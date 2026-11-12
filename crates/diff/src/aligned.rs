//! Two texts side by side: their lines in rows, each row a line of both (the same, or changed), or a line of only one, and the runs of
//! rows that are not the same as hunks that can be copied from one side to the other.
//!
//! The rows come from the line comparison ([`crate::diff_lines`]). A comparison of trees (elements and members) can then [`mark`] the
//! hunks that hold one of its differences, so that a difference of the text alone (a line broken elsewhere, white space between
//! elements) is still shown but is not counted as a difference of the documents.
//!
//! [`mark`]: Aligned::mark

use std::ops::Range;

use xmlcraft_text::Edit;
use xmlcraft_text::eol::LineEndings;

use crate::lines::{LineDiff, LineOptions, diff_lines, diff_words_within, split_lines};

/// The most lines of one text that are put side by side.
pub const MAX_LINES: usize = 200_000;
/// The most pairs of changed lines whose words are compared (the rest are marked as wholly changed).
pub const MAX_WORD_PAIRS: usize = 20_000;
/// The longest line (in bytes) whose words are compared.
const MAX_WORD_LINE: usize = 4_000;
/// How much work comparing the words of one pair of lines may do.
const WORD_BUDGET: usize = 50_000;

/// One row: a line of each text, or of one of them. The numbers are lines (from 0) of the left and the right text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// The same line in both (as the options compare lines: with white space ignored they may differ in it).
    Equal(usize, usize),
    /// A line of the left that became the line of the right, with the parts of each that differ (byte ranges within the line).
    Changed { left: usize, right: usize, left_parts: Vec<Range<usize>>, right_parts: Vec<Range<usize>> },
    /// A line only the left text has.
    LeftOnly(usize),
    /// A line only the right text has.
    RightOnly(usize),
}

impl Row {
    /// The line of the left text in this row, if it has one.
    pub fn left(&self) -> Option<usize> {
        match self {
            Row::Equal(l, _) | Row::LeftOnly(l) | Row::Changed { left: l, .. } => Some(*l),
            Row::RightOnly(_) => None,
        }
    }

    /// The line of the right text in this row, if it has one.
    pub fn right(&self) -> Option<usize> {
        match self {
            Row::Equal(_, r) | Row::RightOnly(r) | Row::Changed { right: r, .. } => Some(*r),
            Row::LeftOnly(_) => None,
        }
    }
}

/// A maximal run of rows that are not [`Row::Equal`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlignedHunk {
    /// The first and the last row of the run (inclusive).
    pub first_row: usize,
    pub last_row: usize,
    /// The lines of the left text and of the right text in it (either may be empty: lines only added or only removed).
    pub left: Range<usize>,
    pub right: Range<usize>,
    /// Whether it is a difference of the documents: for lines, unless the options make it none (blank lines); after [`Aligned::mark`],
    /// whether a difference of the trees is in it.
    pub significant: bool,
}

/// Which text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Two texts in rows.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Aligned {
    pub rows: Vec<Row>,
    pub hunks: Vec<AlignedHunk>,
    /// The byte range of each line of the left text and of the right, without its line break.
    pub left_lines: Vec<Range<usize>>,
    pub right_lines: Vec<Range<usize>>,
    /// A text had more than [`MAX_LINES`] lines: nothing was compared and there are no rows.
    pub too_large: bool,
    /// Pairs of changed lines whose words were not compared (beyond [`MAX_WORD_PAIRS`], or too long): wholly marked.
    pub words_skipped: usize,
}

/// The lines of two texts side by side.
pub fn side_by_side(left: &str, right: &str, options: &LineOptions) -> Aligned {
    let (left_lines, right_lines) = (split_lines(left), split_lines(right));
    if left_lines.len() > MAX_LINES || right_lines.len() > MAX_LINES {
        return Aligned { left_lines, right_lines, too_large: true, ..Aligned::default() };
    }
    Aligned::from_diff(&diff_lines(left, right, options), left, right)
}

impl Aligned {
    /// The rows of a line comparison already made (of these two texts).
    pub fn from_diff(diff: &LineDiff, left: &str, right: &str) -> Aligned {
        let (left_lines, right_lines) = (diff.old_lines.clone(), diff.new_lines.clone());
        if left_lines.len() > MAX_LINES || right_lines.len() > MAX_LINES {
            return Aligned { left_lines, right_lines, too_large: true, ..Aligned::default() };
        }
        let mut aligned = Aligned { left_lines, right_lines, ..Aligned::default() };
        let mut pairs_done = 0usize;
        let (mut i, mut j) = (0usize, 0usize);
        for (gap, real) in diff.gaps() {
            aligned.equal_rows(&mut i, &mut j, gap.old.start, gap.new.start);
            let first_row = aligned.rows.len();
            let paired = gap.old.len().min(gap.new.len());
            for k in 0..paired {
                let (l, r) = (gap.old.start + k, gap.new.start + k);
                let (a, b) = (line_at(left, &aligned.left_lines, l), line_at(right, &aligned.right_lines, r));
                let (left_parts, right_parts) = if pairs_done < MAX_WORD_PAIRS && a.len() <= MAX_WORD_LINE && b.len() <= MAX_WORD_LINE {
                    pairs_done += 1;
                    diff_words_within(a, b, WORD_BUDGET)
                } else {
                    aligned.words_skipped += 1;
                    (whole(a), whole(b))
                };
                aligned.rows.push(Row::Changed { left: l, right: r, left_parts, right_parts });
            }
            aligned.rows.extend(gap.old.clone().skip(paired).map(Row::LeftOnly));
            aligned.rows.extend(gap.new.clone().skip(paired).map(Row::RightOnly));
            if let Some(last_row) = aligned.rows.len().checked_sub(1).filter(|last| *last >= first_row) {
                aligned.hunks.push(AlignedHunk { first_row, last_row, left: gap.old.clone(), right: gap.new.clone(), significant: *real });
            }
            i = gap.old.end;
            j = gap.new.end;
        }
        let (left_end, right_end) = (aligned.left_lines.len(), aligned.right_lines.len());
        aligned.equal_rows(&mut i, &mut j, left_end, right_end);
        aligned
    }
}

/// The text of line `n` (without its break).
fn line_at<'a>(text: &'a str, lines: &[Range<usize>], n: usize) -> &'a str {
    lines.get(n).and_then(|x| text.get(x.clone())).unwrap_or_default()
}

fn whole(line: &str) -> Vec<Range<usize>> {
    std::iter::once(0..line.len()).filter(|r| !r.is_empty()).collect()
}

/// The index of the line that holds byte `offset` (a line break belongs to the line it ends).
fn line_of(lines: &[Range<usize>], offset: usize) -> usize {
    lines.partition_point(|l| l.end < offset)
}

impl Aligned {
    /// Equal rows from lines `i`, `j` up to `to_left`, `to_right` (which the line comparison makes the same length).
    fn equal_rows(&mut self, i: &mut usize, j: &mut usize, to_left: usize, to_right: usize) {
        while *i < to_left && *j < to_right {
            self.rows.push(Row::Equal(*i, *j));
            *i += 1;
            *j += 1;
        }
    }

    /// How many hunks are differences of the documents.
    pub fn significant_count(&self) -> usize {
        self.hunks.iter().filter(|h| h.significant).count()
    }

    /// The hunk a row is in.
    pub fn hunk_of_row(&self, row: usize) -> Option<usize> {
        let at = self.hunks.partition_point(|h| h.last_row < row);
        self.hunks.get(at).filter(|h| h.first_row <= row).map(|_| at)
    }

    /// Mark the hunks that hold a difference of the trees: `left` and `right` are the byte ranges of the differences in each text (the
    /// node removed or changed in the left, added or changed in the right). Every hunk is then significant only when one of them meets
    /// its lines on either side. A difference whose lines are all the same in both texts (as can happen when the order of children is
    /// ignored) meets no hunk and is not shown here.
    pub fn mark(&mut self, left: &[Range<usize>], right: &[Range<usize>]) {
        // For each text, how many lines up to each line hold a difference (a running count, so that a hunk is looked up at once).
        let marked = |lines: &[Range<usize>], ranges: &[Range<usize>]| -> Vec<usize> {
            let mut starts = vec![0i64; lines.len() + 1];
            for r in ranges {
                let first = line_of(lines, r.start).min(lines.len());
                let last = line_of(lines, r.end.saturating_sub(1).max(r.start)).min(lines.len().saturating_sub(1));
                if first > last {
                    continue;
                }
                if let Some(s) = starts.get_mut(first) {
                    *s += 1;
                }
                if let Some(s) = starts.get_mut(last + 1) {
                    *s -= 1;
                }
            }
            // counts[i]: how many of lines 0..i are marked.
            let mut counts = Vec::with_capacity(lines.len() + 1);
            let (mut depth, mut total) = (0i64, 0usize);
            counts.push(0);
            for s in starts.iter().take(lines.len()) {
                depth += s;
                total += usize::from(depth > 0);
                counts.push(total);
            }
            counts
        };
        let (left_marks, right_marks) = (marked(&self.left_lines, left), marked(&self.right_lines, right));
        let meets = |counts: &[usize], lines: &Range<usize>| {
            !lines.is_empty() && counts.get(lines.end).copied().unwrap_or(0) > counts.get(lines.start).copied().unwrap_or(0)
        };
        for hunk in &mut self.hunks {
            hunk.significant = meets(&left_marks, &hunk.left) || meets(&right_marks, &hunk.right);
        }
    }
}

/// The text of the lines `range` of `text`, each line without its break.
fn lines_text<'a>(text: &'a str, lines: &[Range<usize>], range: &Range<usize>) -> Vec<&'a str> {
    range.clone().filter_map(|i| lines.get(i).and_then(|l| text.get(l.clone()))).collect()
}

/// The line break after line `i` (empty after the last line when the text does not end with one).
fn break_after<'a>(text: &'a str, lines: &[Range<usize>], i: usize) -> &'a str {
    let end = lines.get(i).map_or(text.len(), |l| l.end);
    let next = lines.get(i + 1).map_or(text.len(), |l| l.start);
    text.get(end..next).unwrap_or_default()
}

fn ends_with_break(text: &str) -> bool {
    text.ends_with(['\n', '\r'])
}

/// The edit that makes the `target` side of `hunk` the same as the other side: its lines are replaced by those of the other text,
/// with the target text's own (dominant) line ending, and the edit is trimmed to the bytes that change. Nothing is re-parsed: whether
/// the result is still well-formed is for the caller to check. The texts must be those that were compared. Empty when nothing changes.
pub fn take(aligned: &Aligned, hunk: &AlignedHunk, target: Side, left: &str, right: &str) -> Vec<Edit> {
    let (target_text, target_lines, target_range, source_text, source_lines, source_range) = match target {
        Side::Right => (right, &aligned.right_lines, &hunk.right, left, &aligned.left_lines, &hunk.left),
        Side::Left => (left, &aligned.left_lines, &hunk.left, right, &aligned.right_lines, &hunk.right),
    };
    let counts = LineEndings::count(target_text);
    let eol = if counts.total() == 0 { LineEndings::count(source_text).dominant() } else { counts.dominant() }.as_str();
    let source = lines_text(source_text, source_lines, source_range);
    let start_of = |i: usize| target_lines.get(i).map_or(target_text.len(), |l| l.start);
    let at_end = target_range.end >= target_lines.len() && source_range.end >= source_lines.len();
    let (span, replacement) = if at_end {
        // The hunk runs to the end of both texts: whether the text ends with a line break follows the other text.
        let a = target_range.start;
        let anchor = match a.checked_sub(1) {
            Some(previous) => target_lines.get(previous).map_or(target_text.len(), |l| l.end),
            None => 0,
        };
        let mut r = String::new();
        if let Some(previous) = a.checked_sub(1)
            && (!source.is_empty() || ends_with_break(source_text))
        {
            let existing = break_after(target_text, target_lines, previous);
            r.push_str(if existing.is_empty() { eol } else { existing });
        }
        r.push_str(&source.join(eol));
        if !source.is_empty() && ends_with_break(source_text) {
            r.push_str(eol);
        }
        (anchor..target_text.len(), r)
    } else {
        let span = start_of(target_range.start)..start_of(target_range.end);
        let mut r = String::with_capacity(source.iter().map(|l| l.len() + eol.len()).sum());
        for line in &source {
            r.push_str(line);
            r.push_str(eol);
        }
        (span, r)
    };
    minimal(target_text, span, replacement).into_iter().collect()
}

/// The edit replacing `span` of `text` by `replacement`, without the bytes at either end that it would leave as they are.
fn minimal(text: &str, span: Range<usize>, replacement: String) -> Option<Edit> {
    let old = text.get(span.clone())?;
    if old == replacement {
        return None;
    }
    let prefix = common_prefix(old, &replacement);
    let (old_rest, new_rest) = (old.get(prefix..).unwrap_or_default(), replacement.get(prefix..).unwrap_or_default());
    let suffix = common_suffix(old_rest, new_rest);
    let new_text = new_rest.get(..new_rest.len().saturating_sub(suffix)).unwrap_or_default().to_owned();
    let start = span.start.saturating_add(prefix);
    let end = span.end.saturating_sub(suffix).max(start);
    Some(Edit { range: start..end, text: new_text })
}

/// The length in bytes of the longest common beginning of two strings, at a character boundary.
fn common_prefix(a: &str, b: &str) -> usize {
    a.char_indices().zip(b.chars()).find(|((_, x), y)| x != y).map_or(a.len().min(b.len()), |((i, _), _)| i)
}

/// The same for the end. A line break `\r\n` is not split (so that an edit never leaves a `\r` and a `\n` that read as two lines).
fn common_suffix(a: &str, b: &str) -> usize {
    let mut n = 0;
    for (x, y) in a.chars().rev().zip(b.chars().rev()) {
        if x != y {
            break;
        }
        n += x.len_utf8();
    }
    // Do not keep a `\n` whose `\r` is replaced, nor start the kept part between `\r` and `\n`.
    let keep_start_a = a.len().saturating_sub(n);
    if n > 0 && a.as_bytes().get(keep_start_a) == Some(&b'\n') && keep_start_a > 0 && a.as_bytes().get(keep_start_a - 1) == Some(&b'\r') {
        n -= 1;
    }
    n
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;
    use xmlcraft_text::apply_edits;

    fn apply(text: &str, edits: &[Edit]) -> String {
        apply_edits(text, edits)
    }

    /// The text of the right after every hunk is taken from the left (from the last hunk to the first, so the places stay right).
    fn take_all(left: &str, right: &str) -> String {
        let aligned = side_by_side(left, right, &LineOptions::default());
        let mut edits: Vec<Edit> = aligned.hunks.iter().flat_map(|h| take(&aligned, h, Side::Right, left, right)).collect();
        edits.sort_by_key(|e| e.range.start);
        apply(right, &edits)
    }

    fn contents(text: &str) -> Vec<&str> {
        split_lines(text).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn a_changed_line_and_an_added_one_are_rows_and_hunks() {
        let (l, r) = ("a\nb\nc\n", "a\nB\nc\nd\n");
        let s = side_by_side(l, r, &LineOptions::default());
        assert_eq!(s.rows.len(), 4);
        assert_eq!(s.rows[0], Row::Equal(0, 0));
        assert!(
            matches!(&s.rows[1], Row::Changed { left: 1, right: 1, left_parts, right_parts } if left_parts == &vec![0..1] && right_parts == &vec![0..1])
        );
        assert_eq!(s.rows[2], Row::Equal(2, 2));
        assert_eq!(s.rows[3], Row::RightOnly(3));
        assert_eq!(s.hunks.len(), 2);
        assert_eq!((s.hunks[0].first_row, s.hunks[0].last_row), (1, 1));
        assert_eq!((s.hunks[1].left.clone(), s.hunks[1].right.clone()), (3..3, 3..4));
        assert_eq!(s.significant_count(), 2);
        assert_eq!(s.hunk_of_row(3), Some(1));
        assert_eq!(s.hunk_of_row(2), None);
    }

    #[test]
    fn the_words_that_differ_are_parts_of_the_line_in_bytes() {
        let s = side_by_side("<item n=\"2\">Milk</item>\n", "<item n=\"2\">Oat milk</item>\n", &LineOptions::default());
        let Row::Changed { left_parts, right_parts, .. } = &s.rows[0] else { panic!("{:?}", s.rows) };
        assert_eq!(&"<item n=\"2\">Milk</item>"[left_parts[0].clone()], "Milk");
        assert_eq!(&"<item n=\"2\">Oat milk</item>"[right_parts[0].clone()], "Oat milk");
    }

    #[test]
    fn multi_byte_lines_have_parts_at_character_boundaries() {
        let (l, r) = ("größe: ü\n", "größe: ö\n");
        let s = side_by_side(l, r, &LineOptions::default());
        let Row::Changed { left_parts, right_parts, .. } = &s.rows[0] else { panic!() };
        assert_eq!(&l[left_parts[0].clone()], "ü");
        assert_eq!(&r[right_parts[0].clone()], "ö");
        assert_eq!(take_all(l, r), l);
    }

    #[test]
    fn taking_a_hunk_replaces_only_its_lines_with_the_targets_line_ending() {
        let (l, r) = ("a\nX\nY\nc\n", "a\r\nb\r\nc\r\n");
        let s = side_by_side(l, r, &LineOptions::default());
        assert_eq!(s.hunks.len(), 1);
        let edits = take(&s, &s.hunks[0], Side::Right, l, r);
        assert_eq!(apply(r, &edits), "a\r\nX\r\nY\r\nc\r\n");
        assert_eq!(edits.len(), 1);
        // Trimmed to what changes: the line break after `b` stays.
        let back = take(&s, &s.hunks[0], Side::Left, l, r);
        assert_eq!(apply(l, &back), "a\nb\nc\n");
    }

    #[test]
    fn crlf_is_not_split_by_trimming() {
        let (l, r) = ("a\r\nb\r\n", "a\r\n");
        let s = side_by_side(l, r, &LineOptions::default());
        let edits = take(&s, &s.hunks[0], Side::Right, l, r);
        assert_eq!(apply(r, &edits), l);
        assert_eq!(take_all("x\r\n", "x\r\ny\r\n"), "x\r\n");
    }

    #[test]
    fn the_end_of_the_text_follows_the_other_side() {
        assert_eq!(take_all("a\nb", "a\nc"), "a\nb");
        assert_eq!(take_all("a\nb\n", "a"), "a\nb\n");
        assert_eq!(take_all("a", "a\nb\n"), "a");
        assert_eq!(take_all("", "a\nb\n"), "");
        assert_eq!(take_all("a\nb\n", ""), "a\nb\n");
        assert_eq!(take_all("x\ny", "y"), "x\ny");
    }

    #[test]
    fn nothing_to_take_is_no_edit() {
        let s = side_by_side("a\n", "a\n", &LineOptions::default());
        assert!(s.hunks.is_empty() && s.rows.len() == 1);
    }

    #[test]
    fn too_many_lines_are_refused_without_rows() {
        let big = "x\n".repeat(MAX_LINES + 1);
        let s = side_by_side(&big, "x\n", &LineOptions::default());
        assert!(s.too_large && s.rows.is_empty() && s.hunks.is_empty());
    }

    #[test]
    fn the_words_of_long_lines_are_not_compared_but_the_line_is_marked() {
        let (l, r) = ("a".repeat(MAX_WORD_LINE + 1), "b".repeat(10));
        let s = side_by_side(&l, &r, &LineOptions::default());
        assert_eq!(s.words_skipped, 1);
        let Row::Changed { left_parts, .. } = &s.rows[0] else { panic!() };
        assert_eq!(left_parts, &vec![0..l.len()]);
    }

    #[test]
    fn a_tree_difference_marks_the_hunk_it_is_in_and_leaves_the_others() {
        // The second hunk is white space between elements only.
        let (l, r) = ("<r>\n<a>1</a>\n<b/>\n</r>\n", "<r>\n<a>2</a>\n  <b/>\n</r>\n");
        let mut s = side_by_side(l, r, &LineOptions::default());
        assert_eq!(s.hunks.len(), 1, "two adjacent changed lines are one hunk");
        let (l2, r2) = ("<r>\n<a>1</a>\n<m/>\n<b/>\n</r>\n", "<r>\n<a>2</a>\n<m/>\n  <b/>\n</r>\n");
        let mut s2 = side_by_side(l2, r2, &LineOptions::default());
        assert_eq!(s2.hunks.len(), 2);
        let at = |t: &str, what: &str| t.find(what).map(|i| i..i + what.len()).expect("there");
        s2.mark(&[at(l2, "1")], &[at(r2, "2")]);
        assert_eq!(s2.hunks.iter().map(|h| h.significant).collect::<Vec<_>>(), vec![true, false]);
        assert_eq!(s2.significant_count(), 1);
        s.mark(&[], &[]);
        assert_eq!(s.significant_count(), 0, "no tree difference: the text differs, the documents do not");
        // An addition is marked on the side that has it.
        let (l3, r3) = ("<r>\n</r>\n", "<r>\n<n/>\n</r>\n");
        let mut s3 = side_by_side(l3, r3, &LineOptions::default());
        s3.mark(&[], &[at(r3, "<n/>")]);
        assert_eq!(s3.significant_count(), 1);
    }

    #[test]
    fn blank_lines_that_do_not_count_are_hunks_that_are_not_significant() {
        let options = LineOptions { ignore_blank_lines: true, ..LineOptions::default() };
        let s = side_by_side("a\n\nb\n", "a\nb\n", &options);
        assert_eq!(s.hunks.len(), 1);
        assert!(!s.hunks[0].significant);
    }

    fn line_text() -> impl proptest::strategy::Strategy<Value = String> {
        proptest::prop_oneof![
            proptest::string::string_regex("[ab ]{0,4}").expect("regex"),
            proptest::string::string_regex("[äöü€x]{0,3}").expect("regex"),
        ]
    }

    fn text_from(lines: Vec<String>, eol: &str, end: bool) -> String {
        let mut t = lines.join(eol);
        if end && !lines.is_empty() {
            t.push_str(eol);
        }
        t
    }

    proptest::proptest! {
        #[test]
        fn taking_every_hunk_left_to_right_gives_the_left_lines(
            a in proptest::collection::vec(line_text(), 0..12),
            b in proptest::collection::vec(line_text(), 0..12),
            crlf in proptest::bool::ANY,
            end_a in proptest::bool::ANY,
            end_b in proptest::bool::ANY,
        ) {
            let eol = if crlf { "\r\n" } else { "\n" };
            let (l, r) = (text_from(a, eol, end_a), text_from(b, eol, end_b));
            let s = side_by_side(&l, &r, &LineOptions::default());
            // Every line of each side is in exactly one row, in order.
            let lefts: Vec<usize> = s.rows.iter().filter_map(Row::left).collect();
            let rights: Vec<usize> = s.rows.iter().filter_map(Row::right).collect();
            proptest::prop_assert_eq!(lefts, (0..s.left_lines.len()).collect::<Vec<_>>());
            proptest::prop_assert_eq!(rights, (0..s.right_lines.len()).collect::<Vec<_>>());
            for h in &s.hunks {
                proptest::prop_assert_eq!(h.last_row + 1 - h.first_row, h.left.len().max(h.right.len()));
                proptest::prop_assert!(s.rows[h.first_row..=h.last_row].iter().all(|r| !matches!(r, Row::Equal(..))));
            }
            let equal = s.rows.iter().filter(|r| matches!(r, Row::Equal(..))).count();
            let in_hunks: usize = s.hunks.iter().map(|h| h.last_row + 1 - h.first_row).sum();
            proptest::prop_assert_eq!(equal + in_hunks, s.rows.len());
            for row in &s.rows {
                if let Row::Changed { left, right, left_parts, right_parts } = row {
                    let (lt, rt) = (&l[s.left_lines[*left].clone()], &r[s.right_lines[*right].clone()]);
                    proptest::prop_assert!(left_parts.iter().all(|p| lt.get(p.clone()).is_some()));
                    proptest::prop_assert!(right_parts.iter().all(|p| rt.get(p.clone()).is_some()));
                }
            }
            let taken = take_all(&l, &r);
            proptest::prop_assert_eq!(contents(&taken), contents(&l));
            // When both end the same way, even the bytes are the same (the line ending is the same on both sides here).
            if ends_with_break(&l) == ends_with_break(&r) {
                proptest::prop_assert_eq!(&taken, &l);
            }
        }
    }
}
