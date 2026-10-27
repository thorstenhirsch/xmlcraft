//! The widget: drawing, scrolling and input.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use egui::{
    Align2, Color32, CursorIcon, Event, EventFilter, FontId, Id, ImeEvent, Key, Modifiers, Pos2, Rect, Response, ScrollArea, Sense, Stroke, Ui,
    WidgetInfo, WidgetType, pos2, vec2,
};
use xmlcraft_editor::Editor;
use xmlcraft_editor::motion::snap_to_cluster;
use xmlcraft_text::Buffer;

use crate::colors::EditorColors;
use crate::keys::{Command, command_for};
use crate::layout::{LayoutParams, LineLayout, LineStyler, MAX_WINDOW, StyleSpan};

/// Space on the left and right of the line numbers, in points.
const GUTTER_PADDING: f32 = 10.0;
/// Space between the text and the gutter, in points.
const TEXT_MARGIN: f32 = 8.0;
/// Extra blank lines below the last, so the end of the document can be scrolled up.
const BOTTOM_LINES: f32 = 3.0;

/// Text being composed through an input method: shown at the caret, not yet part of the document.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Preedit {
    text: String,
    /// The part of it the input method highlights, in characters.
    active: Option<Range<usize>>,
}

#[derive(Clone, Copy, PartialEq)]
struct CacheKey {
    version: u64,
    highlight: u64,
    font_size: u32,
    tab_width: usize,
    color: Color32,
}

struct Cached {
    key: CacheKey,
    layout: Arc<LineLayout>,
    chars: usize,
    frame: u64,
}

/// Where things were on the screen in the last frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    /// The screen position of the start of the document (scrolling moves it).
    pub origin: Pos2,
    /// The screen x of the start of the text, after the gutter.
    pub text_x: f32,
    pub row_height: f32,
    pub column_width: f32,
}

impl Geometry {
    /// The screen position of the left end of the top of row `row` (counting from the first line of the document) at display column
    /// `column`. Exact for text without wide characters.
    pub fn point(&self, row: usize, column: usize) -> Pos2 {
        pos2(self.text_x + column as f32 * self.column_width, self.origin.y + row as f32 * self.row_height)
    }
}

/// What the view keeps between frames. One per document, next to its [`Editor`].
#[derive(Default)]
pub struct ViewState {
    geometry: Option<Geometry>,
    cache: HashMap<usize, Cached>,
    frame: u64,
    /// The widest line seen, in columns: sets the horizontal scroll range. It only grows until [`Self::reset`].
    max_columns: usize,
    max_columns_known_for: Option<u64>,
    preedit: Option<Preedit>,
    /// When the last key or click happened, so the caret does not blink while it is being used.
    last_input: f64,
    /// A line to scroll to the top next time the editor is drawn.
    scroll_to_line: Option<usize>,
    /// How many lines a page is (updated every frame from the size of the view).
    page_lines: usize,
    /// Bump this when the host's colouring changes, so cached lines are laid out again.
    pub highlight_version: u64,
    /// The editor's version when the styler was last told about changes.
    styled_version: u64,
}

impl ViewState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget everything about the text (after loading another file). Also call it if the host changes the fonts: laid-out lines belong
    /// to the fonts they were laid out with.
    pub fn reset(&mut self) {
        self.cache.clear();
        self.max_columns = 0;
        self.max_columns_known_for = None;
        self.preedit = None;
    }

    /// Scroll so that `line` (0-based) is at the top, the next time the editor is drawn.
    pub fn scroll_to_line(&mut self, line: usize) {
        self.scroll_to_line = Some(line);
    }

    /// Where things were on the screen in the last frame, once there has been one.
    pub fn geometry(&self) -> Option<Geometry> {
        self.geometry
    }

    /// How many lines were laid out in the last frame: the lines on screen, not the lines in the document.
    pub fn laid_out_lines(&self) -> usize {
        self.cache.len()
    }

    /// Whether an input method is composing text at the caret.
    pub fn is_composing(&self) -> bool {
        self.preedit.is_some()
    }
}

/// What a frame of the editor did.
pub struct CodeEditorOutput {
    pub response: Response,
    /// The text changed during this frame (typing, a command, undo).
    pub changed: bool,
    /// The carets moved or the selection changed during this frame.
    pub selection_changed: bool,
}

