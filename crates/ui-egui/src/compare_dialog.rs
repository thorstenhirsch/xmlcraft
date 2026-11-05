//! The Compare window: the document being shown against another open document, as trees (XML elements, JSON and YAML members) or line
//! by line (`xmlcraft_engine::compare`, on a worker thread). The differences are listed; a click selects one in the first document, a
//! right click in the second.

use std::ops::Range;

use xmlcraft_diff::ChangeKind;
use xmlcraft_engine::compare::{CompareJob, Compared, Ignore, Mode};

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::i18n::{fmt, tl};
use crate::kit::{self, Footer, Tone};
use crate::widgets;

/// The most bytes of one document the window compares.
const MAX_BYTES: usize = 16 * 1024 * 1024;
/// The most differences the list draws.
const LISTED: usize = 1000;
const ROW: f32 = 22.0;

/// What a comparison came to.
pub(crate) type Outcome = Result<Compared, String>;

/// The Compare window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CompareForm {
    first: u64,
    first_name: String,
    /// The other open documents: id and name.
    others: Vec<(u64, String)>,
    /// The index in `others` of the second document.
    second: usize,
    mode: Mode,
    ignore: Ignore,
    waiting: Option<u64>,
    outcome: Option<Outcome>,
}

/// Where a click on a difference goes.
enum Go {
    First(Range<usize>),
    Second(u64, Range<usize>),
}

fn glyph(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "+",
        ChangeKind::Removed => "−",
        ChangeKind::Changed => "≠",
    }
}

impl XmlCraftApp {
    /// Open the Compare window on the document being shown (Compare Documents…).
    pub(crate) fn open_compare(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        let Some(doc) = self.session.active_document() else { return };
        let first = doc.id();
        let others: Vec<(u64, String)> = self.session.documents().iter().filter(|d| d.id() != first).map(|d| (d.id(), d.name().to_owned())).collect();
        if others.is_empty() {
            self.status = tl!("Open a second document to compare this one with.").to_owned();
            return;
        }
        let first_name = doc.name().to_owned();
        self.dialog = Some(Dialog::Compare(Box::new(CompareForm {
            first,
            first_name,
            others,
            second: 0,
            mode: Mode::Auto,
            ignore: Ignore::default(),
            waiting: None,
            outcome: None,
        })));
    }

    fn run_compare(&mut self, form: &mut CompareForm) {
        let Some((second, _)) = form.others.get(form.second) else { return };
        let (Some(a), Some(b)) = (
            self.session.position_of(form.first).and_then(|i| self.session.document(i)),
            self.session.position_of(*second).and_then(|i| self.session.document(i)),
        ) else {
            form.outcome = Some(Err(tl!("A document is gone.").to_owned()));
            return;
        };
        let (old, new) = (a.editor().text(), b.editor().text());
        if old.len() > MAX_BYTES || new.len() > MAX_BYTES {
            form.outcome = Some(Err(fmt(tl!("A document is larger than {limit} MB."), &[("limit", &(MAX_BYTES / (1024 * 1024)).to_string())])));
            return;
        }
        self.generate_serial += 1;
        let serial = self.generate_serial;
        self.comparer.submit(CompareJob {
            serial,
            old,
            old_language: a.language(),
            new,
            new_language: b.language(),
            mode: form.mode,
            ignore: form.ignore,
        });
        form.waiting = Some(serial);
        form.outcome = None;
    }

    /// Collect what the worker finished. Called every frame.
    pub(crate) fn pump_compare(&mut self) {
        for done in self.comparer.poll() {
            let Some(Dialog::Compare(form)) = &mut self.dialog else { continue };
            if form.waiting != Some(done.serial) {
                continue;
            }
            form.waiting = None;
            form.outcome = Some(done.result);
        }
    }

    pub(crate) fn show_compare(&mut self, ctx: &egui::Context) {
        let Some(Dialog::Compare(mut form)) = self.dialog.take() else { return };
        let subtitle = fmt(tl!("{name} against another open document."), &[("name", &form.first_name)]);
        let ((action, go), closed) =
            kit::dialog(ctx, "xmlcraft-compare", 720.0, "files", tl!("Compare documents"), &subtitle, |ui| compare_form(ui, &mut form));
        match go {
            Some(Go::First(range)) => {
                self.jump_to(range);
                return;
            }
            Some(Go::Second(id, range)) => {
                if let Some(index) = self.session.position_of(id) {
                    self.session.set_active(Some(index));
                    self.jump_to(range);
                }
                return;
            }
            None => {}
        }
        match action {
            Some(Footer::Primary) => {
                self.run_compare(&mut form);
                self.dialog = Some(Dialog::Compare(form));
            }
            Some(Footer::Cancel) => {}
            _ if closed => {}
            _ => self.dialog = Some(Dialog::Compare(form)),
        }
    }
}

