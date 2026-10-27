//! The editor's look: the colours of the text of XML, and the bridge from the lexer's classes to the widget's styler.

use egui::Color32;
use std::ops::Range;

use xmlcraft_code_editor::{EditorColors, Highlight, LineStyler, Mark, StyleSpan};
use xmlcraft_editor::find::Matcher;
use xmlcraft_syntax::{HighlightClass, Highlighter, Severity};
use xmlcraft_text::Buffer;

use crate::theme::{ThemeKind, Tokens};

/// The colours of the parts of an XML document. Contrast against the editor background is at least 4.5:1 for everything meant to be read.
#[derive(Clone, Copy, Debug)]
pub struct SyntaxTokens {
    pub punctuation: Color32,
    pub element: Color32,
    pub attribute: Color32,
    pub value: Color32,
    pub reference: Color32,
    pub comment: Color32,
    pub cdata_markup: Color32,
    pub cdata_text: Color32,
    pub processing_instruction: Color32,
    pub declaration: Color32,
    pub literal: Color32,
    pub error: Color32,
    pub warning: Color32,
    pub info: Color32,
    /// The background of a search match.
    pub search_match: Color32,
}

impl SyntaxTokens {
    pub fn for_kind(kind: ThemeKind) -> Self {
        let rgb = |r, g, b| Color32::from_rgb(r, g, b);
        match kind {
            ThemeKind::Light => Self {
                punctuation: rgb(0x6B, 0x6B, 0x73),
                element: rgb(0x17, 0x55, 0xC4),
                attribute: rgb(0x0B, 0x6E, 0x69),
                value: rgb(0x9A, 0x4A, 0x0A),
                reference: rgb(0xB0, 0x1E, 0x55),
                comment: rgb(0x5E, 0x6B, 0x78),
                cdata_markup: rgb(0x6B, 0x6B, 0x73),
                cdata_text: rgb(0x6A, 0x3F, 0x93),
                processing_instruction: rgb(0x7A, 0x3E, 0xA8),
                declaration: rgb(0x7A, 0x3E, 0xA8),
                literal: rgb(0x9A, 0x4A, 0x0A),
                error: rgb(0xC6, 0x28, 0x28),
                warning: rgb(0x9A, 0x62, 0x00),
                info: rgb(0x1B, 0x63, 0xE0),
                search_match: Color32::from_rgba_unmultiplied(0xFF, 0xC8, 0x00, 0x6E),
            },
            ThemeKind::Dark => Self {
                punctuation: rgb(0x9A, 0x9A, 0xA3),
                element: rgb(0x7A, 0xB0, 0xFF),
                attribute: rgb(0x5F, 0xD0, 0xC4),
                value: rgb(0xE5, 0xA9, 0x6E),
                reference: rgb(0xF0, 0x80, 0xA8),
                comment: rgb(0x8A, 0x98, 0xA6),
                cdata_markup: rgb(0x9A, 0x9A, 0xA3),
                cdata_text: rgb(0xC3, 0xA0, 0xE8),
                processing_instruction: rgb(0xC3, 0xA0, 0xE8),
                declaration: rgb(0xC3, 0xA0, 0xE8),
                literal: rgb(0xE5, 0xA9, 0x6E),
                error: rgb(0xFF, 0x7B, 0x7B),
                warning: rgb(0xE5, 0xB0, 0x4A),
                info: rgb(0x7A, 0xB0, 0xFF),
                search_match: Color32::from_rgba_unmultiplied(0xFF, 0xC8, 0x00, 0x4A),
            },
        }
    }

    /// The colour of a problem of this severity.
    pub fn severity(&self, severity: Severity) -> Color32 {
        match severity {
            Severity::Error => self.error,
            Severity::Warning => self.warning,
            Severity::Info => self.info,
        }
    }

