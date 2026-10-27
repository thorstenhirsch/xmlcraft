//! Laying out one line: tabs become spaces to the next stop, control characters become dots, a very long line is laid out in a
//! window around what is on screen, and positions map between the line's bytes and pixels.

use std::ops::Range;
use std::sync::Arc;

use egui::epaint::text::{ByteIndex, LayoutJob, LayoutSection, TextFormat};
use egui::{Color32, FontId, Galley, Painter, Vec2, vec2};
use xmlcraft_editor::motion::char_width;

/// A colour for part of a line, as a byte range of the line's text.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleSpan {
    pub range: Range<usize>,
    pub color: Color32,
}

/// A wavy underline under part of a line, as a byte range of the line's text (a problem the host found).
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub range: Range<usize>,
    pub color: Color32,
}

/// A filled background behind part of a line, as a byte range of the line's text (a search match the host wants to show).
#[derive(Clone, Debug, PartialEq)]
pub struct Highlight {
    pub range: Range<usize>,
    pub color: Color32,
}

/// Supplies the colours of the text, line by line. The host implements it for its language.
pub trait LineStyler {
    /// The text changed. Lines before `first_line` (0-based) are as they were, so whatever was worked out for them is still good.
    fn text_changed(&mut self, first_line: usize);

    /// The coloured parts of line `line` (its text without the line break), as byte ranges of that text. `buffer` is the whole text,
    /// for a styler that needs to look at the lines before.
    fn style_line(&mut self, buffer: &xmlcraft_text::Buffer, line: usize, text: &str) -> Vec<StyleSpan>;

    /// The wavy underlines of line `line`, as byte ranges of its text. Asked for every visible line on every frame, so it must be cheap.
    fn marks(&mut self, _buffer: &xmlcraft_text::Buffer, _line: usize) -> Vec<Mark> {
        Vec::new()
    }

    /// The backgrounds of line `line`, as byte ranges of its text, drawn behind the text and the selection. Asked for every visible line
    /// on every frame, so it must be cheap.
    fn highlights(&mut self, _buffer: &xmlcraft_text::Buffer, _line: usize) -> Vec<Highlight> {
        Vec::new()
    }

    /// Whether the lines after `line` can be folded into it (and how far the fold goes): the gutter shows a mark on `line` when they
    /// can. Asked for every visible line on every frame, so it must be cheap.
    fn fold_at(&mut self, _buffer: &xmlcraft_text::Buffer, _line: usize) -> Option<FoldRegion> {
        None
    }
}

/// What can be folded into a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldRegion {
    /// The last line (0-based) that is hidden when the lines after the one that stands for it are folded.
    pub last_line: usize,
}

/// Lines longer than this (in characters) are laid out in a window of this many characters.
pub const MAX_WINDOW: usize = 4096;

/// One laid-out line (or the window of a long line).
pub struct LineLayout {
    galley: Arc<Galley>,
    /// The first character of the line that is in the layout.
    first_char: usize,
    /// For each character of the window and one past the last: its index in the text that was laid out (tabs are several spaces).
    laid_out_index: Vec<u32>,
    /// For each character of the window and one past the last: its byte offset in the line.
    byte_of_char: Vec<u32>,
    /// Where the window starts, in pixels from the line's start.
    x_origin: f32,
}

/// Whether the font may draw `a` followed by `b` as one joined shape (a ligature): any two ASCII punctuation characters.
fn joins(a: char, b: char) -> bool {
    a.is_ascii_punctuation() && b.is_ascii_punctuation()
}

fn section(start: usize, end: usize, font: &FontId, color: Color32) -> LayoutSection {
    LayoutSection { leading_space: 0.0, byte_range: ByteIndex(start)..ByteIndex(end), format: TextFormat::simple(font.clone(), color) }
}

pub struct LayoutParams<'a> {
    pub font: &'a FontId,
    pub color: Color32,
    pub tab_width: usize,
    /// The width of one column (the font is monospaced).
    pub column_width: f32,
    pub spans: &'a [StyleSpan],
}