/// The editor widget. `show` it every frame.
pub struct CodeEditor<'a> {
    editor: &'a mut Editor,
    view: &'a mut ViewState,
    colors: EditorColors,
    id_salt: Id,
    font: Option<FontId>,
    line_height: f32,
    blink: bool,
    gutter: bool,
    read_only: bool,
    styler: Option<&'a mut dyn LineStyler>,
}

impl<'a> CodeEditor<'a> {
    pub fn new(id_salt: Id, editor: &'a mut Editor, view: &'a mut ViewState, colors: EditorColors) -> Self {
        Self { editor, view, colors, id_salt, font: None, line_height: 1.5, blink: true, gutter: true, read_only: false, styler: None }
    }

    /// The font (monospaced). The default is the style's monospace font.
    pub fn font(mut self, font: FontId) -> Self {
        self.font = Some(font);
        self
    }

    /// Row height as a multiple of the font size.
    pub fn line_height(mut self, factor: f32) -> Self {
        self.line_height = factor.clamp(1.0, 3.0);
        self
    }

    /// Whether the caret blinks (tests turn it off).
    pub fn blink(mut self, blink: bool) -> Self {
        self.blink = blink;
        self
    }

    pub fn gutter(mut self, show: bool) -> Self {
        self.gutter = show;
        self
    }

    /// No typing or deleting; moving, selecting and copying still work.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Colours for the text, line by line (the host's lexer or parser).
    pub fn styler(mut self, styler: &'a mut dyn LineStyler) -> Self {
        self.styler = Some(styler);
        self
    }