    /// The colour for a class, or `None` for plain text.
    pub fn color(&self, class: HighlightClass) -> Option<Color32> {
        Some(match class {
            HighlightClass::Plain => return None,
            HighlightClass::Punctuation => self.punctuation,
            HighlightClass::ElementName => self.element,
            HighlightClass::AttributeName | HighlightClass::DeclarationName => self.attribute,
            HighlightClass::AttributeValue | HighlightClass::DeclarationValue => self.value,
            HighlightClass::Reference => self.reference,
            HighlightClass::Comment => self.comment,
            HighlightClass::CdataMarkup => self.cdata_markup,
            HighlightClass::CdataText => self.cdata_text,
            HighlightClass::ProcessingInstruction => self.processing_instruction,
            HighlightClass::Declaration => self.declaration,
            HighlightClass::Literal => self.literal,
            HighlightClass::Error => self.error,
        })
    }
}

/// The editor's colours from the theme.
pub fn editor_colors(tokens: &Tokens) -> EditorColors {
    let base = match tokens.kind {
        ThemeKind::Light => EditorColors::light(),
        ThemeKind::Dark => EditorColors::dark(),
    };
    EditorColors {
        background: tokens.card,
        text: tokens.text,
        caret: tokens.accent,
        gutter_background: tokens.panel,
        gutter_border: tokens.divider,
        ..base
    }
}

/// Colours XML for the editor widget: the lexer's classes, mapped through [`SyntaxTokens`].
pub struct XmlStyler {
    highlighter: Highlighter,
    tokens: SyntaxTokens,
    /// The problems to underline: byte ranges of the whole text, sorted by start.
    problems: Vec<(Range<usize>, Severity)>,
    /// For each problem, how far any problem up to it reaches (the largest end so far, at least one past its start): it never goes
    /// down, so the problems that can touch a line are found by searching, not by looking at every earlier one.
    reach: Vec<usize>,
    /// What the find bar looks for: every match on a visible line is highlighted.
    search: Option<Matcher>,
}

impl XmlStyler {
    pub fn new(kind: ThemeKind) -> Self {
        Self { highlighter: Highlighter::new(), tokens: SyntaxTokens::for_kind(kind), problems: Vec::new(), reach: Vec::new(), search: None }
    }

    /// Switch to another theme's colours. The caller bumps the view's `highlight_version` so cached lines are laid out again.
    pub fn set_theme(&mut self, kind: ThemeKind) {
        self.tokens = SyntaxTokens::for_kind(kind);
    }

    /// Start over (another document was loaded).
    pub fn reset(&mut self) {
        self.highlighter.reset();
        self.problems.clear();
        self.reach.clear();
    }

    /// Highlight the matches of this search (none: no highlights).
    pub fn set_search(&mut self, search: Option<Matcher>) {
        self.search = search;
    }

    /// The problems to underline (replacing the last ones). Ranges are in bytes of the whole text; they may be empty (a problem at a
    /// point).
    pub fn set_problems(&mut self, mut problems: Vec<(Range<usize>, Severity)>) {
        problems.sort_by_key(|(range, _)| (range.start, range.end));
        let mut farthest = 0;
        self.reach = problems
            .iter()
            .map(|(range, _)| {
                farthest = farthest.max(range.end.max(range.start.saturating_add(1)));
                farthest
            })
            .collect();
        self.problems = problems;
    }
}

impl LineStyler for XmlStyler {
    fn text_changed(&mut self, first_line: usize) {
        self.highlighter.invalidate_from(first_line);
    }

    fn style_line(&mut self, buffer: &Buffer, line: usize, _text: &str) -> Vec<StyleSpan> {
        self.highlighter
            .spans(buffer, line)
            .into_iter()
            .filter_map(|(range, class)| self.tokens.color(class).map(|color| StyleSpan { range, color }))
            .collect()
    }

    fn highlights(&mut self, buffer: &Buffer, line: usize) -> Vec<Highlight> {
        let Some(matcher) = &self.search else { return Vec::new() };
        let Some(text) = buffer.line_text(line) else { return Vec::new() };
        let color = self.tokens.search_match;
        // A match across lines is not shown (each line is searched alone); at most a few hundred per line.
        matcher.find_all(&text, 0..text.len(), 300).0.into_iter().filter(|r| !r.is_empty()).map(|range| Highlight { range, color }).collect()
    }

