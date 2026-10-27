//! The editor's look: the colours of the text of XML, and the bridge from the lexer's classes to the widget's styler.

use egui::Color32;
use xmlcraft_code_editor::{EditorColors, LineStyler, StyleSpan};
use xmlcraft_syntax::{HighlightClass, Highlighter};
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
            },
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
}

impl XmlStyler {
    pub fn new(kind: ThemeKind) -> Self {
        Self { highlighter: Highlighter::new(), tokens: SyntaxTokens::for_kind(kind) }
    }

    /// Switch to another theme's colours. The caller bumps the view's `highlight_version` so cached lines are laid out again.
    pub fn set_theme(&mut self, kind: ThemeKind) {
        self.tokens = SyntaxTokens::for_kind(kind);
    }

    /// Start over (another document was loaded).
    pub fn reset(&mut self) {
        self.highlighter.reset();
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
}