impl LineLayout {
    /// Lay out `text` (a line without its break), or the characters `window` of it.
    pub fn new(painter: &Painter, text: &str, window: Range<usize>, params: &LayoutParams<'_>) -> Self {
        let tab = params.tab_width.max(1);
        // The column the window starts at, so that tabs in it stop where they would in the whole line.
        let mut column = 0usize;
        for c in text.chars().take(window.start) {
            column += char_width(c, column, tab);
        }
        let x_origin = column as f32 * params.column_width;

        let mut job = LayoutJob::default();
        job.wrap.max_width = f32::INFINITY;
        job.break_on_newline = false;
        let mut laid_out_index = Vec::new();
        let mut byte_of_char = Vec::new();
        let mut text_out = String::new();
        // The laid-out text is cut into sections: a change of colour starts one, and so does every pair of adjacent punctuation
        // characters, because the font joins some of them into one shape (`--`, `->`, `<!--`) and a document must show what is there.
        // Shaping is done per section, so a boundary stops the joining.
        let mut sections: Vec<LayoutSection> = Vec::new();
        let mut section_start = 0usize;
        let mut section_color = params.color;
        let mut previous: Option<char> = None;
        let mut laid_out_chars = 0u32;
        let mut spans = params.spans.iter().peekable();
        let width = window.end.saturating_sub(window.start);
        let mut end_byte = text.len();
        for (taken, (byte, c)) in text.char_indices().skip(window.start).enumerate() {
            if taken == width {
                end_byte = byte;
                break;
            }
            laid_out_index.push(laid_out_chars);
            byte_of_char.push(byte as u32);
            while spans.peek().is_some_and(|s| s.range.end <= byte) {
                spans.next();
            }
            let color = spans.peek().filter(|s| s.range.start <= byte).map_or(params.color, |s| s.color);
            if text_out.len() > section_start && (color != section_color || previous.is_some_and(|p| joins(p, c))) {
                sections.push(section(section_start, text_out.len(), params.font, section_color));
                section_start = text_out.len();
            }
            section_color = color;
            previous = Some(c);
            match c {
                '\t' => {
                    let w = char_width('\t', column, tab);
                    text_out.extend(std::iter::repeat_n(' ', w));
                    laid_out_chars += w as u32;
                    column += w;
                }
                c if c.is_control() => {
                    text_out.push('\u{B7}');
                    laid_out_chars += 1;
                    column += char_width(c, column, tab);
                }
                c => {
                    text_out.push(c);
                    laid_out_chars += 1;
                    column += char_width(c, column, tab);
                }
            }
        }
        if text_out.len() > section_start {
            sections.push(section(section_start, text_out.len(), params.font, section_color));
        }
        job.text = text_out;
        job.sections = sections;
        laid_out_index.push(laid_out_chars);
        byte_of_char.push(end_byte as u32);
        let galley = painter.layout_job(job);
        Self { galley, first_char: window.start, laid_out_index, byte_of_char, x_origin }
    }

    pub fn galley(&self) -> &Arc<Galley> {
        &self.galley
    }

    pub fn first_char(&self) -> usize {
        self.first_char
    }

    /// Where the laid-out text starts, in pixels from the start of the line.
    pub fn x_origin(&self) -> f32 {
        self.x_origin
    }

    pub fn size(&self) -> Vec2 {
        vec2(self.galley.size().x, self.galley.size().y)
    }

    /// The window's last byte offset in the line.
    pub fn end_byte(&self) -> usize {
        self.byte_of_char.last().map_or(0, |b| *b as usize)
    }

    /// The window's first byte offset in the line.
    pub fn start_byte(&self) -> usize {
        self.byte_of_char.first().map_or(0, |b| *b as usize)
    }

    /// `range` (bytes of the line) cut to the window, or `None` when none of it is in the window: a long line is laid out in a window,
    /// and what is outside it has no place on the screen.
    pub fn in_window(&self, range: &std::ops::Range<usize>) -> Option<std::ops::Range<usize>> {
        let (start, end) = (range.start.max(self.start_byte()), range.end.min(self.end_byte()));
        (start < end || (range.is_empty() && range.start >= self.start_byte() && range.start <= self.end_byte())).then_some(start..end.max(start))
    }

    /// How many characters the window holds.
    pub fn chars(&self) -> usize {
        self.byte_of_char.len().saturating_sub(1)
    }

    /// The x position (pixels from the line's start) of the byte offset `byte` in the line; offsets outside the window are clamped to
    /// its edges.
    pub fn x_of_byte(&self, byte: usize) -> f32 {
        let index = self.byte_of_char.partition_point(|b| (*b as usize) <= byte).saturating_sub(1);
        let laid_out = self.laid_out_index.get(index).copied().unwrap_or(0) as usize;
        self.x_origin + self.galley.pos_from_cursor(egui::text::CCursor::new(laid_out)).min.x
    }

