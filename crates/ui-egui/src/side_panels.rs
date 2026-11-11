//! The Outline panel's body: the element tree of the document being shown, with a filter. The Workbench ([`crate::workbench`]) hosts
//! it with the other panels.

use egui::vec2;
use xmlcraft_engine::analysis::Skipped;

use crate::i18n::tl;
use crate::theme::{self, Tokens};
use crate::{XmlCraftApp, widgets};

fn pane_hint(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(16, 0)).show(ui, |ui| {
        ui.label(egui::RichText::new(text).font(theme::regular(12.0)).color(t.text_faint));
    });
}

/// Rows of a list: tall enough to click.
const ROW: f32 = 24.0;

/// The Outline's rows (indexes into the analysis's outline) for one filter, one document and one analysis; kept between frames so that
/// 50,000 names are not lowercased and compared again every frame.
pub(crate) struct OutlineRows {
    filter: String,
    document: u64,
    version: u64,
    rows: Vec<usize>,
}

pub(crate) fn outline_body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    let t = Tokens::get(ui.ctx());
    let Some(doc) = app.session.active_document() else {
        widgets::empty_state(ui, 24.0, tl!("No document open"), tl!("The element tree of the open file appears here."));
        return;
    };
    let (caret, document) = (doc.editor().primary().head(), doc.id());
    egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 12, top: 0, bottom: 6 }).show(ui, |ui| {
        let filter = ui.add(egui::TextEdit::singleline(&mut app.outline_filter).hint_text(tl!("Filter elements")).desired_width(f32::INFINITY));
        widgets::name_for_assistive_technology(ui, &filter, tl!("Filter elements"));
    });
    let filter = app.outline_filter.trim().to_lowercase();
    let cached = app.outline_rows.take();
    let Some(analysis) = app.active_analysis() else {
        widgets::empty_state(ui, 12.0, tl!("Reading the document…"), "");
        return;
    };
    match analysis.skipped {
        Some(Skipped::TooLarge) => {
            return widgets::empty_state(ui, 12.0, tl!("Not checked"), tl!("The document is too large for an outline while typing."));
        }
        Some(Skipped::Failed) => return widgets::empty_state(ui, 12.0, tl!("The check failed"), tl!("This is a bug. The document is unchanged.")),
        None => {}
    }
    if analysis.outline.is_empty() {
        widgets::empty_state(ui, 12.0, tl!("No elements"), tl!("The element tree appears once the document has a root element."));
        return;
    }
    let rows = match cached.filter(|c| c.filter == filter && c.document == document && c.version == analysis.version) {
        Some(rows) => rows,
        None => {
            let matching = analysis
                .outline
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    filter.is_empty()
                        || item.name.to_lowercase().contains(&filter)
                        || item.label.as_ref().is_some_and(|l| l.to_lowercase().contains(&filter))
                })
                .map(|(i, _)| i)
                .collect();
            OutlineRows { filter, document, version: analysis.version, rows: matching }
        }
    };
    // The innermost element around the caret.
    let current = analysis.outline.iter().rposition(|item| item.range.start <= caret && caret <= item.range.end);
    // Above the list, where it is seen (below a list that fills the pane it never would be).
    if analysis.outline_truncated {
        pane_hint(ui, tl!("Only the first elements are listed."));
    }
    let mut clicked = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW, rows.rows.len(), |ui, range| {
        for &index in rows.rows.get(range).unwrap_or_default() {
            let Some(item) = analysis.outline.get(index) else { continue };
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), egui::Sense::click());
            let label = match &item.label {
                Some(label) => format!("{} {label}", item.name),
                None => item.name.clone(),
            };
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label.clone()));
            if Some(index) == current {
                ui.painter().rect_filled(rect, 0.0, t.selected);
            } else if response.hovered() {
                ui.painter().rect_filled(rect, 0.0, t.hover);
            }
            let indent = 14.0 + item.depth.min(20) as f32 * 12.0;
            let clip = ui.painter().with_clip_rect(rect);
            let name = clip.text(rect.left_center() + vec2(indent, 0.0), egui::Align2::LEFT_CENTER, &item.name, theme::medium(12.5), t.text);
            if let Some(hint) = &item.label {
                clip.text(egui::pos2(name.right() + 8.0, rect.center().y), egui::Align2::LEFT_CENTER, hint, theme::regular(12.0), t.text_faint);
            }
            if response.clicked() {
                clicked = Some(item.name_range.clone());
            }
        }
    });
    app.outline_rows = Some(rows);
    if let Some(range) = clicked {
        app.jump_to(range.start..range.start);
    }
}
