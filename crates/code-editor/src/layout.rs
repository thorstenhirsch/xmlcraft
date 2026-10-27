//! Laying out one line: tabs become spaces to the next stop, control characters become dots, a very long line is laid out in a
//! window around what is on screen, and positions map between the line's bytes and pixels.

use std::ops::Range;
use std::sync::Arc;

use egui::epaint::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Galley, Painter, Vec2, vec2};
use xmlcraft_editor::motion::char_width;

/// A colour for part of a line, as a byte range of the line's text.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleSpan {
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
        let mut run = String::new();
        let mut run_color = params.color;
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
            if color != run_color && !run.is_empty() {
                job.append(&run, 0.0, TextFormat::simple(params.font.clone(), run_color));
                run.clear();
            }
            run_color = color;
            match c {
                '\t' => {
                    let w = char_width('\t', column, tab);
                    run.extend(std::iter::repeat_n(' ', w));
                    laid_out_chars += w as u32;
                    column += w;
                }
                c if c.is_control() => {
                    run.push('\u{B7}');
                    laid_out_chars += 1;
                }
                c => {
                    run.push(c);
                    laid_out_chars += 1;
                    column += char_width(c, column, tab);
                }
            }
        }
        if !run.is_empty() {
            job.append(&run, 0.0, TextFormat::simple(params.font.clone(), run_color));
        }
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