    /// The byte offset in the line nearest to the x position `x` (pixels from the line's start), on a character boundary.
    pub fn byte_at_x(&self, x: f32) -> usize {
        let cursor = self.galley.cursor_from_pos(vec2(x - self.x_origin, 0.0));
        let target = cursor.index.0 as u32;
        // The character whose laid-out span contains `target`; inside a tab, the nearer edge.
        let after = self.laid_out_index.partition_point(|i| *i <= target);
        let index = after.saturating_sub(1).min(self.chars());
        let start = self.laid_out_index.get(index).copied().unwrap_or(0);
        let end = self.laid_out_index.get(index + 1).copied().unwrap_or(start);
        let nearest = if target.saturating_sub(start) * 2 > end.saturating_sub(start) { index + 1 } else { index };
        self.byte_of_char.get(nearest.min(self.chars())).map_or(0, |b| *b as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lay out `text` in a bare egui context and return the laid-out job's sections as text pieces.
    fn pieces(text: &str, spans: &[StyleSpan]) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut result = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let font = FontId::monospace(13.0);
            let params = LayoutParams { font: &font, color: Color32::BLACK, tab_width: 4, column_width: 8.0, spans };
            let layout = LineLayout::new(ui.painter(), text, 0..text.chars().count(), &params);
            let job = &layout.galley().job;
            result = job.sections.iter().map(|s| job.text[s.byte_range.start.0..s.byte_range.end.0].to_owned()).collect();
        });
        output.textures_delta.clear();
        result
    }

    #[test]
    fn adjacent_punctuation_is_never_shaped_together() {
        assert_eq!(
            pieces("<!-- a -->", &[]),
            ["<", "!", "-", "- a -", "-", ">"],
            "so `<!--` and `-->` are drawn as the characters they are (a space already separates the rest)"
        );
        assert_eq!(pieces("a->b", &[]), ["a-", ">b"], "a letter and a dash may stay together; two punctuation marks may not");
        assert_eq!(pieces("ab cd", &[]), ["ab cd"], "ordinary text stays one section");
        assert_eq!(pieces("", &[]), Vec::<String>::new());
    }

    #[test]
    fn a_change_of_colour_starts_a_section_and_a_tab_becomes_spaces() {
        let spans = [StyleSpan { range: 1..3, color: Color32::RED }];
        // The tab is at column 1, so it fills three columns; the red part is the tab and `b`.
        assert_eq!(pieces("a\tb c", &spans), ["a", "   b", " c"]);
    }

    #[test]
    fn positions_map_back_through_tabs_and_wide_text() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let font = FontId::monospace(13.0);
            let params = LayoutParams { font: &font, color: Color32::BLACK, tab_width: 4, column_width: 8.0, spans: &[] };
            let text = "a\tb";
            let layout = LineLayout::new(ui.painter(), text, 0..3, &params);
            // `b` is after the tab, which fills the line to column 4: three laid-out columns beyond `a`'s one.
            assert!(layout.x_of_byte(3) > layout.x_of_byte(2), "b is right of the tab");
            assert!(layout.x_of_byte(2) > layout.x_of_byte(1), "the tab has width");
            // Clicking inside the tab goes to its nearer edge.
            let tab_start = layout.x_of_byte(1);
            let tab_end = layout.x_of_byte(2);
            assert_eq!(layout.byte_at_x(tab_start + 1.0), 1);
            assert_eq!(layout.byte_at_x(tab_end - 1.0), 2);
            assert_eq!(layout.byte_at_x(10_000.0), 3, "past the end: the end");
        });
        output.textures_delta.clear();
    }

    /// Lay `text` out headless (the first frame of a throwaway context has the default fonts).
    fn layout(text: &str) -> LineLayout {
        let ctx = egui::Context::default();
        let mut out = None;
        let font = FontId::monospace(13.0);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let params = LayoutParams { font: &font, color: Color32::BLACK, tab_width: 4, column_width: 8.0, spans: &[] };
            out = Some(LineLayout::new(ui.painter(), text, 0..text.chars().count(), &params));
        });
        output.textures_delta.clear(); // nothing is painted
        out.expect("the pass ran")
    }

    #[test]
    fn a_control_character_takes_a_column_so_the_tab_after_it_stops_where_the_editor_expects() {
        // One dot, then a tab to column 4 (three spaces), then `x`.
        let layout = layout("\u{1}\tx");
        assert_eq!(layout.laid_out_index, [0, 1, 4, 5]);
    }

    #[test]
    fn only_what_is_in_the_laid_out_window_has_a_place_on_the_screen() {
        let ctx = egui::Context::default();
        let mut found = Vec::new();
        let font = FontId::monospace(13.0);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let text = "a".repeat(100);
            let params = LayoutParams { font: &font, color: Color32::BLACK, tab_width: 4, column_width: 8.0, spans: &[] };
            // A long line is laid out in a window: here characters 20 to 60.
            let layout = LineLayout::new(ui.painter(), &text, 20..60, &params);
            for range in [0..10, 10..30, 30..40, 50..90, 70..80, 20..20, 30..30, 10..10, 60..60, 61..61] {
                found.push((range.clone(), layout.in_window(&range)));
            }
        });
        output.textures_delta.clear();
        let expected: Vec<(Range<usize>, Option<Range<usize>>)> = vec![
            (0..10, None),
            (10..30, Some(20..30)),
            (30..40, Some(30..40)),
            (50..90, Some(50..60)),
            (70..80, None),
            (20..20, Some(20..20)),
            (30..30, Some(30..30)),
            (10..10, None),
            (60..60, Some(60..60)),
            (61..61, None),
        ];
        assert_eq!(found, expected);
    }

    #[test]
    fn tabs_and_wide_characters_in_the_layout() {
        let layout = layout("a\t日\tb");
        // `a` is column 0; the tab fills columns 1..4; the wide character takes 4..6; the next tab goes to 8; `b` is column 8.
        assert_eq!(layout.laid_out_index, [0, 1, 4, 5, 7, 8]);
    }
}