    pub fn show(self, ui: &mut Ui) -> CodeEditorOutput {
        let Self { editor, view, colors, id_salt, font, line_height, blink, gutter, read_only, mut styler } = self;
        let id = ui.make_persistent_id(id_salt);
        let font = font.unwrap_or_else(|| egui::TextStyle::Monospace.resolve(ui.style()));
        let (column_w, glyph_row) = ui.ctx().fonts_mut(|f| (f.glyph_width(&font, '0').max(1.0), f.row_height(&font)));
        let row_h = (font.size * line_height).round().max(glyph_row).max(1.0);
        let mac = ui.ctx().os() == egui::os::OperatingSystem::Mac;
        let now = ui.input(|i| i.time);
        editor.set_time(now);
        let version_before = editor.version();
        let selection_before = editor.selections();

        // --- input, before drawing, so this frame shows its result ---
        let focused = ui.memory(|m| m.has_focus(id));
        if focused {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(id, EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true });
            });
            let page = view.page_lines;
            handle_events(ui, editor, view, mac, read_only, page, now);
        }

        // The styler hears about edits before anything is drawn, so no frame shows colours of the text as it was.
        if let Some(styler) = styler.as_mut()
            && let Some(first_line) = editor.first_changed_line_since(view.styled_version)
        {
            styler.text_changed(first_line);
        }
        view.styled_version = editor.version();

        // --- geometry ---
        let line_count = editor.buffer().line_count();
        let gutter_w = if gutter { gutter_width(line_count, column_w) } else { 0.0 };
        update_max_columns(editor, view);
        let content_w = gutter_w + TEXT_MARGIN + (view.max_columns as f32 + 8.0) * column_w;
        let content_h = (line_count as f32 + BOTTOM_LINES) * row_h;

        let mut scroll = ScrollArea::both().id_salt(id.with("scroll")).auto_shrink([false, false]);
        if let Some(line) = view.scroll_to_line.take() {
            scroll = scroll.vertical_scroll_offset(line.min(line_count.saturating_sub(1)) as f32 * row_h);
        }
        let reveal = editor.take_reveal();
        let mut response_slot: Option<Response> = None;
        // The scroll area's edge fade would smear over the text; the host's setting comes back afterwards.
        let fade = ui.spacing().scroll.fade;
        ui.spacing_mut().scroll.fade.strength = 0.0;
        let outer = scroll.show_viewport(ui, |ui, viewport| {
            let size = vec2(content_w.max(viewport.width()), content_h.max(viewport.height()));
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let response = ui.interact(rect, id, Sense::click_and_drag());
            view.page_lines = ((viewport.height() / row_h).floor() as usize).saturating_sub(1).max(1);
            let screen_view = viewport.translate(rect.min.to_vec2());
            let painter = ui.painter_at(screen_view);
            painter.rect_filled(screen_view, 0.0, colors.background);

            let first = ((viewport.min.y / row_h).floor().max(0.0) as usize).min(line_count.saturating_sub(1));
            let last = (((viewport.max.y / row_h).ceil().max(0.0) as usize) + 1).min(line_count);
            view.frame += 1;
            // A shorter borrow of the styler for this frame (a trait object's lifetime can only be shortened at a coercion).
            let styler_ref: Option<&mut dyn LineStyler> = match styler.as_mut() {
                Some(s) => Some(&mut **s),
                None => None,
            };
            let frame = Frame {
                painter: &painter,
                font: &font,
                colors: &colors,
                column_w,
                tab_width: editor.settings().tab_width,
                scroll_x: viewport.min.x,
                version: editor.version(),
                styler: std::cell::RefCell::new(styler_ref),
            };
            let text_x = rect.min.x + gutter_w + TEXT_MARGIN;
            view.geometry = Some(Geometry { origin: rect.min, text_x, row_height: row_h, column_width: column_w });

            // Pointer.
            handle_pointer(ui, &response, editor, view, &frame, rect.min, gutter_w, row_h, screen_view, read_only);

            // Current line.
            let carets: Vec<_> = editor.carets().to_vec();
            let caret_lines: Vec<usize> = carets.iter().map(|c| editor.buffer().line_of(c.head())).collect();
            let has_focus = ui.memory(|m| m.has_focus(id));
            for line in first..last {
                let y = rect.min.y + line as f32 * row_h;
                if caret_lines.contains(&line) && carets.iter().all(|c| c.is_empty()) {
                    painter.rect_filled(Rect::from_min_max(pos2(screen_view.min.x, y), pos2(screen_view.max.x, y + row_h)), 0.0, colors.current_line);
                }
            }
            // Selections, then text.
            let selection_color = if has_focus { colors.selection } else { colors.selection_inactive };
            let text_clip = Rect::from_min_max(pos2(screen_view.min.x + gutter_w, screen_view.min.y), screen_view.max);
            let text_painter = painter.with_clip_rect(text_clip);
            let mut layouts: Vec<(usize, Arc<LineLayout>)> = Vec::with_capacity(last.saturating_sub(first));
            for line in first..last {
                let layout = layout_line(view, editor.buffer(), line, &frame);
                let y = rect.min.y + line as f32 * row_h;
                let line_start = editor.buffer().line_start(line).unwrap_or(0);
                let line_range = editor.buffer().line_range_with_break(line).unwrap_or(line_start..line_start);
                for caret in carets.iter().filter(|c| !c.is_empty()) {
                    if caret.end() <= line_range.start || caret.start() > line_range.end {
                        continue;
                    }
                    let text_end = editor.buffer().line_range(line).map_or(line_range.end, |r| r.end);
                    let from = caret.start().max(line_start) - line_start;
                    let to = caret.end().min(text_end) - line_start;
                    let x0 = text_x + layout.x_of_byte(from);
                    let mut x1 = text_x + layout.x_of_byte(to);
                    // A selection that takes in the line break shows one more column.
                    if caret.end() > text_end && caret.start() <= text_end {
                        x1 += column_w * 0.6;
                    }
                    if x1 > x0 || (caret.start() <= line_start && caret.end() > line_start && to == 0) {
                        let x1 = if x1 > x0 { x1 } else { x0 + column_w * 0.6 };
                        text_painter.rect_filled(Rect::from_min_max(pos2(x0, y), pos2(x1, y + row_h)), 0.0, selection_color);
                    }
                }
                let galley = layout.galley();
                let dy = ((row_h - layout.size().y) / 2.0).max(0.0);
                text_painter.galley(pos2(text_x + layout.x_origin(), y + dy), galley.clone(), colors.text);
                let marks = frame.styler.borrow_mut().as_mut().map(|styler| styler.marks(editor.buffer(), line)).unwrap_or_default();
                for mark in marks {
                    let x0 = text_x + layout.x_of_byte(mark.range.start);
                    // A problem at a point still shows: at least a character wide.
                    let x1 = (text_x + layout.x_of_byte(mark.range.end)).max(x0 + column_w);
                    wavy_underline(&text_painter, x0, x1, y + row_h - 2.0, mark.color);
                }
                layouts.push((line, layout));
            }
            view.cache.retain(|_, c| c.frame == view.frame);

            // Carets and the composition.
            // Solid for half a second after the last key or click, then blinking.
            let blink_on = !blink || (((now - view.last_input).max(0.0) * 2.0) as u64).is_multiple_of(2);
            let mut primary_rect: Option<Rect> = None;
            for (index, caret) in carets.iter().enumerate() {
                let line = editor.buffer().line_of(caret.head());
                let is_primary = index == carets.len().saturating_sub(1).min(editor_primary(editor, &carets));
                let Some((_, layout)) = layouts.iter().find(|(l, _)| *l == line) else { continue };
                let line_start = editor.buffer().line_start(line).unwrap_or(0);
                let x = text_x + layout.x_of_byte(caret.head().saturating_sub(line_start));
                let y = rect.min.y + line as f32 * row_h;
                let caret_rect = Rect::from_min_max(pos2(x, y + 1.0), pos2(x + 2.0, y + row_h - 1.0));
                if is_primary {
                    primary_rect = Some(caret_rect);
                }
                if has_focus && blink_on && view.preedit.is_none() {
                    text_painter.rect_filled(caret_rect, 0.0, colors.caret);
                }
            }
            if let (Some(preedit), Some(caret_rect)) = (&view.preedit, primary_rect) {
                draw_preedit(&text_painter, preedit, caret_rect, &font, &colors, row_h, column_w);
            }

            // The gutter, over the text, fixed to the left of the view.
            if gutter {
                let gutter_rect = Rect::from_min_max(screen_view.min, pos2(screen_view.min.x + gutter_w, screen_view.max.y));
                painter.rect_filled(gutter_rect, 0.0, colors.gutter_background);
                painter.line_segment(
                    [pos2(gutter_rect.max.x - 0.5, gutter_rect.min.y), pos2(gutter_rect.max.x - 0.5, gutter_rect.max.y)],
                    Stroke::new(1.0, colors.gutter_border),
                );
                for line in first..last {
                    let y = rect.min.y + line as f32 * row_h;
                    let color = if caret_lines.contains(&line) { colors.gutter_text_current } else { colors.gutter_text };
                    painter.text(pos2(gutter_rect.max.x - GUTTER_PADDING, y + row_h / 2.0), Align2::RIGHT_CENTER, line + 1, font.clone(), color);
                }
            }

            // Scroll the caret into view when a command asked for it, and tell an input method where to put its window.
            if let Some(caret_rect) = primary_rect {
                if reveal {
                    let margin = vec2(column_w * 3.0 + gutter_w, row_h);
                    ui.scroll_to_rect(Rect::from_min_max(caret_rect.min - margin, caret_rect.max + margin), None);
                }
                if has_focus {
                    ui.output_mut(|o| {
                        o.ime = Some(egui::output::IMEOutput {
                            purpose: egui::IMEPurpose::Normal,
                            rect: screen_view,
                            cursor_rect: caret_rect,
                            should_interrupt_composition: false,
                        });
                    });
                }
            } else if reveal {
                // The caret's line is not laid out (it is off screen): scroll to its line, then it will be drawn and revealed.
                let line = editor.buffer().line_of(editor.primary().head());
                let y = rect.min.y + line as f32 * row_h;
                ui.scroll_to_rect(Rect::from_min_max(pos2(rect.min.x, y), pos2(rect.min.x + 1.0, y + row_h)), None);
            }
            if has_focus && blink {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            }
            response_slot = Some(response);
        });
        let _ = outer;
        ui.spacing_mut().scroll.fade = fade;
        let response = response_slot.unwrap_or_else(|| ui.interact(ui.min_rect(), id, Sense::hover()));
        response.widget_info(|| WidgetInfo::labeled(WidgetType::TextEdit, true, "XML editor"));
        CodeEditorOutput { response, changed: editor.version() != version_before, selection_changed: editor.selections() != selection_before }
    }
}

