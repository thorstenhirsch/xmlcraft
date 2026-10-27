//! The editor widget under egui_kittest: typing, keys, the mouse, the clipboard, input methods, and that only what is on screen is
//! laid out.

use egui::{Event, Id, ImeEvent, Key, Modifiers, PointerButton};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_code_editor::{CodeEditor, EditorColors, FoldRegion, LineStyler, MAX_WINDOW, StyleSpan, ViewState};
use xmlcraft_editor::Editor;

/// Records what the widget asks of a styler, and colours the first byte of every line.
#[derive(Default)]
struct Recorder {
    changed: Vec<usize>,
    styled: std::collections::BTreeSet<usize>,
    /// The lines that can be folded: (the line, the last line it would hide).
    foldable: Vec<(usize, usize)>,
}

impl LineStyler for Recorder {
    fn text_changed(&mut self, first_line: usize) {
        self.changed.push(first_line);
        self.styled.clear();
    }

    fn style_line(&mut self, _buffer: &xmlcraft_text::Buffer, line: usize, text: &str) -> Vec<StyleSpan> {
        self.styled.insert(line);
        if text.is_empty() { Vec::new() } else { vec![StyleSpan { range: 0..1, color: egui::Color32::RED }] }
    }

    fn fold_at(&mut self, _buffer: &xmlcraft_text::Buffer, line: usize) -> Option<FoldRegion> {
        self.foldable.iter().find(|(first, _)| *first == line).map(|(_, last)| FoldRegion { last_line: *last })
    }
}

struct App {
    editor: Editor,
    view: ViewState,
    read_only: bool,
    recorder: Recorder,
    /// Whether the recorder is attached as the styler this frame.
    styled: bool,
    /// The lines whose fold marks were clicked, in order.
    fold_clicks: Vec<usize>,
}

fn harness(text: &str) -> Harness<'static, App> {
    harness_with_step(text, 0.25)
}

/// A harness whose frames are `step_dt` seconds apart (two clicks must come within 0.3 s to be a double click).
fn harness_with_step(text: &str, step_dt: f32) -> Harness<'static, App> {
    let app = App {
        editor: Editor::new(text),
        view: ViewState::new(),
        read_only: false,
        recorder: Recorder::default(),
        styled: true,
        fold_clicks: Vec::new(),
    };
    let mut h = Harness::builder().with_size(egui::vec2(640.0, 360.0)).with_step_dt(step_dt).build_ui_state(
        |ui, app: &mut App| {
            let editor =
                CodeEditor::new(Id::new("editor"), &mut app.editor, &mut app.view, EditorColors::light()).blink(false).read_only(app.read_only);
            let output = if app.styled { editor.styler(&mut app.recorder).show(ui) } else { editor.show(ui) };
            app.fold_clicks.extend(output.fold_clicked);
        },
        app,
    );
    h.run_steps(3);
    h.get_by_label("XML editor").focus();
    h.run_steps(3);
    h
}

fn text(h: &Harness<'_, App>) -> String {
    h.state().editor.text()
}

/// The text the last frame asked egui to put on the clipboard.
fn copied(h: &Harness<'_, App>) -> Option<String> {
    h.output().platform_output.commands.iter().find_map(|command| match command {
        egui::OutputCommand::CopyText(text) => Some(text.clone()),
        _ => None,
    })
}

fn carets(h: &Harness<'_, App>) -> Vec<(usize, usize)> {
    h.state().editor.carets().iter().map(|c| (c.sel.anchor, c.sel.head)).collect()
}

#[test]
fn typing_inserts_text_at_the_caret() {
    let mut h = harness("");
    h.get_by_label("XML editor").type_text("abc");
    h.run_steps(2);
    assert_eq!(text(&h), "abc");
    assert_eq!(carets(&h), vec![(3, 3)]);
    h.key_press(Key::Enter);
    h.run_steps(2);
    h.get_by_label("XML editor").type_text("x");
    h.run_steps(2);
    assert_eq!(text(&h), "abc\nx");
    h.key_press(Key::Backspace);
    h.key_press(Key::Backspace);
    h.run_steps(2);
    assert_eq!(text(&h), "abc");
}

