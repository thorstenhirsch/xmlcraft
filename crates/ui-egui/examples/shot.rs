//! Headless screenshot of the real XmlCraft shell (egui_kittest + wgpu, no window needed).
//!
//! ```text
//! cargo run -p xmlcraft-ui-egui --example shot -- out.png [--size 1440x900] [--scale 2] [--theme light|dark|system]
//!     [--tabs a.xml,b.xml] [--active 0] [--width 1200] [--open FILE.xml [--find TEXT] [--goto LINE:COL]]
//! ```
//! `--width N` downscales the image to N pixels wide (README screenshots). `--tabs` shows sample document tabs (the engine's
//! documents replace them with milestone M1.12). `--open` reads a real file from disk and opens it (use a synthetic one), and `--find`,
//! which needs `--open`, opens the find bar with that text; `--goto`, also with `--open`, puts the caret at a line and column (from 1). Only synthetic names and files are ever shown.

use std::time::Duration;

use egui_kittest::Harness;
use xmlcraft_ui_egui::theme::ThemePreference;
use xmlcraft_ui_egui::{TabInfo, XmlCraftApp};

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let out = args.next().ok_or(
        "usage: shot <out.png> [--size WxH] [--scale N] [--theme light|dark|system] [--tabs a,b] [--active N] [--width N] [--open FILE [--find TEXT] [--goto LINE:COL]]",
    )?;
    let (mut size, mut scale) = (egui::vec2(1440.0, 900.0), 2.0f32);
    let (mut width, mut theme, mut tabs, mut active): (Option<u32>, Option<ThemePreference>, Vec<TabInfo>, Option<usize>) =
        (None, None, Vec::new(), None);
    let (mut open, mut find, mut goto): (Option<std::path::PathBuf>, Option<String>, Option<(usize, usize)>) = (None, None, None);
    while let Some(a) = args.next() {
        match a.strip_prefix("--") {
            Some("size") => {
                let v = args.next().unwrap_or_default();
                let (w, h) = v.split_once('x').ok_or("--size WxH")?;
                size = egui::vec2(w.parse().map_err(|_| "bad width")?, h.parse().map_err(|_| "bad height")?);
            }
            Some("open") => open = Some(args.next().ok_or("--open needs a file")?.into()),
            Some("find") => find = Some(args.next().ok_or("--find needs the text to look for")?),
            Some("goto") => {
                let v = args.next().unwrap_or_default();
                let (line, column) = v.split_once(':').ok_or("--goto LINE:COL")?;
                goto = Some((line.parse().map_err(|_| "bad line")?, column.parse().map_err(|_| "bad column")?));
            }
            Some("scale") => scale = args.next().and_then(|v| v.parse().ok()).ok_or("bad --scale")?,
            Some("width") => width = Some(args.next().and_then(|v| v.parse().ok()).ok_or("bad --width")?),
            Some("active") => active = Some(args.next().and_then(|v| v.parse().ok()).ok_or("bad --active")?),
            Some("tabs") => {
                // A trailing `*` marks a tab with unsaved changes.
                tabs = args
                    .next()
                    .unwrap_or_default()
                    .split(',')
                    .filter(|n| !n.is_empty())
                    .map(|n| TabInfo { name: n.trim_end_matches('*').to_owned(), dirty: n.ends_with('*') })
                    .collect();
            }
            Some("theme") => {
                theme = Some(match args.next().as_deref() {
                    Some("light") => ThemePreference::Light,
                    Some("dark") => ThemePreference::Dark,
                    Some("system") => ThemePreference::System,
                    other => return Err(format!("--theme light|dark|system, not {other:?}")),
                });
            }
            _ => return Err(format!("unexpected argument {a}")),
        }
    }
    if (find.is_some() || goto.is_some()) && open.is_none() {
        return Err("--find and --goto need --open FILE: they work on that file".into());
    }
    let mut harness = Harness::builder().with_size(size).with_pixels_per_point(scale).build_eframe(move |cc| {
        let mut app = XmlCraftApp::new(cc);
        if let Some(theme) = theme {
            app.theme_preference = theme;
        }
        // The document index of each tab named: a repeated name is the same document (the session opens a file once), so `--active N`
        // goes through this list rather than counting tabs.
        let mut documents: Vec<Option<usize>> = Vec::new();
        for tab in &tabs {
            let opened = app.session.open(Some(std::path::PathBuf::from(&tab.name)), b"", None).ok();
            if opened.is_some()
                && tab.dirty
                && let Some(doc) = app.session.active_document_mut()
            {
                doc.editor_mut().type_text(" ");
            }
            documents.push(opened);
        }
        app.session.set_active(active.and_then(|i| documents.get(i).copied().flatten()));
        app
    });
    // Let fonts install and layout settle; hover and press washes ease for a moment.
    harness.run_steps(8);
    if let Some(path) = &open {
        use egui_kittest::kittest::Queryable;
        if !harness.state_mut().open_path(path) {
            return Err(format!("could not open {}", path.display()));
        }
        harness.run_steps(6);
        if let Some((line, column)) = goto
            && let Some(doc) = harness.state_mut().session.active_document_mut()
        {
            let at = doc.editor().buffer().offset_of(xmlcraft_text::Position { line: line.saturating_sub(1), col: column.saturating_sub(1) });
            doc.editor_mut().place_caret(at, false);
        }
        if let Some(query) = &find {
            harness.state_mut().execute("edit.find");
            harness.run_steps(3);
            harness.get_by_label("Find").type_text(query);
            harness.run_steps(4);
        }
        // The analysis runs on another thread: wait until the problems of the file are in (five seconds at most).
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while harness.state().problems().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            harness.run_steps(1);
        }
        harness.run_steps(4);
    }
    std::thread::sleep(Duration::from_millis(250));
    harness.run_steps(4);
    let mut image = harness.render()?;
    if let Some(w) = width.filter(|w| *w < image.width()) {
        let h = (f64::from(image.height()) * f64::from(w) / f64::from(image.width())).round() as u32;
        image = image::imageops::resize(&image, w, h, image::imageops::FilterType::Lanczos3);
    }
    image.save(&out).map_err(|e| e.to_string())?;
    eprintln!("shot: wrote {out} ({}×{})", image.width(), image.height());
    Ok(())
}