/// Index of the primary caret among `carets` (the editor keeps the carets sorted and the primary is one of them).
fn editor_primary(editor: &Editor, carets: &[xmlcraft_editor::Caret]) -> usize {
    let primary = editor.primary();
    carets.iter().position(|c| c.sel == primary.sel).unwrap_or(carets.len().saturating_sub(1))
}

fn gutter_width(line_count: usize, column_w: f32) -> f32 {
    let digits = line_count.max(1).to_string().len().max(3);
    digits as f32 * column_w + GUTTER_PADDING * 2.0
}

/// Make sure the horizontal scroll range covers the widest line. The first time (and after a reset) every line is looked at; after
/// that only the lines in the editor's carets and the version are cheap to follow, so the range only grows.
fn update_max_columns(editor: &Editor, view: &mut ViewState) {
    let buffer = editor.buffer();
    if view.max_columns_known_for.is_none() && buffer.len() <= 32 * 1024 * 1024 {
        let mut widest = 0;
        for line in 0..buffer.line_count() {
            if let Some(range) = buffer.line_range(line) {
                widest = widest.max(range.len());
            }
        }
        view.max_columns = view.max_columns.max(widest);
    }
    if view.max_columns_known_for != Some(editor.version()) {
        // Lines the carets are on may have grown.
        for caret in editor.carets() {
            if let Some(range) = buffer.line_range(buffer.line_of(caret.head())) {
                view.max_columns = view.max_columns.max(range.len());
            }
        }
        view.max_columns_known_for = Some(editor.version());
    }
}

