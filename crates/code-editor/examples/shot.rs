//! Headless screenshot of the editor widget with synthetic text (egui_kittest + wgpu, no window needed).
//!
//! ```text
//! cargo run -p xmlcraft-code-editor --example shot -- out.png [--theme light|dark] [--size 900x420] [--scale 2]
//! ```

use egui::{Color32, Id};
use egui_kittest::Harness;
use xmlcraft_code_editor::{CodeEditor, EditorColors, StyleSpan, ViewState};
use xmlcraft_editor::Editor;
use xmlcraft_text::Selection;

const SAMPLE: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!-- A synthetic document: tabs, wide characters, a long attribute list -->\n\
<library xmlns=\"urn:example:library\">\n\
\t<book id=\"b1\" lang=\"de\">\n\
\t\t<title>Die Verwandlung</title>\n\
\t\t<author>Franz Kafka</author>\n\
\t\t<note>日本語のテキスト and e\u{301} with a combining accent</note>\n\
\t</book>\n\
\t<book id=\"b2\" lang=\"en\" year=\"1851\" pages=\"635\" isbn=\"000-0-00-000000-0\" publisher=\"Synthetic House\">\n\
\t\t<title>Moby-Dick</title>\n\
\t</book>\n\
</library>\n";

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let out = args.next().ok_or("usage: shot <out.png> [--theme light|dark] [--size WxH] [--scale N]")?;
    let (mut size, mut scale, mut dark) = (egui::vec2(900.0, 420.0), 2.0f32, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--theme" => dark = args.next().as_deref() == Some("dark"),
            "--scale" => scale = args.next().and_then(|v| v.parse().ok()).ok_or("bad --scale")?,
            "--size" => {
                let v = args.next().unwrap_or_default();
                let (w, h) = v.split_once('x').ok_or("--size WxH")?;
                size = egui::vec2(w.parse().map_err(|_| "bad width")?, h.parse().map_err(|_| "bad height")?);
            }
            _ => return Err(format!("unexpected argument {a}")),
        }
    }
    let mut editor = Editor::new(SAMPLE);
    // A selection across two lines, and a second caret.
    let title = SAMPLE.find("Die Verwandlung").unwrap_or(0);
    let author = SAMPLE.find("Kafka").unwrap_or(0) + 5;
    editor.set_selections(&[Selection { anchor: title, head: author }, Selection::caret(SAMPLE.find("Moby").unwrap_or(0))]);
    let colors = if dark { EditorColors::dark() } else { EditorColors::light() };
    // A stand-in for the real colouring (M1.7): tags and attribute values, by simple rules.
    let tag = if dark { Color32::from_rgb(0x7A, 0xB0, 0xFF) } else { Color32::from_rgb(0x1B, 0x63, 0xE0) };
    let value = if dark { Color32::from_rgb(0xE0, 0xA6, 0x6B) } else { Color32::from_rgb(0xB0, 0x5A, 0x12) };
    let comment = if dark { Color32::from_rgb(0x7F, 0x8C, 0x98) } else { Color32::from_rgb(0x6A, 0x73, 0x7D) };
    let highlighter = move |_line: usize, text: &str| -> Vec<StyleSpan> {
        let mut spans = Vec::new();
        if text.trim_start().starts_with("<!--") {
            spans.push(StyleSpan { range: 0..text.len(), color: comment });
            return spans;
        }
        let mut in_tag = false;
        let mut quote_start = None;
        for (i, c) in text.char_indices() {
            match c {
                '<' => {
                    in_tag = true;
                    spans.push(StyleSpan { range: i..i + 1, color: tag });
                }
                '>' => {
                    in_tag = false;
                    spans.push(StyleSpan { range: i..i + 1, color: tag });
                }
                '"' if in_tag => match quote_start.take() {
                    Some(start) => spans.push(StyleSpan { range: start..i + 1, color: value }),
                    None => quote_start = Some(i),
                },
                _ if in_tag && quote_start.is_none() => spans.push(StyleSpan { range: i..i + c.len_utf8(), color: tag }),
                _ => {}
            }
        }
        spans
    };
    struct State {
        editor: Editor,
        view: ViewState,
    }
    let mut harness = Harness::builder().with_size(size).with_pixels_per_point(scale).build_ui_state(
        move |ui, state: &mut State| {
            ui.visuals_mut().override_text_color = None;
            CodeEditor::new(Id::new("shot"), &mut state.editor, &mut state.view, colors).blink(false).highlighter(&highlighter).show(ui);
        },
        State { editor, view: ViewState::new() },
    );
    harness.run_steps(6);
    std::thread::sleep(std::time::Duration::from_millis(200));
    harness.run_steps(3);
    let image = harness.render()?;
    image.save(&out).map_err(|e| e.to_string())?;
    eprintln!("shot: wrote {out} ({}×{})", image.width(), image.height());
    Ok(())
}
