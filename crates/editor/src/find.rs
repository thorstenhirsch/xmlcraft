//! Find and replace: a [`Query`] compiled into a [`Matcher`] that finds matches in a text and writes the edits of a replacement.
//!
//! It works on a `&str`, so it knows nothing of carets or the buffer: the editor (and the tools) hand it the text. A literal search
//! is a regular expression of the escaped text, so case folding and whole-word checks are the regex engine's (Unicode-aware) ones.
//! Every compile and every search is bounded: the pattern's size, the number of matches returned.

use std::ops::Range;

use regex::{Regex, RegexBuilder};
use xmlcraft_text::{Edit, Selection};

use crate::Editor;

/// The most memory a compiled pattern may take, in bytes: a hostile pattern fails to compile instead of eating memory.
const PATTERN_SIZE_LIMIT: usize = 4 * 1024 * 1024;
/// The longest pattern accepted, in bytes.
const MAX_PATTERN_BYTES: usize = 64 * 1024;

/// What to look for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    pub pattern: String,
    /// The pattern is a regular expression (`^` and `$` match at every line, `.` does not match a line break); otherwise it is text.
    pub regex: bool,
    pub case_sensitive: bool,
    /// A match must start and end at a word boundary.
    pub whole_word: bool,
}

impl Query {
    /// A search for this text, case-sensitive.
    pub fn text(pattern: &str) -> Self {
        Self { pattern: pattern.to_owned(), regex: false, case_sensitive: true, whole_word: false }
    }
}

/// Why a query cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryError {
    /// Nothing to look for.
    Empty,
    /// The regular expression is not valid (or too big); the message says where.
    Invalid(String),
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryError::Empty => f.write_str("there is nothing to look for"),
            QueryError::Invalid(message) => write!(f, "the pattern is not valid: {message}"),
        }
    }
}

impl std::error::Error for QueryError {}

/// A compiled [`Query`].
#[derive(Clone, Debug)]
pub struct Matcher {
    regex: Regex,
    /// The pattern is a regular expression, so a replacement may refer to groups (`$1`, `${name}`).
    groups: bool,
}

impl Matcher {
    pub fn new(query: &Query) -> Result<Self, QueryError> {
        if query.pattern.is_empty() {
            return Err(QueryError::Empty);
        }
        if query.pattern.len() > MAX_PATTERN_BYTES {
            return Err(QueryError::Invalid(format!("longer than {MAX_PATTERN_BYTES} bytes")));
        }
        let body = if query.regex { query.pattern.clone() } else { regex::escape(&query.pattern) };
        let source = if query.whole_word { format!(r"\b(?:{body})\b") } else { body };
        let regex = RegexBuilder::new(&source)
            .case_insensitive(!query.case_sensitive)
            .multi_line(true)
            // `^` and `$` are at the ends of lines whether they end in LF or CRLF.
            .crlf(true)
            .size_limit(PATTERN_SIZE_LIMIT)
            .dfa_size_limit(PATTERN_SIZE_LIMIT)
            .nest_limit(64)
            .build()
            .map_err(|e| QueryError::Invalid(first_line(&e.to_string())))?;
        Ok(Self { regex, groups: query.regex })
    }

    /// The first match that starts at or after byte `from` (which must be on a character boundary).
    pub fn find(&self, text: &str, from: usize) -> Option<Range<usize>> {
        if !text.is_char_boundary(from) {
            return None;
        }
        self.regex.find_at(text, from).map(|m| m.range())
    }

    /// The last match that ends at or before byte `before`. (The whole text is searched, so `$` and look-around see what is really
    /// around the match, not the artificial end of a prefix.)
    pub fn find_last(&self, text: &str, before: usize) -> Option<Range<usize>> {
        let end = floor_boundary(text, before);
        // Matches do not overlap, so the last one before `end` is the last of those found from the start. (A regex cannot search backwards.)
        self.regex.find_iter(text).take_while(|m| m.end() <= end).last().map(|m| m.range())
    }