#[test]
fn a_quote_typed_in_a_tag_is_paired_but_what_an_input_method_commits_is_not() {
    let mut h = harness("");
    h.get_by_label("XML editor").type_text("<a b=");
    h.get_by_label("XML editor").type_text("\"");
    h.run_steps(2);
    assert_eq!((text(&h).as_str(), carets(&h)), ("<a b=\"\"", vec![(6, 6)]), "the pair, the caret between");
    h.get_by_label("XML editor").type_text("x");
    h.get_by_label("XML editor").type_text("\"");
    h.run_steps(2);
    assert_eq!((text(&h).as_str(), carets(&h)), ("<a b=\"x\"", vec![(8, 8)]), "the closing quote is stepped over");
    // Backspace between the two quotes of an empty value takes both.
    let mut h = harness("");
    h.get_by_label("XML editor").type_text("<a b=");
    h.get_by_label("XML editor").type_text("\"");
    h.run_steps(2);
    assert_eq!(text(&h), "<a b=\"\"", "the pair is there");
    h.key_press(Key::Backspace);
    h.run_steps(2);
    assert_eq!(text(&h), "<a b=", "and Backspace takes both");
    // The same text arriving in one event is text, not typing: no pair.
    let mut literal = harness("");
    literal.get_by_label("XML editor").type_text("<a b=\"");
    literal.run_steps(2);
    assert_eq!(text(&literal), "<a b=\"");
    // A quote committed by an input method is text, and so is a paste.
    h.event(Event::Ime(ImeEvent::Commit("\"".to_owned())));
    h.run_steps(2);
    assert_eq!(text(&h), "<a b=\"");
    h.event(Event::Paste("'".to_owned()));
    h.run_steps(2);
    assert_eq!(text(&h), "<a b=\"'");
}

#[test]
fn typing_a_start_tag_writes_its_end_tag_and_enter_splits_the_pair() {
    let mut h = harness("");
    for key in ["<", "a", ">"] {
        h.get_by_label("XML editor").type_text(key);
    }
    h.run_steps(2);
    assert_eq!((text(&h).as_str(), carets(&h)), ("<a></a>", vec![(3, 3)]), "the end tag follows, the caret between");
    h.key_press(Key::Enter);
    h.run_steps(2);
    assert_eq!((text(&h).as_str(), carets(&h)), ("<a>\n  \n</a>", vec![(6, 6)]), "a line for the content, the end tag on its own");
    // `</` completes the end tag of the element that is open.
    let mut h = harness("<a><b>x");
    h.state_mut().editor.set_selection(7, 7);
    h.run_steps(2);
    for key in ["<", "/"] {
        h.get_by_label("XML editor").type_text(key);
    }
    h.run_steps(2);
    assert_eq!(text(&h), "<a><b>x</b>");
    // The text of an input method is text: no end tag.
    let mut h = harness("");
    h.get_by_label("XML editor").type_text("<a");
    h.event(Event::Ime(ImeEvent::Commit(">".to_owned())));
    h.run_steps(2);
    assert_eq!(text(&h), "<a>");
}

#[test]
fn keys_move_select_and_delete() {
    let mut h = harness("one two\nthree");
    h.key_press(Key::ArrowRight);
    h.key_press(Key::ArrowRight);
    h.run_steps(2);
    assert_eq!(carets(&h), vec![(2, 2)]);
    h.key_press_modifiers(Modifiers::SHIFT, Key::End);
    h.run_steps(2);
    assert_eq!(carets(&h), vec![(2, 7)]);
    h.key_press(Key::Delete);
    h.run_steps(2);
    assert_eq!(text(&h), "on\nthree");
    h.key_press(Key::ArrowDown);
    h.run_steps(2);
    assert_eq!(carets(&h), vec![(5, 5)], "Down keeps the column");
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(2);
    assert_eq!(carets(&h), vec![(0, 8)]);
}

#[test]
fn undo_and_redo_through_the_keyboard() {
    let mut h = harness("");
    h.get_by_label("XML editor").type_text("abc");
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(text(&h), "");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run_steps(2);
    assert_eq!(text(&h), "abc");
}