/// Everything the layout and hit-testing of lines need.
struct Frame<'a> {
    painter: &'a egui::Painter,
    font: &'a FontId,
    colors: &'a EditorColors,
    column_w: f32,
    tab_width: usize,
    scroll_x: f32,
    version: u64,
    styler: std::cell::RefCell<Option<&'a mut dyn LineStyler>>,
}

/// The characters of a long line that are laid out: a window around what is scrolled into view.
fn window_for(chars: usize, frame: &Frame<'_>) -> Range<usize> {
    if chars <= MAX_WINDOW {
        return 0..chars;
    }
    let first_visible = (frame.scroll_x / frame.column_w).floor().max(0.0) as usize;
    let start = first_visible.saturating_sub(MAX_WINDOW / 4).min(chars.saturating_sub(MAX_WINDOW));
    // Quantise the start so that scrolling a little does not lay the line out again every frame.
    let start = start / 256 * 256;
    start..(start + MAX_WINDOW).min(chars)
}

fn layout_line(view: &mut ViewState, buffer: &Buffer, line: usize, frame: &Frame<'_>) -> Arc<LineLayout> {
    let key = CacheKey {
        version: frame.version,
        highlight: view.highlight_version,
        font_size: frame.font.size.to_bits(),
        tab_width: frame.tab_width,
        color: frame.colors.text,
    };
    if let Some(cached) = view.cache.get_mut(&line)
        && cached.key == key
        && (cached.chars <= MAX_WINDOW || window_for(cached.chars, frame).start == cached.layout.first_char())
    {
        cached.frame = view.frame;
        return cached.layout.clone();
    }
    let text = buffer.line_text(line).unwrap_or_default();
    let chars = text.chars().count();
    let window = window_for(chars, frame);
    let spans: Vec<StyleSpan> = frame.styler.borrow_mut().as_mut().map(|styler| styler.style_line(buffer, line, &text)).unwrap_or_default();
    let params = LayoutParams { font: frame.font, color: frame.colors.text, tab_width: frame.tab_width, column_width: frame.column_w, spans: &spans };
    let layout = Arc::new(LineLayout::new(frame.painter, &text, window, &params));
    view.cache.insert(line, Cached { key, layout: layout.clone(), chars, frame: view.frame });
    layout
}