    /// Every match inside `within` (at most `limit`), and whether there were more. A match is inside when it lies entirely between the
    /// ends; the text around is still the whole text. An empty match (a pattern such as `a*`) counts. The search starts at the start of
    /// `within`, so a match before it that would overlap the first one inside is no obstacle.
    pub fn find_all(&self, text: &str, within: Range<usize>, limit: usize) -> (Vec<Range<usize>>, bool) {
        let end = floor_boundary(text, within.end);
        let mut at = ceil_boundary(text, within.start);
        let mut found = Vec::new();
        let mut last_end: Option<usize> = None;
        while at <= end {
            let Some(m) = self.regex.find_at(text, at) else { break };
            if m.end() > end {
                break;
            }
            // As `Regex::find_iter` does it: an empty match where the match before ended is not a match of its own.
            let skipped = m.is_empty() && last_end == Some(m.end());
            if !skipped {
                if found.len() >= limit {
                    return (found, true);
                }
                found.push(m.range());
            }
            last_end = Some(m.end());
            at = if m.is_empty() { next_boundary(text, m.end()).unwrap_or(usize::MAX) } else { m.end() };
        }
        (found, false)
    }

    /// What `replacement` stands for at `found` (a match of this matcher in `text`): in a regular-expression search `$1` and `${name}`
    /// are the groups (`$$` is a dollar sign); otherwise the replacement is taken as it is.
    pub fn expand(&self, text: &str, found: &Range<usize>, replacement: &str) -> String {
        if !self.groups {
            return replacement.to_owned();
        }
        // The whole text, not a prefix: a match that depends on what follows it (look-ahead cannot be written, but `\b` and `$` do) has the
        // same groups it was found with.
        let Some(captures) = self.regex.captures_at(text, found.start) else { return replacement.to_owned() };
        let mut out = String::new();
        captures.expand(replacement, &mut out);
        out
    }

    /// The edits that replace every match inside `within`, unless there are more than `limit` of them (better none than half) or the
    /// replacements together would be more than [`MAX_REPLACED_BYTES`], so that a long replacement over many matches cannot exhaust
    /// memory.
    pub fn replace_all(&self, text: &str, within: Range<usize>, replacement: &str, limit: usize) -> Result<Vec<Edit>, Refused> {
        let (matches, more) = self.find_all(text, within, limit);
        if more {
            return Err(Refused::TooManyMatches);
        }
        let mut total = 0usize;
        let mut edits = Vec::with_capacity(matches.len());
        for range in matches {
            let with = self.expand(text, &range, replacement);
            total = total.saturating_add(with.len());
            if total > MAX_REPLACED_BYTES {
                return Err(Refused::TooMuchText);
            }
            edits.push(Edit::replace(range, with));
        }
        Ok(edits)
    }
}

/// Why a replace-all was not done (nothing is changed then).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// More than [`MAX_MATCHES`] matches.
    TooManyMatches,
    /// The replacements would add up to more than [`MAX_REPLACED_BYTES`].
    TooMuchText,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::TooManyMatches => write!(f, "more than {MAX_MATCHES} matches"),
            Refused::TooMuchText => write!(f, "the replacements would be more than {} MiB", MAX_REPLACED_BYTES / (1024 * 1024)),
        }
    }
}

impl std::error::Error for Refused {}

/// How many matches select-all and replace-all will deal with at once.
pub const MAX_MATCHES: usize = 10_000;

/// The most text a replace-all may write in all.
pub const MAX_REPLACED_BYTES: usize = 16 * 1024 * 1024;

/// Which way to look, and whether to start again at the other end of the text when nothing is left that way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Direction {
    pub backwards: bool,
    pub wrap: bool,
}

impl Direction {
    pub const NEXT: Self = Self { backwards: false, wrap: true };
    pub const PREVIOUS: Self = Self { backwards: true, wrap: true };
}

