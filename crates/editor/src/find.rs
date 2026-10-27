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

    /// The last match that ends at or before byte `before`.
    pub fn find_last(&self, text: &str, before: usize) -> Option<Range<usize>> {
        let end = floor_boundary(text, before);
        let head = text.get(..end)?;
        // Matches do not overlap, so the last one before `end` is the last of those found from the start. (A regex cannot search backwards.)
        self.regex.find_iter(head).last().map(|m| m.range())
    }

    /// Every match inside `within` (at most `limit`), and whether there were more. An empty match (a pattern such as `a*`) counts.
    pub fn find_all(&self, text: &str, within: Range<usize>, limit: usize) -> (Vec<Range<usize>>, bool) {
        let end = floor_boundary(text, within.end);
        let start = ceil_boundary(text, within.start);
        let mut found = Vec::new();
        let Some(haystack) = text.get(..end) else { return (found, false) };
        for m in self.regex.find_iter(haystack).skip_while(|m| m.start() < start) {
            if found.len() >= limit {
                return (found, true);
            }
            found.push(m.range());
        }
        (found, false)
    }

    /// What `replacement` stands for at `found` (a match of this matcher in `text`): in a regular-expression search `$1` and `${name}`
    /// are the groups (`$$` is a dollar sign); otherwise the replacement is taken as it is.
    pub fn expand(&self, text: &str, found: &Range<usize>, replacement: &str) -> String {
        if !self.groups {
            return replacement.to_owned();
        }
        let Some(captures) = self.regex.captures_at(text.get(..found.end).unwrap_or(text), found.start) else { return replacement.to_owned() };
        let mut out = String::new();
        captures.expand(replacement, &mut out);
        out
    }

    /// The edits that replace every match inside `within` (at most `limit`; more is `None`: better none than half).
    pub fn replace_all(&self, text: &str, within: Range<usize>, replacement: &str, limit: usize) -> Option<Vec<Edit>> {
        let (matches, more) = self.find_all(text, within, limit);
        if more {
            return None;
        }
        Some(matches.into_iter().map(|range| Edit::replace(range.clone(), self.expand(text, &range, replacement))).collect())
    }
}

/// How many matches select-all and replace-all will deal with at once.
pub const MAX_MATCHES: usize = 10_000;

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
            matcher.find_last(&text, start).or_else(|| if direction.wrap { matcher.find_last(&text, text.len()) } else { None })
        } else {
            // After the selection; an empty match at the caret is skipped, or the search would stay where it is.
            let found = matcher.find(&text, end).filter(|m| !(m.is_empty() && m.start == end && start == end && end > 0));
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
            let with = matcher.expand(&text, &selected, replacement);
            replaced = self.apply_edits(&[Edit::replace(selected.clone(), with)]);
            // Carried through the edit, the caret is at the end of what was written.
        }
        self.select_match(matcher, Direction::NEXT);
        replaced
    }

    /// Replace every match (in the primary selection when `in_selection` and it is not empty) as one undoable step. Returns how many were
    /// replaced, or `None` when there are more than [`MAX_MATCHES`] (nothing is changed then).
    pub fn replace_all(&mut self, matcher: &Matcher, replacement: &str, in_selection: bool) -> Option<usize> {
        let text = self.text();
        let selection = self.primary();
        let within = if in_selection && !selection.is_empty() { selection.start()..selection.end() } else { 0..text.len() };
        let edits = matcher.replace_all(&text, within, replacement, MAX_MATCHES)?;
        let count = edits.len();
        if count > 0 {
            self.apply_edits(&edits);
        }
        Some(count)
    }

    /// Put a caret on every match, selecting each (at most [`MAX_MATCHES`]); the last is the primary. Returns how many.
    pub fn select_all_matches(&mut self, matcher: &Matcher) -> usize {
        let text = self.text();
        let (found, _) = matcher.find_all(&text, 0..text.len(), MAX_MATCHES);
        if found.is_empty() {
            return 0;
        }
        let selections: Vec<Selection> = found.iter().map(|r| Selection { anchor: r.start, head: r.end }).collect();
        self.set_selections(&selections);
        selections.len()
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
        let candidates = (matcher.find(&text, primary.end()), matcher.find(&text, 0));
        let Some(found) = [candidates.0, candidates.1].into_iter().flatten().find(|m| !taken.iter().any(|s| s.start() < m.end && m.start < s.end()))
        else {
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
        assert_eq!(m.replace_all(text, 0..8, "X", 2), None, "too many to replace safely");
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
        assert_eq!(e.replace_all(&m, "Q", true), Some(1), "only inside the selection: `X-a`");
        assert_eq!(e.text(), "X-Q-a");
        assert_eq!(e.replace_all(&m, "Q", false), Some(1));
        assert_eq!(e.text(), "X-Q-Q");
        e.undo();
        assert_eq!(e.text(), "X-Q-a", "replace all is one step too");
        assert_eq!(e.replace_all(&matcher("nothing"), "Q", false), Some(0));
    }

    #[test]
    fn regular_expression_replacements_use_groups() {
        let mut e = Editor::new("<a x=\"1\"/><b x=\"2\"/>");
        let m = Matcher::new(&Query { pattern: r#"x="(\d)""#.into(), regex: true, case_sensitive: true, whole_word: false }).expect("ok");
        assert_eq!(e.replace_all(&m, "n=\"$1$1\"", false), Some(2));
        assert_eq!(e.text(), "<a n=\"11\"/><b n=\"22\"/>");
    }

    #[test]
    fn select_all_matches_makes_a_caret_for_each() {
        let mut e = Editor::new("ab ab ab");
        assert_eq!(e.select_all_matches(&matcher("ab")), 3);
        assert_eq!(sel(&e), [(0, 2), (3, 5), (6, 8)]);
        e.type_text("X");
        assert_eq!(e.text(), "X X X", "typing replaces every selection at once");
        assert_eq!(e.select_all_matches(&matcher("nope")), 0);
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
