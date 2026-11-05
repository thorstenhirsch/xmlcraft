//! The Merge window: the document being shown (ours) and another open document (theirs), both made from a third (the base), merged line
//! by line (`xmlcraft_diff::merge3`) into a new document. What both changed in the same place is a conflict, settled as asked: kept as
//! markers (`<<<<<<<`, `|||||||`, `=======`, `>>>>>>>`) to be settled in the text, or taken from ours, theirs, both or the base.

use xmlcraft_diff::{Choice, LineOptions, Whitespace, merge3};

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::i18n::{fmt, tl};
use crate::kit::{self, Footer, Tone};
use crate::widgets;

/// The most bytes of one document the window merges.
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// The Merge window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MergeForm {
    ours: u64,
    ours_name: String,
    /// The other open documents: id and name.
    others: Vec<(u64, String)>,
    base: usize,
    theirs: usize,
    conflicts: Choice,
    ignore_whitespace: bool,
    error: Option<String>,
}

/// The name of the merged document: that of ours with `-merged`.
fn merged_name(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}-merged.{ext}"),
        None => format!("{name}-merged"),
    }
}

impl XmlCraftApp {
    /// Open the Merge window with the document being shown as ours (Merge Documents…).
    pub(crate) fn open_merge(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        let Some(doc) = self.session.active_document() else { return };
        let ours = doc.id();
        let others: Vec<(u64, String)> = self.session.documents().iter().filter(|d| d.id() != ours).map(|d| (d.id(), d.name().to_owned())).collect();
        if others.len() < 2 {
            self.status = tl!("A merge needs three documents: this one, the base it and the other were made from, and the other.").to_owned();
            return;
        }
        self.dialog = Some(Dialog::Merge(Box::new(MergeForm {
            ours,
            ours_name: doc.name().to_owned(),
            others,
            base: 0,
            theirs: 1,
            conflicts: Choice::Markers,
            ignore_whitespace: false,
            error: None,
        })));
    }

    /// Merge with the options of the form and open the result.
    fn run_merge(&mut self, form: &mut MergeForm) -> bool {
        let text_of = |app: &Self, id: u64| app.session.position_of(id).and_then(|i| app.session.document(i)).map(|d| d.editor().text());
        let (Some(base_id), Some(theirs_id)) = (form.others.get(form.base).map(|o| o.0), form.others.get(form.theirs).map(|o| o.0)) else {
            return false;
        };
        let (Some(ours), Some(base), Some(theirs)) = (text_of(self, form.ours), text_of(self, base_id), text_of(self, theirs_id)) else {
            form.error = Some(tl!("A document is gone.").to_owned());
            return false;
        };
        if ours.len().max(base.len()).max(theirs.len()) > MAX_BYTES {
            form.error = Some(fmt(tl!("A document is larger than {limit} MB."), &[("limit", &(MAX_BYTES / (1024 * 1024)).to_string())]));
            return false;
        }
        if form.base == form.theirs {
            form.error = Some(tl!("The base and the other document are the same one: choose two different documents.").to_owned());
            return false;
        }
        let options = LineOptions {
            whitespace: if form.ignore_whitespace { Whitespace::Collapse } else { Whitespace::Exact },
            ignore_blank_lines: form.ignore_whitespace,
            ..LineOptions::default()
        };
        let merged = merge3(&base, &ours, &theirs, &options);
        let (text, count) = (merged.render(form.conflicts), merged.conflict_count());
        let name = merged_name(&form.ours_name);
        self.open_generated(&name, &text);
        self.status = match (count, form.conflicts) {
            (0, _) => fmt(tl!("Merged into {name} with no conflicts"), &[("name", &name)]),
            (n, Choice::Markers) => {
                fmt(tl!("Merged into {name}; {count} conflicts are marked in the text"), &[("name", &name), ("count", &n.to_string())])
            }
            (n, _) => fmt(tl!("Merged into {name}; {count} conflicts were settled as chosen"), &[("name", &name), ("count", &n.to_string())]),
        };
        true
    }

    pub(crate) fn show_merge(&mut self, ctx: &egui::Context) {
        let Some(Dialog::Merge(mut form)) = self.dialog.take() else { return };
        let subtitle = fmt(tl!("Merge {name} with another document, both made from a common base."), &[("name", &form.ours_name)]);
        let (action, closed) = kit::dialog(ctx, "xmlcraft-merge", 560.0, "files", tl!("Merge documents"), &subtitle, |ui| merge_form(ui, &mut form));
        match action {
            Some(Footer::Primary) => {
                if self.run_merge(&mut form) {
                    return;
                }
                self.dialog = Some(Dialog::Merge(form));
            }
            Some(Footer::Cancel) => {}
            _ if closed => {}
            _ => self.dialog = Some(Dialog::Merge(form)),
        }
    }
}

fn merge_form(ui: &mut egui::Ui, form: &mut MergeForm) -> Option<Footer> {
    let others = form.others.clone();
    let pick = |ui: &mut egui::Ui, id: &str, label: &str, value: &mut usize| {
        kit::row(ui, label, "", |ui| {
            egui::ComboBox::from_id_salt(id).selected_text(others.get(*value).map_or("", |(_, n)| n.as_str()).to_owned()).width(260.0).show_ui(
                ui,
                |ui| {
                    for (i, (_, name)) in others.iter().enumerate() {
                        ui.selectable_value(value, i, name);
                    }
                },
            );
        });
    };
    kit::section(ui, tl!("The three documents"), |ui| {
        kit::row(ui, tl!("Ours"), tl!("The document being shown."), |ui| {
            ui.label(&form.ours_name);
        });
        pick(ui, "xmlcraft-merge-base", tl!("Base"), &mut form.base);
        pick(ui, "xmlcraft-merge-theirs", tl!("Theirs"), &mut form.theirs);
    });
    kit::section(ui, tl!("Where both changed the same lines"), |ui| {
        kit::segmented(
            ui,
            &mut form.conflicts,
            &[
                (Choice::Markers, tl!("Mark")),
                (Choice::Ours, tl!("Ours")),
                (Choice::Theirs, tl!("Theirs")),
                (Choice::Both, tl!("Both")),
                (Choice::Base, tl!("Base")),
            ],
        );
        ui.add_space(4.0);
        ui.add(widgets::checkbox(&mut form.ignore_whitespace, tl!("White space does not count as a change")));
    });
    if let Some(message) = &form.error {
        ui.add_space(kit::GAP_GROUP);
        kit::banner(ui, Tone::Bad, message);
    }
    match kit::footer(ui, tl!("Cancel"), tl!("Merge"), Some("files"), true, "") {
        Footer::Nothing => None,
        other => Some(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_merged_document_is_named_after_ours() {
        assert_eq!(merged_name("order.xml"), "order-merged.xml");
        assert_eq!(merged_name("a.b.json"), "a.b-merged.json");
        assert_eq!(merged_name("Untitled-1"), "Untitled-1-merged");
    }
}