impl Editor {
    /// The match to go to from the primary caret: the first one after the selection (or before it, going backwards).
    pub fn find(&self, matcher: &Matcher, direction: Direction) -> Option<Range<usize>> {
        let text = self.text();
        let primary = self.primary();
        let (start, end) = (primary.start(), primary.end());
        if direction.backwards {
            // Before the selection; an empty match at the caret is skipped, or the search would stay where it is.
            let mut found = matcher.find_last(&text, start);
            if start == end && found.as_ref().is_some_and(|m| m.is_empty() && m.start == start) {
                found = previous_boundary(&text, start).and_then(|before| matcher.find_last(&text, before));
            }
            found.or_else(|| if direction.wrap { matcher.find_last(&text, text.len()) } else { None })
        } else {
            let mut found = matcher.find(&text, end);
            if start == end && found.as_ref().is_some_and(|m| m.is_empty() && m.start == end) {
                found = next_boundary(&text, end).and_then(|after| matcher.find(&text, after));
            }
            found.or_else(|| if direction.wrap { matcher.find(&text, 0) } else { None })
        }
    }

    /// Select the next (or previous) match. Returns it, or `None` when there is none.
    pub fn select_match(&mut self, matcher: &Matcher, direction: Direction) -> Option<Range<usize>> {
        let found = self.find(matcher, direction)?;
        self.set_selection(found.start, found.end);
        Some(found)
    }

    /// Select the first match that starts at or after the start of the primary selection (so a search that is being typed stays on the
    /// match it has, as long as it still matches), wrapping round. Returns it, or `None` when there is none.
    pub fn select_match_at_or_after_caret(&mut self, matcher: &Matcher) -> Option<Range<usize>> {
        let text = self.text();
        let from = self.primary().start();
        let found = matcher.find(&text, from).or_else(|| matcher.find(&text, 0))?;
        self.set_selection(found.start, found.end);
        Some(found)
    }

    /// Replace the match that is selected (if the selection is exactly one) by `replacement`, then select the next one. Returns whether
    /// anything was replaced; when the selection is not a match, it only moves on to the next one.
    pub fn replace_next(&mut self, matcher: &Matcher, replacement: &str) -> bool {
        let text = self.text();
        let selection = self.primary();
        let selected = selection.start()..selection.end();
        let is_match = matcher.find(&text, selected.start).is_some_and(|m| m == selected);
        let mut replaced = false;
        if is_match {
            // New line breaks in the replacement are the document's.
            let with = matcher.expand(&text, &selected, &convert_eol(replacement, self.settings().eol));
            replaced = self.apply_edits(&[Edit::replace(selected.clone(), with)]);
            // Carried through the edit, the caret is at the end of what was written.
        }
        self.select_match(matcher, Direction::NEXT);
        replaced
    }

    /// Replace every match (in the primary selection when `in_selection` and it is not empty) as one undoable step. Returns how many were
    /// replaced, or why not (nothing is changed then): more than [`MAX_MATCHES`] matches, or more than [`MAX_REPLACED_BYTES`] to write.
    pub fn replace_all(&mut self, matcher: &Matcher, replacement: &str, in_selection: bool) -> Result<usize, Refused> {
        let text = self.text();
        let selection = self.primary();
        let within = if in_selection && !selection.is_empty() { selection.start()..selection.end() } else { 0..text.len() };
        let edits = matcher.replace_all(&text, within, &convert_eol(replacement, self.settings().eol), MAX_MATCHES)?;
        let count = edits.len();
        if count > 0 {
            self.apply_edits(&edits);
        }
        Ok(count)
    }

    /// Put a caret on every match, selecting each; the last is the primary. Returns how many, or `None` when there are more than
    /// [`MAX_MATCHES`] (nothing is changed then: carets on only the first ones would be a trap).
    pub fn select_all_matches(&mut self, matcher: &Matcher) -> Option<usize> {
        let text = self.text();
        let (found, more) = matcher.find_all(&text, 0..text.len(), MAX_MATCHES);
        if more {
            return None;
        }
        if found.is_empty() {
            return Some(0);
        }
        let selections: Vec<Selection> = found.iter().map(|r| Selection { anchor: r.start, head: r.end }).collect();
        self.set_selections(&selections);
        Some(selections.len())
    }

