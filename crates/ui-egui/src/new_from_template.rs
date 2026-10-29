//! New from Template: a list of kinds of document to start from; the one chosen becomes a new document that is not a file yet.

use xmlcraft_engine::templates::TEMPLATES;

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::i18n::tl;
use crate::theme::{self, Tokens};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TemplateForm {
    /// The index of the template that is chosen.
    selected: usize,
}

enum Action {
    Create,
    Cancel,
}

impl XmlCraftApp {
    /// Ask which kind of document to start from (File ▸ New from Template…).
    pub(crate) fn open_new_from_template(&mut self) {
        self.dialog = Some(Dialog::NewFromTemplate(Box::new(TemplateForm { selected: 0 })));
    }

    /// A new document made from the template with this id, shown. `false` when there is none.
    pub fn new_document_from_template(&mut self, id: &str) -> bool {
        let Some(template) = xmlcraft_engine::templates::template(id) else { return false };
        self.open_generated(template.name, template.text);
        true
    }

    pub(crate) fn show_new_from_template(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let t = Tokens::get(ctx);
        let mut action = None;
        let response = egui::Modal::new(egui::Id::unique("xmlcraft-new-from-template")).show(ctx, |ui| {
            ui.set_width(460.0);
            if let Dialog::NewFromTemplate(form) = &mut dialog {
                action = form_ui(ui, &t, form);
            }
        });
        if action.is_none() && response.should_close() {
            action = Some(Action::Cancel);
        }
        match (action, &dialog) {
            (Some(Action::Create), Dialog::NewFromTemplate(form)) => {
                if let Some(template) = TEMPLATES.get(form.selected) {
                    self.new_document_from_template(template.id);
                }
            }
            (Some(_), _) => {}
            (None, _) => self.dialog = Some(dialog),
        }
    }
}

fn form_ui(ui: &mut egui::Ui, t: &Tokens, form: &mut TemplateForm) -> Option<Action> {
    let mut action = None;
    ui.label(egui::RichText::new(tl!("New from Template")).font(theme::semibold(15.0)).color(t.text));
    ui.add_space(4.0);
    ui.label(egui::RichText::new(tl!("What kind of document do you want to start from?")).font(theme::regular(13.0)).color(t.text_muted));
    ui.add_space(8.0);
    for (index, template) in TEMPLATES.iter().enumerate() {
        let response = row(ui, t, template.title, template.description, form.selected == index);
        if response.clicked() {
            form.selected = index;
        }
        // A double click is a click first: the row is the one chosen by now.
        if response.double_clicked() {
            action = Some(Action::Create);
        }
    }
    ui.add_space(10.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let create = egui::Button::new(egui::RichText::new(tl!("Create")).color(egui::Color32::WHITE)).fill(t.accent);
        if ui.add(create).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            action = Some(Action::Create);
        }
        if ui.button(tl!("Cancel")).clicked() {
            action = Some(Action::Cancel);
        }
    });
    action
}

/// One kind of document: its name and what it is for, with a wash when it is the one chosen. It is named by its title for the
/// assistive technology. A click chooses it, a double click chooses and creates.
fn row(ui: &mut egui::Ui, t: &Tokens, title: &str, description: &str, selected: bool) -> egui::Response {
    let width = ui.available_width();
    // The height is what the description needs (it wraps), under the title.
    let galley = ui.painter().layout(description.to_owned(), theme::regular(12.0), t.text_muted, width - 24.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, (7.0 + 18.0 + galley.size().y + 7.0).max(46.0)), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, selected, title));
    let fill = if selected {
        t.accent_soft
    } else if response.hovered() {
        t.hover
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 6.0, fill);
    let at = rect.left_top() + egui::vec2(12.0, 7.0);
    ui.painter().text(at, egui::Align2::LEFT_TOP, title, theme::medium(13.0), t.text);
    ui.painter().galley(at + egui::vec2(0.0, 18.0), galley, t.text_muted);
    response
}