/// The buffer offset under a point in the screen, snapped to a character boundary.
fn offset_at(view: &mut ViewState, editor: &Editor, frame: &Frame<'_>, origin: Pos2, gutter_w: f32, row_h: f32, pos: Pos2) -> usize {
    let buffer = editor.buffer();
    let line = (((pos.y - origin.y) / row_h).floor().max(0.0) as usize).min(buffer.line_count().saturating_sub(1));
    let layout = layout_line(view, buffer, line, frame);
    let x = pos.x - origin.x - gutter_w - TEXT_MARGIN;
    let line_start = buffer.line_start(line).unwrap_or(0);
    let line_end = buffer.line_range(line).map_or(line_start, |r| r.end);
    let offset = (line_start + layout.byte_at_x(x)).min(line_end);
    // Inside a cluster (a letter and its accent): the cluster's start.
    snap_to_cluster(buffer, offset).max(line_start)
}

#[allow(clippy::too_many_arguments)]
fn handle_pointer(
    ui: &mut Ui,
    response: &Response,
    editor: &mut Editor,
    view: &mut ViewState,
    frame: &Frame<'_>,
    origin: Pos2,
    gutter_w: f32,
    row_h: f32,
    view_rect: Rect,
    read_only: bool,
) {
    let _ = read_only;
    let (pressed, modifiers, pos) = ui.input(|i| (i.pointer.primary_pressed(), i.modifiers, i.pointer.interact_pos()));
    let in_gutter = pos.is_some_and(|p| p.x < view_rect.min.x + gutter_w);
    if response.hovered() && !in_gutter {
        ui.output_mut(|o| o.cursor_icon = CursorIcon::Text);
    }
    let Some(pos) = pos else { return };
    let now = ui.input(|i| i.time);
    if pressed && response.hovered() {
        response.request_focus();
        view.last_input = now;
        editor.break_undo_group();
        let offset = offset_at(view, editor, frame, origin, gutter_w, row_h, pos);
        if in_gutter {
            editor.select_line_at(offset, modifiers.command);
        } else if modifiers.shift {
            editor.extend_selection_to(offset);
        } else {
            editor.place_caret(offset, modifiers.command);
        }
    } else if response.dragged() {
        view.last_input = now;
        let offset = offset_at(view, editor, frame, origin, gutter_w, row_h, pos);
        editor.extend_selection_to(offset);
        if !view_rect.contains(pos) {
            // Dragging past the edge scrolls.
            ui.scroll_to_rect(Rect::from_center_size(pos, vec2(1.0, 1.0)), None);
        }
    } else if response.double_clicked() && !in_gutter {
        let offset = offset_at(view, editor, frame, origin, gutter_w, row_h, pos);
        editor.select_word_at(offset, modifiers.command);
    } else if response.triple_clicked() && !in_gutter {
        let offset = offset_at(view, editor, frame, origin, gutter_w, row_h, pos);
        editor.select_line_at(offset, modifiers.command);
    }
}

fn handle_events(ui: &mut Ui, editor: &mut Editor, view: &mut ViewState, mac: bool, read_only: bool, page: usize, now: f64) {
    let events = ui.input(|i| i.events.clone());
    for event in events {
        if matches!(event, Event::Text(_) | Event::Paste(_) | Event::Cut | Event::Ime(_) | Event::Key { pressed: true, .. }) {
            log::debug!("editor input: {event:?}");
        }
        match event {
            Event::Text(text) if !read_only => {
                let text: String = text.chars().filter(|c| !c.is_control()).collect();
                if !text.is_empty() && view.preedit.is_none() {
                    editor.type_text(&text);
                    view.last_input = now;
                }
            }
            Event::Paste(text) if !read_only => {
                editor.paste(&text);
                view.last_input = now;
            }
            Event::Copy => {
                let text = editor.copy_text();
                ui.ctx().copy_text(text);
            }
            Event::Cut => {
                if read_only {
                    let text = editor.copy_text();
                    ui.ctx().copy_text(text);
                } else {
                    let text = editor.cut();
                    ui.ctx().copy_text(text);
                    view.last_input = now;
                }
            }
            Event::Key { key, pressed: true, modifiers, .. } => {
                if view.preedit.is_some() && matches!(key, Key::Enter | Key::Tab | Key::Backspace | Key::Delete) {
                    continue; // the input method owns these while composing
                }
                if let Some(command) = command_for(key, modifiers, mac, page) {
                    run_command(editor, command, read_only, modifiers);
                    view.last_input = now;
                }
            }
            Event::Ime(ime) if !read_only => match ime {
                ImeEvent::Preedit { text, active_range_chars } => {
                    view.preedit = (!text.is_empty()).then_some(Preedit { text, active: active_range_chars });
                }
                ImeEvent::Commit(text) => {
                    view.preedit = None;
                    let text: String = text.chars().filter(|c| !c.is_control()).collect();
                    if !text.is_empty() {
                        editor.type_text(&text);
                        view.last_input = now;
                    }
                }
                ImeEvent::DeleteSurrounding { before_chars, after_chars } => {
                    editor.delete_surrounding(before_chars, after_chars);
                }
                #[allow(deprecated)]
                ImeEvent::Enabled | ImeEvent::Disabled => {}
            },
            _ => {}
        }
    }
}