    /// ⌘D: with an empty selection, select the word at the caret; otherwise add the next occurrence of the selected text (the text of the
    /// primary selection, matched exactly) as a further selection, wrapping round. Returns whether something was selected.
    pub fn select_next_occurrence(&mut self) -> bool {
        let primary = self.primary();
        if primary.is_empty() {
            let word = word_at(&self.text(), primary.head());
            return match word {
                Some(range) => {
                    self.set_selection(range.start, range.end);
                    true
                }
                None => false,
            };
        }
        let text = self.text();
        let Some(needle) = text.get(primary.start()..primary.end()) else { return false };
        let Ok(matcher) = Matcher::new(&Query::text(needle)) else { return false };
        let taken = self.selections();
        // Every occurrence, those after the selection first and then those before it: the first that is not taken.
        let (all, _) = matcher.find_all(&text, 0..text.len(), MAX_MATCHES);
        let (after, before): (Vec<_>, Vec<_>) = all.into_iter().partition(|m| m.start >= primary.end());
        let Some(found) = after.into_iter().chain(before).find(|m| !taken.iter().any(|s| s.start() < m.end && m.start < s.end())) else {
            return false;
        };
        let mut all = taken;
        all.push(Selection { anchor: found.start, head: found.end });
        self.set_selections(&all);
        true
    }
}

/// The word at (or just before) `offset`: letters, digits and the characters of XML names.
fn word_at(text: &str, offset: usize) -> Option<Range<usize>> {
    let at = floor_boundary(text, offset);
    let is_word = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':');
    let start = text.get(..at)?.char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(at, |(i, _)| i);
    let end = at + text.get(at..)?.char_indices().take_while(|(_, c)| is_word(*c)).last().map_or(0, |(i, c)| i + c.len_utf8());
    (start < end).then_some(start..end)
}

/// `text` as a pattern that matches exactly it (for putting a selection into a regular-expression search).
pub fn escape(text: &str) -> String {
    regex::escape(text)
}

/// The replacement with its line breaks written as the document writes them.
fn convert_eol(text: &str, eol: xmlcraft_text::eol::Eol) -> String {
    xmlcraft_text::eol::convert(text, eol)
}

/// The first character boundary after `at`, if the text goes on.
fn next_boundary(text: &str, at: usize) -> Option<usize> {
    (at < text.len()).then(|| ceil_boundary(text, at + 1))
}

/// The last character boundary before `at`, if there is one.
fn previous_boundary(text: &str, at: usize) -> Option<usize> {
    (at > 0).then(|| floor_boundary(text, at - 1))
}

fn first_line(message: &str) -> String {
    message.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or(message).trim().trim_start_matches("error: ").to_owned()
}