fn compare_form(ui: &mut egui::Ui, form: &mut CompareForm) -> (Option<Footer>, Option<Go>) {
    kit::section(ui, tl!("Compare with"), |ui| {
        let current = form.others.get(form.second).map_or("", |(_, n)| n.as_str()).to_owned();
        egui::ComboBox::from_id_salt("xmlcraft-compare-second").selected_text(current).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
            for (i, (_, name)) in form.others.iter().enumerate() {
                ui.selectable_value(&mut form.second, i, name);
            }
        });
        ui.add_space(4.0);
        kit::segmented(ui, &mut form.mode, &[(Mode::Auto, tl!("Automatic")), (Mode::Tree, tl!("Elements and members")), (Mode::Lines, tl!("Lines"))]);
    });
    kit::section(ui, tl!("Do not count as a difference"), |ui| {
        ui.add(widgets::checkbox(&mut form.ignore.whitespace, tl!("White space")));
        ui.add(widgets::checkbox(&mut form.ignore.case, tl!("Upper and lower case")));
        if form.mode != Mode::Lines {
            ui.add(widgets::checkbox(&mut form.ignore.comments, tl!("Comments")));
            ui.add(widgets::checkbox(&mut form.ignore.child_order, tl!("The order of elements and list items")));
            ui.add(widgets::checkbox(&mut form.ignore.numbers_by_value, tl!("How a number is written (1.0 and 1)")));
            ui.add(widgets::checkbox(&mut form.ignore.prefixes, tl!("Namespace prefixes")));
        }
    });
    let mut go = None;
    if form.waiting.is_some() {
        ui.add_space(kit::GAP_GROUP);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Comparing…"));
        });
    }
    match &form.outcome {
        None => {}
        Some(Err(message)) => {
            ui.add_space(kit::GAP_GROUP);
            kit::banner(ui, Tone::Bad, message);
        }
        Some(Ok(done)) => {
            kit::section(ui, tl!("Differences"), |ui| {
                if let Some(note) = &done.note {
                    kit::help(ui, note);
                }
                let count = done.count.to_string();
                kit::help(
                    ui,
                    &if done.count == 0 {
                        tl!("The documents are the same.").to_owned()
                    } else if done.count > done.differences.len().min(LISTED) {
                        fmt(
                            tl!(
                                "{count} differences; the first {shown} are listed. A click selects one in the first document, a right click in the second."
                            ),
                            &[("count", &count), ("shown", &done.differences.len().min(LISTED).to_string())],
                        )
                    } else {
                        fmt(tl!("{count} differences. A click selects one in the first document, a right click in the second."), &[("count", &count)])
                    },
                );
                let second = form.others.get(form.second).map(|(id, _)| *id);
                egui::ScrollArea::vertical().max_height(240.0).auto_shrink([false, true]).show(ui, |ui| {
                    for difference in done.differences.iter().take(LISTED) {
                        let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::click());
                        let text = format!("{}  {}", glyph(difference.kind), difference.label.replace(['\n', '\r'], " "));
                        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, text.clone()));
                        let t = crate::theme::Tokens::get(ui.ctx());
                        if response.hovered() {
                            ui.painter().rect_filled(rect, 4.0, t.hover);
                        }
                        ui.painter().with_clip_rect(rect).text(
                            rect.left_center() + egui::vec2(6.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            &text,
                            egui::FontId::monospace(12.5),
                            t.text,
                        );
                        if response.clicked() {
                            go = Some(Go::First(difference.old.clone()));
                        } else if response.secondary_clicked()
                            && let Some(id) = second
                        {
                            go = Some(Go::Second(id, difference.new.clone()));
                        }
                    }
                });
            });
        }
    }
    let can = form.waiting.is_none();
    let pressed = match kit::footer(ui, tl!("Close"), tl!("Compare"), Some("files"), can, "") {
        Footer::Nothing => None,
        other => Some(other),
    };
    (pressed, go)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_of_difference_has_its_own_mark() {
        let marks = [glyph(ChangeKind::Added), glyph(ChangeKind::Removed), glyph(ChangeKind::Changed)];
        assert_eq!(marks, ["+", "−", "≠"]);
    }
}
