//! Headless screenshot of the editor with the real theme and XML colouring (synthetic text only).
//!
//! ```text
//! cargo run -p xmlcraft-ui-egui --example editor_shot -- out.png [--theme light|dark] [--size 1000x560] [--scale 2]
//! ```

use egui::Id;
use egui_kittest::Harness;
use xmlcraft_code_editor::{CodeEditor, ViewState};
use xmlcraft_editor::Editor;
use xmlcraft_text::Selection;
use xmlcraft_ui_egui::editor_view::{XmlStyler, editor_colors};
use xmlcraft_ui_egui::theme::{ThemeKind, Tokens};

const SAMPLE: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<!DOCTYPE library [\n\
  <!ELEMENT library (book+)>\n\
  <!ENTITY pub \"Synthetic House\">\n\
]>\n\
<!-- A synthetic library: every kind of markup the colouring knows -->\n\
<?render mode=\"fast\"?>\n\
<library xmlns=\"urn:example:library\" xmlns:x=\"urn:example:extra\">\n\
\t<book id=\"b1\" x:rank=\"1\" lang='de'>\n\
\t\t<title>Fish &amp; Chips &#x2764; &pub;</title>\n\
\t\t<summary><![CDATA[Raw <text> & more]]></summary>\n\
\t\t<note>5 < 6 &oops</note>\n\
\t</book>\n\
</library>\n";

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let out = args.next().ok_or("usage: editor_shot <out.png> [--theme light|dark] [--size WxH] [--scale N]")?;
    let (mut size, mut scale, mut kind) = (egui::vec2(1000.0, 560.0), 2.0f32, ThemeKind::Light);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--theme" => {
                kind = match args.next().as_deref() {
                    Some("dark") => ThemeKind::Dark,
                    Some("light") => ThemeKind::Light,
                    other => return Err(format!("--theme is light or dark, not {}", other.unwrap_or("missing"))),
                }
            }
            "--scale" => {
                let arg = args.next().unwrap_or_default();
                scale = arg
                    .parse::<f32>()
                    .ok()
                    .filter(|s| s.is_finite() && (0.5..=8.0).contains(s))
                    .ok_or(format!("--scale is a number from 0.5 to 8, not {arg:?}"))?;
            }
            "--size" => {
                let v = args.next().unwrap_or_default();
                let dimension = |text: &str| text.parse::<f32>().ok().filter(|d| d.is_finite() && (100.0..=4096.0).contains(d));
                let (w, h) = v
                    .split_once('x')
                    .and_then(|(w, h)| Some((dimension(w)?, dimension(h)?)))
                    .ok_or(format!("--size is WxH with each from 100 to 4096, not {v:?}"))?;
                size = egui::vec2(w, h);
            }
            _ => return Err(format!("unexpected argument {a}")),
        }
    }
    struct State {
        editor: Editor,
        view: ViewState,
        styler: XmlStyler,
        kind: ThemeKind,
        fonts_installed: bool,
    }
    let mut editor = Editor::new(SAMPLE);
    let start = SAMPLE.find("Fish").unwrap_or(0);
    editor.set_selections(&[Selection { anchor: start, head: start + 4 }]);
    let mut harness = Harness::builder().with_size(size).with_pixels_per_point(scale).build_ui_state(
        |ui, state: &mut State| {
            // The app's own fonts (JetBrains Mono for code), installed before anything is laid out: they take effect next frame.
            if !state.fonts_installed {
                xmlcraft_ui_egui::theme::install_fonts(ui.ctx());
                state.fonts_installed = true;
                return;
            }
            let tokens = Tokens::for_kind(state.kind);
            CodeEditor::new(Id::new("shot"), &mut state.editor, &mut state.view, editor_colors(&tokens))
                .blink(false)
                .styler(&mut state.styler)
                .show(ui);
        },
        State { editor, view: ViewState::new(), styler: XmlStyler::new(kind), kind, fonts_installed: false },
    );
    harness.run_steps(6);
    std::thread::sleep(std::time::Duration::from_millis(200));
    harness.run_steps(3);
    let image = harness.render()?;
    image.save(&out).map_err(|e| e.to_string())?;
    eprintln!("editor_shot: wrote {out} ({}×{})", image.width(), image.height());
    Ok(())
}