#[test]
fn clicking_places_the_caret() {
    let mut h = harness("alpha beta\ngamma");
    let g = h.state().view.geometry().expect("drawn");
    // Between `be` and `ta` on the first line: column 8.
    let at = g.point(0, 8) + egui::vec2(0.0, g.row_height / 2.0);
    h.hover_at(at);
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    assert_eq!(carets(&h), vec![(8, 8)]);
    // A click on the second line, far to the right of its text: the end of the line.
    let at = g.point(1, 40) + egui::vec2(0.0, g.row_height / 2.0);
    h.hover_at(at);
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    assert_eq!(carets(&h), vec![(16, 16)]);
}

fn click(h: &mut Harness<'_, App>, at: egui::Pos2) {
    h.hover_at(at);
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// Twelve lines `a0` … `a11`.
fn twelve() -> String {
    (0..12).map(|i| format!("a{i}")).collect::<Vec<_>>().join("\n")
}

#[test]
fn hidden_lines_have_no_row_and_a_click_goes_to_the_line_on_the_row() {
    let mut h = harness(&twelve());
    h.state_mut().editor.fold_lines(2, 6); // lines 3 to 6 are hidden
    h.run_steps(3);
    assert_eq!(h.state().view.laid_out_lines(), 8, "twelve lines, four of them hidden");
    // Laid out again from scratch, the lines that are drawn are the ones that are not hidden.
    h.state_mut().view.reset();
    h.state_mut().recorder.styled.clear();
    h.run_steps(3);
    assert_eq!(h.state().recorder.styled.iter().copied().collect::<Vec<_>>(), [0, 1, 2, 7, 8, 9, 10, 11]);
    let g = h.state().view.geometry().expect("drawn");
    // Rows 0, 1, 2 are lines 0, 1, 2 and row 3 is line 7 (bytes 21 and on): a click on the second letter of row 3.
    click(&mut h, g.point(3, 1) + egui::vec2(0.0, g.row_height / 2.0));
    assert_eq!(carets(&h), vec![(22, 22)]);
    // Typing goes where the caret is, below the fold.
    h.get_by_label("XML editor").type_text("X");
    h.run_steps(2);
    assert_eq!(h.state().editor.text().lines().nth(7), Some("aX7"));
    assert_eq!(h.state().editor.folds().len(), 1, "the fold is still there");
    // Keys move over it: Up from line 7 goes to the line that stands for the fold, not into it.
    h.key_press(Key::ArrowUp);
    h.run_steps(2);
    assert_eq!(h.state().editor.buffer().line_of(carets(&h)[0].1), 2);
}

#[test]
fn the_mark_of_a_fold_opens_it_when_clicked() {
    let mut h = harness(&twelve());
    h.state_mut().editor.fold_lines(2, 6);
    h.run_steps(3);
    let g = h.state().view.geometry().expect("drawn");
    // Line 2 is `a2`: its text ends at column 2 and the mark follows a little later.
    click(&mut h, g.point(2, 4) + egui::vec2(0.0, g.row_height / 2.0));
    assert!(h.state().editor.folds().is_empty(), "the fold is open");
    assert_eq!(h.state().view.laid_out_lines(), 12);
}

#[test]
fn the_gutter_has_a_mark_where_something_can_be_folded_and_a_click_on_it_is_reported_or_opens() {
    let mut h = harness(&twelve());
    h.state_mut().recorder.foldable = vec![(2, 6)];
    h.run_steps(3);
    let g = h.state().view.geometry().expect("drawn");
    // The strip at the right of the gutter (16 points wide) is left of the text margin (8 points).
    let strip = |row: usize| g.point(row, 0) + egui::vec2(-8.0 - 8.0, g.row_height / 2.0);
    click(&mut h, strip(2));
    assert_eq!(h.state().fold_clicks, [2], "the host is asked to fold it");
    assert!(h.state().editor.folds().is_empty(), "the widget does not know how far a fold goes");
    // A line without a mark: a click there does nothing (it does not select the line, as a click on the numbers does).
    click(&mut h, strip(4));
    assert_eq!(h.state().fold_clicks, [2]);
    assert_eq!(carets(&h), vec![(0, 0)]);
    // The host folds it; the mark now says "folded", and a click on it opens the fold itself.
    h.state_mut().editor.fold_lines(2, 6);
    h.run_steps(3);
    click(&mut h, strip(2));
    assert!(h.state().editor.folds().is_empty());
    assert_eq!(h.state().fold_clicks, [2], "not reported: it was an opening");
    // A click on the numbers part of the gutter still selects the line.
    let numbers = g.point(1, 0) + egui::vec2(-8.0 - 16.0 - 12.0, g.row_height / 2.0);
    click(&mut h, numbers);
    assert_eq!(carets(&h), vec![(3, 6)], "line 1 with its break");
}

#[test]
fn double_clicking_selects_the_word_under_the_pointer() {
    let mut h = harness_with_step("alpha beta\ngamma", 0.05);
    let g = h.state().view.geometry().expect("drawn");
    // On `beta` (columns 6 to 10): two clicks, each in a frame of its own, as a pointer delivers them.
    let at = g.point(0, 8) + egui::vec2(0.0, g.row_height / 2.0);
    h.hover_at(at);
    for _ in 0..2 {
        h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.step();
        h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
    assert_eq!(carets(&h), vec![(6, 10)], "the word, not just a caret");
}

#[test]
fn dragging_selects() {
    let mut h = harness("alpha beta");
    let g = h.state().view.geometry().expect("drawn");
    let y = g.row_height / 2.0;
    let from = g.point(0, 2) + egui::vec2(0.0, y);
    let to = g.point(0, 7) + egui::vec2(0.0, y);
    h.hover_at(from);
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(2);
    h.hover_at(to);
    h.run_steps(2);
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert_eq!(carets(&h), vec![(2, 7)]);
}

#[test]
fn copy_cut_and_paste_events() {
    let mut h = harness("hello world");
    h.state_mut().editor.set_selection(0, 5);
    h.run_steps(2);
    h.event(Event::Copy);
    h.step();
    assert!(copied(&h).as_deref() == Some("hello"), "copy hands egui the selection: {:?}", copied(&h));
    h.run_steps(2);
    assert_eq!(text(&h), "hello world");
    h.event(Event::Cut);
    h.step();
    assert!(copied(&h).as_deref() == Some("hello"), "cut hands egui the selection too: {:?}", copied(&h));
    h.run_steps(2);
    assert_eq!(text(&h), " world");
    h.event(Event::Paste("Goodbye".to_owned()));
    h.run_steps(2);
    assert_eq!(text(&h), "Goodbye world");
    assert_eq!(carets(&h), vec![(7, 7)]);
}

#[test]
fn an_input_method_composes_without_touching_the_document_until_it_commits() {
    let mut h = harness("ab");
    h.state_mut().editor.set_selection(1, 1);
    h.run_steps(2);
    h.event(Event::Ime(ImeEvent::Preedit { text: "にほ".to_owned(), active_range_chars: Some(0..1) }));
    h.run_steps(2);
    assert!(h.state().view.is_composing());
    assert_eq!(text(&h), "ab", "the composition is not in the text yet");
    h.event(Event::Ime(ImeEvent::Commit("日本".to_owned())));
    h.run_steps(2);
    assert!(!h.state().view.is_composing());
    assert_eq!(text(&h), "a日本b");
    assert_eq!(carets(&h), vec![(7, 7)]);
    // Ending a composition with nothing clears it.
    h.event(Event::Ime(ImeEvent::Preedit { text: "x".to_owned(), active_range_chars: None }));
    h.run_steps(2);
    assert!(h.state().view.is_composing());
    h.event(Event::Ime(ImeEvent::Preedit { text: String::new(), active_range_chars: None }));
    h.run_steps(2);
    assert!(!h.state().view.is_composing());
}

#[test]
fn a_read_only_editor_moves_and_selects_but_does_not_change() {
    let mut h = harness("abc");
    h.state_mut().read_only = true;
    h.run_steps(2);
    h.get_by_label("XML editor").type_text("x");
    h.key_press(Key::Backspace);
    h.event(Event::Paste("y".to_owned()));
    h.key_press(Key::ArrowRight);
    h.run_steps(2);
    assert_eq!(text(&h), "abc");
    assert_eq!(carets(&h), vec![(1, 1)]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(2);
    assert_eq!(carets(&h), vec![(0, 3)]);
}

#[test]
fn only_the_lines_on_screen_are_laid_out() {
    let text = (0..200_000).map(|i| format!("<line n=\"{i}\"/>")).collect::<Vec<_>>().join("\n");
    let mut h = harness(&text);
    h.run_steps(3);
    let laid_out = h.state().view.laid_out_lines();
    assert!(laid_out > 5 && laid_out < 60, "{laid_out} lines laid out for a 640x360 view");
    // Jump to the end: the caret is scrolled into view and the view lays out lines near the end.
    h.key_press_modifiers(Modifiers::COMMAND, Key::End);
    h.run_steps(8);
    assert!(h.state().view.laid_out_lines() < 60);
    let g = h.state().view.geometry().expect("drawn");
    assert!(g.origin.y < -1_000_000.0, "scrolled far down: {:?}", g.origin);
}

#[test]
fn a_very_long_line_is_laid_out_in_a_window() {
    let line = "<x a=\"1\"/>".repeat(100_000); // a megabyte on one line
    // Timed from the first frame, which is where laying out the whole line would show.
    let started = std::time::Instant::now();
    let mut h = harness(&line);
    h.run_steps(5);
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "took {:?}", started.elapsed());
    assert_eq!(h.state().view.laid_out_lines(), 1);
    let laid_out = h.state().view.laid_out_characters();
    assert!(laid_out > 0 && laid_out <= MAX_WINDOW, "{laid_out} of {} characters laid out", line.chars().count());
    // The caret can still be moved and the text edited at both ends.
    h.key_press_modifiers(Modifiers::COMMAND, Key::End);
    h.run_steps(3);
    h.get_by_label("XML editor").type_text("!");
    h.run_steps(3);
    assert!(text(&h).ends_with("/>!"));
}

#[test]
fn the_styler_hears_about_edits_and_styles_only_what_is_on_screen() {
    let text = (0..100_000).map(|i| format!("<l n=\"{i}\"/>")).collect::<Vec<_>>().join("\n");
    let mut h = harness(&text);
    h.run_steps(3);
    assert!(h.state().recorder.changed.is_empty(), "nothing has changed yet: {:?}", h.state().recorder.changed);
    let styled = h.state().recorder.styled.clone();
    assert!(styled.len() > 5 && styled.len() < 60, "{} lines styled", styled.len());
    assert_eq!(styled.iter().next().copied(), Some(0));
    // An edit on the third line: the styler is told, once, and the lines are styled again.
    let third = text.lines().take(2).map(|l| l.len() + 1).sum::<usize>();
    h.state_mut().editor.set_selection(third, third);
    h.run_steps(2);
    h.get_by_label("XML editor").type_text("x");
    h.run_steps(3);
    assert_eq!(h.state().recorder.changed, vec![2]);
    assert!(h.state().recorder.styled.contains(&2), "the edited line is styled again");
}

#[test]
fn a_reset_view_tells_the_styler_the_whole_text_changed() {
    let mut h = harness("<a>\n<b/>\n</a>");
    assert!(h.state().recorder.changed.is_empty());
    // Another document in the same view, with an editor whose version counts from zero again: nothing in the edit history says the text
    // changed, so only the reset can tell the styler its state from the old document is stale from the first line.
    h.state_mut().editor = Editor::new("<x>\n<y/>\n</x>");
    h.state_mut().view.reset();
    h.run_steps(2);
    assert_eq!(h.state().recorder.changed.first(), Some(&0), "{:?}", h.state().recorder.changed);
    // Told once: the next frames have nothing to report.
    let told = h.state().recorder.changed.len();
    h.run_steps(3);
    assert_eq!(h.state().recorder.changed.len(), told);
}

#[test]
fn a_styler_attached_again_hears_what_changed_while_it_was_away() {
    let mut h = harness("<a>\n<b/>\n</a>");
    h.state_mut().styled = false;
    h.run_steps(2);
    // An edit on the second line while no styler is attached.
    let second = "<a>\n".len();
    h.state_mut().editor.set_selection(second, second);
    h.run_steps(2);
    h.get_by_label("XML editor").type_text("x");
    h.run_steps(3);
    assert!(h.state().recorder.changed.is_empty(), "no styler, nobody to tell");
    h.state_mut().styled = true;
    h.run_steps(3);
    assert_eq!(h.state().recorder.changed, vec![1], "the edit made while it was detached");
}