fn floor_boundary(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn ceil_boundary(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while at < text.len() && !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // the tests compare a list of ranges with one range in it
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn matcher(pattern: &str) -> Matcher {
        Matcher::new(&Query::text(pattern)).expect("compiles")
    }

    fn all(m: &Matcher, text: &str) -> Vec<Range<usize>> {
        m.find_all(text, 0..text.len(), 1000).0
    }

    #[test]
    fn text_is_found_literally_even_with_regex_characters() {
        let m = matcher("a.b(");
        assert_eq!(all(&m, "a.b( axb( a.b("), [0..4, 10..14]);
        assert_eq!(m.find("a.b( a.b(", 1), Some(5..9));
        assert_eq!(m.find("a.b(", 5), None);
    }

    #[test]
    fn case_and_whole_words() {
        let q = |pattern: &str, case_sensitive, whole_word| {
            Matcher::new(&Query { pattern: pattern.into(), regex: false, case_sensitive, whole_word }).expect("ok")
        };
        assert_eq!(all(&q("Book", true, false), "book Book BOOK"), [5..9]);
        assert_eq!(all(&q("Book", false, false), "book Book BOOK"), [0..4, 5..9, 10..14]);
        assert_eq!(all(&q("book", false, true), "book books notebook BOOK"), [0..4, 20..24]);
        assert_eq!(all(&q("É", false, false), "é É"), [0..2, 3..5], "Unicode case folding");
    }

    #[test]
    fn regular_expressions_have_groups_and_line_anchors() {
        let m = Matcher::new(&Query { pattern: r"<(\w+)/>".into(), regex: true, case_sensitive: true, whole_word: false }).expect("ok");
        let text = "<a/> <b/>";
        let edits = m.replace_all(text, 0..text.len(), "<$1></$1>", 100).expect("few matches");
        assert_eq!(xmlcraft_text::apply_edits(text, &edits), "<a></a> <b></b>");
        let lines = Matcher::new(&Query { pattern: "^x$".into(), regex: true, case_sensitive: true, whole_word: false }).expect("ok");
        assert_eq!(all(&lines, "x\nxx\nx"), [0..1, 5..6], "^ and $ match at every line");
        // In a text search a dollar sign is a dollar sign.
        let literal = matcher("a");
        assert_eq!(literal.expand("a", &(0..1), "$1 $$"), "$1 $$");
    }

    #[test]
    fn find_last_and_ranges() {
        let m = matcher("ab");
        let text = "ab ab ab";
        assert_eq!(m.find_last(text, text.len()), Some(6..8));
        assert_eq!(m.find_last(text, 7), Some(3..5), "ends at or before the position");
        assert_eq!(m.find_last(text, 1), None);
        assert_eq!(m.find_all(text, 2..8, 10).0, [3..5, 6..8]);
        assert_eq!(m.find_all(text, 0..8, 2), (vec![0..2, 3..5], true), "the limit, and that there were more");
        assert_eq!(m.replace_all(text, 0..8, "X", 2), Err(Refused::TooManyMatches), "too many to replace safely");
    }

    #[test]
    fn a_range_is_searched_from_its_start_whatever_overlaps_before_it() {
        let m = matcher("aba");
        // The match at 0..3 overlaps the one at 2..5: a search of the whole text does not see the second, a search from 2 does.
        assert_eq!(m.find_all("ababa", 0..5, 10).0, [0..3]);
        assert_eq!(m.find_all("ababa", 2..5, 10).0, [2..5]);
        assert_eq!(m.find_all("ababa", 1..5, 10).0, [2..5]);
        // What is around the range is still there for the anchors and word boundaries.
        let word = Matcher::new(&Query { pattern: "ab".into(), regex: false, case_sensitive: true, whole_word: true }).expect("ok");
        assert_eq!(word.find_all("xab ab", 1..6, 10).0, [4..6], "`ab` at 1 is in the middle of a word");
        let line = Matcher::new(&Query { pattern: "^b".into(), regex: true, case_sensitive: true, whole_word: false }).expect("ok");
        assert_eq!(line.find_all("ab\nb", 1..4, 10).0, [3..4]);
        assert_eq!(line.find_all("ab\nb", 0..2, 10).0, Vec::<Range<usize>>::new());
    }

    #[test]
    fn bad_queries_are_errors_not_panics() {
        assert_eq!(Matcher::new(&Query::text("")).expect_err("empty"), QueryError::Empty);
        let bad = Matcher::new(&Query { pattern: "(".into(), regex: true, case_sensitive: true, whole_word: false }).expect_err("unbalanced");
        assert!(matches!(&bad, QueryError::Invalid(m) if !m.is_empty()), "{bad}");
        // A pattern that would compile to something huge is refused, and so is one that is simply too long.
        let huge = Matcher::new(&Query { pattern: r"(\w{1000}){1000}".into(), regex: true, case_sensitive: true, whole_word: false });
        assert!(matches!(huge, Err(QueryError::Invalid(_))));
        assert!(Matcher::new(&Query::text(&"a".repeat(MAX_PATTERN_BYTES + 1))).is_err());
        // Positions inside a character are not matches.
        assert_eq!(matcher("é").find("é", 1), None);
    }

    #[test]
    fn empty_matches_do_not_loop() {
        let m = Matcher::new(&Query { pattern: "x*".into(), regex: true, case_sensitive: true, whole_word: false }).expect("ok");
        let (found, more) = m.find_all("abc", 0..3, 100);
        assert!(!more && found.len() == 4 && found.iter().all(Range::is_empty), "{found:?}");
    }

    fn sel(e: &Editor) -> Vec<(usize, usize)> {
        e.carets().iter().map(|c| (c.start(), c.end())).collect()
    }

    #[test]
    fn selecting_matches_goes_forwards_and_backwards_and_wraps() {
        let mut e = Editor::new("ab x ab y ab");
        let m = matcher("ab");
        e.set_selection(0, 0);
        assert_eq!(e.select_match(&m, Direction::NEXT), Some(0..2), "from a caret at the start, the match there");
        assert_eq!(e.select_match(&m, Direction::NEXT), Some(5..7));
        assert_eq!(e.select_match(&m, Direction::NEXT), Some(10..12));
        assert_eq!(e.select_match(&m, Direction::NEXT), Some(0..2), "wraps round");
        assert_eq!(e.select_match(&m, Direction::PREVIOUS), Some(10..12), "and back");
        assert_eq!(e.select_match(&m, Direction::PREVIOUS), Some(5..7));
        let no_wrap = Direction { backwards: false, wrap: false };
        e.set_selection(10, 12);
        assert_eq!(e.find(&m, no_wrap), None, "nothing after the last without wrapping");
        e.set_selection(11, 11);
        assert_eq!(e.find(&m, Direction::NEXT), Some(0..2));
        assert_eq!(e.select_match(&matcher("zz"), Direction::NEXT), None);
        assert_eq!(sel(&e), [(11, 11)], "no match: the selection stays");
    }

    #[test]
    fn replacing_one_at_a_time_then_all() {
        let mut e = Editor::new("a-a-a");
        let m = matcher("a");
        e.set_selection(0, 0);
        // Not on a match yet: it only goes to the first.
        assert!(!e.replace_next(&m, "X"));
        assert_eq!(sel(&e), [(0, 1)]);
        assert!(e.replace_next(&m, "X"), "the selected match is replaced and the next one selected");
        assert_eq!((e.text().as_str(), sel(&e)), ("X-a-a", vec![(2, 3)]));
        assert!(e.replace_next(&m, "YY"));
        assert_eq!((e.text().as_str(), sel(&e)), ("X-YY-a", vec![(5, 6)]));
        e.undo();
        assert_eq!(e.text(), "X-a-a", "one replacement is one undo step");
        e.set_selection(0, 3);
        assert_eq!(e.replace_all(&m, "Q", true), Ok(1), "only inside the selection: `X-a`");
        assert_eq!(e.text(), "X-Q-a");
        assert_eq!(e.replace_all(&m, "Q", false), Ok(1));
        assert_eq!(e.text(), "X-Q-Q");
        e.undo();
        assert_eq!(e.text(), "X-Q-a", "replace all is one step too");
        assert_eq!(e.replace_all(&matcher("nothing"), "Q", false), Ok(0));
    }

    #[test]
    fn regular_expression_replacements_use_groups() {
        let mut e = Editor::new("<a x=\"1\"/><b x=\"2\"/>");
        let m = Matcher::new(&Query { pattern: r#"x="(\d)""#.into(), regex: true, case_sensitive: true, whole_word: false }).expect("ok");
        assert_eq!(e.replace_all(&m, "n=\"$1$1\"", false), Ok(2));
        assert_eq!(e.text(), "<a n=\"11\"/><b n=\"22\"/>");
    }

    #[test]
    fn select_all_matches_makes_a_caret_for_each() {
        let mut e = Editor::new("ab ab ab");
        assert_eq!(e.select_all_matches(&matcher("ab")), Some(3));
        assert_eq!(sel(&e), [(0, 2), (3, 5), (6, 8)]);
        e.type_text("X");
        assert_eq!(e.text(), "X X X", "typing replaces every selection at once");
        assert_eq!(e.select_all_matches(&matcher("nope")), Some(0));
        assert_eq!(sel(&e).len(), 3, "no match: the carets stay");
    }

    #[test]
    fn the_next_occurrence_is_added_one_at_a_time() {
        let mut e = Editor::new("one two one three one");
        e.set_selection(1, 1);
        assert!(e.select_next_occurrence(), "an empty selection becomes the word");
        assert_eq!(sel(&e), [(0, 3)]);
        assert!(e.select_next_occurrence());
        assert_eq!(sel(&e), [(0, 3), (8, 11)]);
        assert!(e.select_next_occurrence());
        assert_eq!(sel(&e), [(0, 3), (8, 11), (18, 21)]);
        assert!(!e.select_next_occurrence(), "all of them are taken");
        e.type_text("1");
        assert_eq!(e.text(), "1 two 1 three 1");
        // No word under the caret: nothing happens.
        let mut blank = Editor::new("  ");
        blank.set_selection(1, 1);
        assert!(!blank.select_next_occurrence());
    }

    #[test]
    fn more_matches_than_the_limit_select_nothing() {
        let mut e = Editor::new(&"x".repeat(MAX_MATCHES + 1));
        assert_eq!(e.select_all_matches(&matcher("x")), None);
        assert_eq!(e.carets().len(), 1, "no carets on only the first ones");
        let mut ok = Editor::new(&"x".repeat(MAX_MATCHES));
        assert_eq!(ok.select_all_matches(&matcher("x")), Some(MAX_MATCHES));
    }

    #[test]
    fn a_long_replacement_over_many_matches_is_refused_before_it_is_built() {
        let mut e = Editor::new(&"x".repeat(MAX_MATCHES));
        let long = "y".repeat(4096); // 10,000 of these are 40 MB
        assert_eq!(e.replace_all(&matcher("x"), &long, false), Err(Refused::TooMuchText));
        assert_eq!(e.text(), "x".repeat(MAX_MATCHES), "nothing changed");
        assert_eq!(e.replace_all(&matcher("x"), "yy", false), Ok(MAX_MATCHES));
        let mut many = Editor::new(&"x".repeat(MAX_MATCHES + 1));
        assert_eq!(many.replace_all(&matcher("x"), "y", false), Err(Refused::TooManyMatches));
        assert_eq!(many.text(), "x".repeat(MAX_MATCHES + 1), "nothing changed");
        assert_ne!(Refused::TooManyMatches.to_string(), Refused::TooMuchText.to_string(), "the two refusals say different things");
    }

    #[test]
    fn replacements_take_the_documents_line_ending() {
        let mut e = Editor::new("a\r\nb\r\nc");
        assert_eq!(e.replace_all(&matcher("b"), "1\n2", false), Ok(1));
        assert_eq!(e.text(), "a\r\n1\r\n2\r\nc", "an LF in the replacement is a CRLF in a CRLF document");
        e.set_selection(0, 1);
        assert!(e.replace_next(&matcher("a"), "x\ny"));
        assert!(e.text().starts_with("x\r\ny\r\n1"), "{:?}", e.text());
        let mut lf = Editor::new("a b");
        assert_eq!(lf.replace_all(&matcher("b"), "1\r\n2", false), Ok(1));
        assert_eq!(lf.text(), "a 1\n2", "and the other way round");
    }

    #[test]
    fn line_anchors_work_in_crlf_text_and_see_the_whole_text() {
        let lines = Matcher::new(&Query { pattern: "^x$".into(), regex: true, case_sensitive: true, whole_word: false }).expect("ok");
        assert_eq!(all(&lines, "x\r\nxx\r\nx"), [0..1, 7..8], "`$` is before the CRLF");
        // `$` is the end of a line in the text, not the end of the prefix or of the selection a search is limited to.
        let end = Matcher::new(&Query { pattern: "a$".into(), regex: true, case_sensitive: true, whole_word: false }).expect("ok");
        assert_eq!(end.find_last("ab\nab", 4), None);
        assert_eq!(end.find_all("abab\n", 0..4, 10).0, Vec::<Range<usize>>::new());
        assert_eq!(end.find_all("ab\na\n", 0..5, 10).0, [3..4]);
        let word = Matcher::new(&Query { pattern: "ab".into(), regex: false, case_sensitive: true, whole_word: true }).expect("ok");
        assert_eq!(word.find_all("abc ab", 0..2, 10).0, Vec::<Range<usize>>::new(), "`ab` at the edge of a range is still the start of `abc`");
    }

    #[test]
    fn stepping_does_not_stick_on_an_empty_match() {
        let m = Matcher::new(&Query { pattern: "x*".into(), regex: true, case_sensitive: true, whole_word: false }).expect("ok");
        let mut e = Editor::new("axb");
        e.set_selection(0, 0);
        let mut seen = Vec::new();
        for _ in 0..6 {
            seen.push(e.select_match(&m, Direction::NEXT).map(|r| (r.start, r.end)));
        }
        assert_eq!(seen[0], Some((1, 2)), "from a caret at 0 the next match after the empty one at 0");
        assert!(seen.windows(2).any(|w| w[0] != w[1]), "it moves: {seen:?}");
        e.set_selection(3, 3);
        let before = e.select_match(&m, Direction::PREVIOUS).map(|r| (r.start, r.end));
        assert_ne!(before, Some((3, 3)), "backwards from the end it does not stay at the empty match there");
    }

    #[test]
    fn next_occurrence_finds_a_free_one_between_and_after_the_taken() {
        let mut e = Editor::new("x x x x");
        e.set_selections(&[Selection { anchor: 0, head: 1 }, Selection { anchor: 4, head: 5 }, Selection { anchor: 2, head: 3 }]);
        assert!(e.select_next_occurrence(), "the one at 6 is free, though the next after the primary and the first are taken");
        assert_eq!(sel(&e), [(0, 1), (2, 3), (4, 5), (6, 7)]);
        assert!(!e.select_next_occurrence());
    }

    #[test]
    fn words_include_the_characters_of_xml_names() {
        assert_eq!(word_at("<xs:element name=\"a-b\"/>", 5), Some(1..11));
        assert_eq!(word_at("ab cd", 2), Some(0..2), "at the end of a word is in it");
        assert_eq!(word_at("ab  cd", 3), None);
        assert_eq!(word_at("日本 x", 3), Some(0..6));
    }

    proptest! {
        /// A literal, case-sensitive search finds exactly what `str::match_indices` finds, and replacing everything is `str::replace`.
        #[test]
        fn literal_search_agrees_with_the_standard_library(text in "[abc \\n]{0,60}", needle in "[abc]{1,3}", replacement in "[xyz$]{0,4}") {
            let m = matcher(&needle);
            let expected: Vec<Range<usize>> = text.match_indices(&needle).map(|(i, s)| i..i + s.len()).collect();
            prop_assert_eq!(all(&m, &text), expected);
            let edits = m.replace_all(&text, 0..text.len(), &replacement, 1000).expect("few");
            prop_assert_eq!(xmlcraft_text::apply_edits(&text, &edits), text.replace(&needle, &replacement));
        }

        /// Searching the whole text finds exactly what `Regex::find_iter` does, empty matches included.
        #[test]
        fn find_all_is_the_regex_crates_iteration(
            text in "[ab\\n é]{0,30}",
            pattern in prop::sample::select(vec!["a*", "b", "ab", "a|", "^", "$", "\\b", "a?", "(a|b)*", "é?", "^a|b$", ".", "\\w+", "a*?"]),
            whole_word in prop::bool::ANY,
        ) {
            let m = Matcher::new(&Query { pattern: pattern.to_owned(), regex: true, case_sensitive: true, whole_word }).expect("compiles");
            let expected: Vec<Range<usize>> = m.regex.find_iter(&text).map(|m| m.range()).collect();
            prop_assert_eq!(m.find_all(&text, 0..text.len(), usize::MAX).0, expected);
        }

        #[test]
        fn any_pattern_and_text_are_handled(pattern in "\\PC{0,12}", text in "\\PC{0,40}") {
            for regex in [false, true] {
                if let Ok(m) = Matcher::new(&Query { pattern: pattern.clone(), regex, case_sensitive: false, whole_word: regex }) {
                    let (found, _) = m.find_all(&text, 0..text.len(), 50);
                    prop_assert!(found.iter().all(|r| text.get(r.clone()).is_some()));
                    let _ = (m.find(&text, 0), m.find_last(&text, text.len()));
                }
            }
        }
    }
}