fn run_command(editor: &mut Editor, command: Command, read_only: bool, _modifiers: Modifiers) {
    match command {
        Command::Move(motion, extend) => editor.move_carets(motion, extend),
        Command::SelectAll => editor.select_all(),
        Command::KeepPrimaryCaret => editor.keep_primary_only(),
        _ if read_only => {}
        Command::Backspace => drop(editor.backspace()),
        Command::Delete => drop(editor.delete_forward()),
        Command::DeleteWordBackward => drop(editor.delete_word_backward()),
        Command::DeleteWordForward => drop(editor.delete_word_forward()),
        Command::DeleteToLineStart => drop(editor.delete_to_line_start()),
        Command::DeleteToLineEnd => drop(editor.delete_to_line_end()),
        Command::Newline => drop(editor.insert_newline()),
        Command::Indent => drop(editor.indent()),
        Command::Outdent => drop(editor.outdent()),
        Command::Undo => drop(editor.undo()),
        Command::Redo => drop(editor.redo()),
    }
}

fn draw_preedit(painter: &egui::Painter, preedit: &Preedit, caret: Rect, font: &FontId, colors: &EditorColors, row_h: f32, column_w: f32) {
    let galley = painter.layout_no_wrap(preedit.text.clone(), font.clone(), colors.text);
    let rect = Rect::from_min_size(pos2(caret.min.x, caret.min.y - 1.0), vec2(galley.size().x.max(column_w), row_h));
    painter.rect_filled(rect, 2.0, colors.preedit_background);
    painter.galley(pos2(rect.min.x, rect.min.y + ((row_h - galley.size().y) / 2.0).max(0.0)), galley.clone(), colors.text);
    // The part the input method is converting is underlined thickly, the rest thinly.
    let underline = |from: usize, to: usize, width: f32| {
        let x0 = rect.min.x + galley.pos_from_cursor(egui::text::CCursor::new(from)).min.x;
        let x1 = rect.min.x + galley.pos_from_cursor(egui::text::CCursor::new(to)).min.x;
        painter.line_segment([pos2(x0, rect.max.y - 2.0), pos2(x1.max(x0 + 1.0), rect.max.y - 2.0)], Stroke::new(width, colors.caret));
    };
    let total = preedit.text.chars().count();
    underline(0, total, 1.0);
    if let Some(active) = &preedit.active {
        underline(active.start.min(total), active.end.min(total), 2.0);
    }
}

/// A zigzag line from `x0` to `x1` at height `y`, only as far as the painter's clip rectangle shows.
fn wavy_underline(painter: &egui::Painter, x0: f32, x1: f32, y: f32, color: Color32) {
    let clip = painter.clip_rect();
    let (x0, x1) = (x0.max(clip.min.x), x1.min(clip.max.x));
    if x1 <= x0 || !x0.is_finite() || !x1.is_finite() {
        return;
    }
    const STEP: f32 = 2.5;
    const AMPLITUDE: f32 = 1.4;
    let mut points = Vec::new();
    let (mut x, mut up) = (x0, false);
    while x < x1 {
        points.push(pos2(x, if up { y - AMPLITUDE } else { y + AMPLITUDE }));
        x += STEP;
        up = !up;
    }
    points.push(pos2(x1, if up { y - AMPLITUDE } else { y + AMPLITUDE }));
    painter.add(egui::Shape::line(points, Stroke::new(1.0, color)));
}