    fn marks(&mut self, buffer: &Buffer, line: usize) -> Vec<Mark> {
        let Some(range) = buffer.line_range(line) else { return Vec::new() };
        let text_len = buffer.len();
        // Problems are sorted by start: those that start after the line's end cannot touch it, and neither can the ones before the first
        // that reaches into it.
        let end = self.problems.partition_point(|(r, _)| r.start <= range.end);
        let first = self.reach.partition_point(|reach| *reach <= range.start);
        self.problems
            .get(first..end)
            .unwrap_or_default()
            .iter()
            .filter(|(r, _)| r.end.max(r.start + 1) > range.start && r.start <= text_len)
            .map(|(r, severity)| {
                let start = r.start.max(range.start) - range.start;
                let finish = r.end.min(range.end).max(r.start.max(range.start)) - range.start;
                Mark { range: start..finish.max(start), color: self.tokens.severity(*severity) }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance and contrast ratio.
    fn luminance(c: Color32) -> f64 {
        let channel = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn every_class_that_is_read_has_enough_contrast_in_both_themes() {
        use HighlightClass::*;
        for kind in [ThemeKind::Light, ThemeKind::Dark] {
            let tokens = Tokens::for_kind(kind);
            let background = editor_colors(&tokens).background;
            let syntax = SyntaxTokens::for_kind(kind);
            for class in [
                Punctuation,
                ElementName,
                AttributeName,
                AttributeValue,
                Reference,
                Comment,
                CdataMarkup,
                CdataText,
                ProcessingInstruction,
                Declaration,
                Literal,
                Error,
            ] {
                let color = syntax.color(class).expect("coloured");
                let ratio = contrast(color, background);
                assert!(ratio >= 4.5, "{class:?} in {kind:?}: {ratio:.2}:1");
            }
            assert_eq!(syntax.color(Plain), None);
            for severity in [Severity::Error, Severity::Warning, Severity::Info] {
                let ratio = contrast(syntax.severity(severity), background);
                assert!(ratio >= 4.5, "{severity:?} in {kind:?}: {ratio:.2}:1");
            }
        }
    }

    #[test]
    fn the_styler_colours_lines_and_forgets_on_change() {
        let buffer = Buffer::from_text("<a b=\"1\">\n<!-- c\n-->");
        let mut styler = XmlStyler::new(ThemeKind::Light);
        let tokens = SyntaxTokens::for_kind(ThemeKind::Light);
        let first = styler.style_line(&buffer, 0, "");
        assert_eq!(first.first().map(|s| s.color), Some(tokens.punctuation));
        assert!(first.iter().any(|s| s.color == tokens.element) && first.iter().any(|s| s.color == tokens.value));
        // The comment started on line 1 runs on to line 2.
        let third = styler.style_line(&buffer, 2, "");
        assert_eq!(third, vec![StyleSpan { range: 0..3, color: tokens.comment }]);
        // After an edit that turns the comment opener into text, line 2 is plain markup again.
        let edited = Buffer::from_text("<a b=\"1\">\nx\n-->");
        styler.text_changed(1);
        let third = styler.style_line(&edited, 2, "");
        assert!(third.is_empty(), "`-->` outside a comment is plain text, not colour: {third:?}");
    }

    #[test]
    fn marks_found_by_search_are_the_ones_a_look_at_every_problem_finds() {
        // Many problems of every length, some far longer than a line: the search must not miss one that starts early and reaches far.
        let text: String = (0..300).map(|i| format!("<line n=\"{i}\"/>\n")).collect();
        let buffer = Buffer::from_text(&text);
        let len = text.len();
        let mut problems = Vec::new();
        let mut seed = 12345u64;
        for _ in 0..2000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let start = (seed >> 33) as usize % len;
            let length = match (seed >> 20) % 5 {
                0 => 0,
                1 => 3,
                2 => 40,
                3 => 900,
                _ => 6000,
            };
            problems.push((start..(start + length).min(len), if seed & 1 == 0 { Severity::Error } else { Severity::Warning }));
        }
        let mut styler = XmlStyler::new(ThemeKind::Light);
        styler.set_problems(problems.clone());
        for line in 0..buffer.line_count() {
            let range = buffer.line_range(line).expect("a line");
            let mut expected: Vec<Range<usize>> = problems
                .iter()
                .filter(|(r, _)| r.start <= range.end && r.end.max(r.start + 1) > range.start)
                .map(|(r, _)| {
                    let start = r.start.max(range.start) - range.start;
                    start..(r.end.min(range.end).max(r.start.max(range.start)) - range.start).max(start)
                })
                .collect();
            let mut found: Vec<Range<usize>> = styler.marks(&buffer, line).into_iter().map(|m| m.range).collect();
            expected.sort_by_key(|r| (r.start, r.end));
            found.sort_by_key(|r| (r.start, r.end));
            assert_eq!(found, expected, "line {line}");
        }
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)] // a list of ranges with one range in it is what is being compared
    fn search_matches_are_highlighted_on_the_lines_that_have_them() {
        use xmlcraft_editor::find::{Matcher, Query};
        let buffer = Buffer::from_text("<a b=\"ab\">\nxx\nab ab\n");
        let mut styler = XmlStyler::new(ThemeKind::Light);
        fn ranges(styler: &mut XmlStyler, buffer: &Buffer, line: usize) -> Vec<Range<usize>> {
            styler.highlights(buffer, line).into_iter().map(|h| h.range).collect()
        }
        assert!(ranges(&mut styler, &buffer, 0).is_empty(), "no search, no highlights");
        styler.set_search(Matcher::new(&Query::text("ab")).ok());
        assert_eq!(ranges(&mut styler, &buffer, 0), [6..8]);
        assert!(ranges(&mut styler, &buffer, 1).is_empty());
        assert_eq!(ranges(&mut styler, &buffer, 2), [0..2, 3..5], "ranges are bytes of the line, not of the text");
        assert!(ranges(&mut styler, &buffer, 99).is_empty(), "a line that does not exist has none");
        let color = styler.highlights(&buffer, 2)[0].color;
        assert_eq!(color, SyntaxTokens::for_kind(ThemeKind::Light).search_match);
        // An empty match (a pattern like `x*`) is not drawn.
        styler.set_search(Matcher::new(&Query { pattern: "x*".into(), regex: true, case_sensitive: true, whole_word: false }).ok());
        assert_eq!(ranges(&mut styler, &buffer, 1), [0..2]);
        assert!(ranges(&mut styler, &buffer, 0).is_empty());
        styler.set_search(None);
        assert!(ranges(&mut styler, &buffer, 2).is_empty());
    }

    #[test]
    fn problems_become_marks_on_the_lines_they_touch() {
        let buffer = Buffer::from_text("<a>\n<b>text</b>\n</a>");
        let mut styler = XmlStyler::new(ThemeKind::Light);
        styler.set_problems(vec![(8..12, Severity::Warning), (0..0, Severity::Error), (2..9, Severity::Error)]);
        let tokens = SyntaxTokens::for_kind(ThemeKind::Light);
        let first = styler.marks(&buffer, 0);
        assert_eq!(first.iter().map(|m| m.range.clone()).collect::<Vec<_>>(), [0..0, 2..3], "a point, and the start of a long one");
        assert_eq!(first[1].color, tokens.error);
        let second = styler.marks(&buffer, 1);
        // Line 1 is bytes 4..15 (the line break is not part of it): the long problem (2..9) covers its first five bytes; the warning (8..12) the next four.
        assert_eq!(second.iter().map(|m| m.range.clone()).collect::<Vec<_>>(), [0..5, 4..8]);
        assert!(styler.marks(&buffer, 2).is_empty());
        assert!(styler.marks(&buffer, 99).is_empty(), "a line that does not exist has no marks");
    }
}
