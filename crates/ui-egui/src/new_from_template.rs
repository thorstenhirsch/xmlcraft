//! New from Template: a list of kinds of document to start from; the one chosen becomes a new document that is not a file yet.

use xmlcraft_engine::templates::TEMPLATES;

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::i18n::tl;
use crate::kit::{self, Footer};
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
        let (mut action, closed) = kit::dialog(
            ctx,
            "xmlcraft-new-from-template",
            640.0,
            "layout-grid",
            tl!("New from Template"),
            tl!("What kind of document do you want to start from?"),
            |ui| match &mut dialog {
                Dialog::NewFromTemplate(form) => form_ui(ui, form),
                _ => None,
            },
        );
        if action.is_none() && closed {
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

/// The icon of a kind of document.
fn icon_of(id: &str) -> &'static str {
    match id {
        "xsd" => "book-open",
        "dtd" => "file-code",
        "xhtml" => "globe",
        "xslt" => "wand-sparkles",
        "catalog" => "files",
        "xml-dtd" => "shield-check",
        _ => "code-xml",
    }
}

fn form_ui(ui: &mut egui::Ui, form: &mut TemplateForm) -> Option<Action> {
    let mut action = None;
    ui.add_space(2.0);
    let gap = 10.0;
    let width = (ui.available_width() - gap) / 2.0;
    for pair in TEMPLATES.chunks(2).enumerate() {
        let (row, templates) = pair;
        // The two cards of a row are as tall as the taller.
        let height = templates.iter().map(|template| card_height(ui, width, template.description)).fold(0.0, f32::max);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (column, template) in templates.iter().enumerate() {
                let index = row * 2 + column;
                let response = card(ui, width, height, icon_of(template.id), template.title, template.description, form.selected == index);
                if response.clicked() {
                    form.selected = index;
                }
                // A double click is a click first: the card is the one chosen by now.
                if response.double_clicked() {
                    action = Some(Action::Create);
                }
            }
        });
        ui.add_space(gap);
    }
    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        action = Some(Action::Create);
    }
    match kit::footer(ui, tl!("Cancel"), tl!("Create"), Some("file-plus"), true, "") {
        Footer::Primary => action = Some(Action::Create),
        Footer::Cancel => action = Some(Action::Cancel),
        Footer::Nothing => {}
    }
    action
}

/// The width the description of a card has.
fn text_width(card: f32) -> f32 {
    card - 16.0 - 40.0 - 12.0 - 14.0
}

/// How tall a card is for its description.
fn card_height(ui: &egui::Ui, width: f32, description: &str) -> f32 {
    let galley = ui.painter().layout(description.to_owned(), theme::regular(11.5), Tokens::get(ui.ctx()).text_muted, text_width(width));
    (14.0 + 20.0 + galley.size().y + 14.0).max(72.0)
}

/// One kind of document: an icon tile, its name and what it is for, outlined in the accent when it is the one chosen. It is named by its
/// title for the assistive technology. A click chooses it, a double click chooses and creates.
fn card(ui: &mut egui::Ui, width: f32, height: f32, icon: &str, title: &str, description: &str, selected: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let galley = ui.painter().layout(description.to_owned(), theme::regular(11.5), t.text_muted, text_width(width));
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, selected, title));
    let press = theme::Press::track(ui, &response);
    let fill = if selected { t.accent_soft } else { press.fill(t.card, t.hover, t.pressed) };
    let edge = if selected { t.accent } else { t.border.lerp_to_gamma(t.text_muted, press.hover * 0.4) };
    ui.painter().rect(rect, 10.0, fill, egui::Stroke::new(if selected { 1.5 } else { 1.0 }, edge), egui::StrokeKind::Inside);
    let tile = egui::Rect::from_min_size(rect.min + egui::vec2(14.0, 14.0), egui::vec2(40.0, 40.0));
    ui.painter().rect_filled(tile, 10.0, if selected { t.card } else { t.accent_soft });
    crate::icons::paint(ui, tile, icon, 20.0, t.accent_text);
    let at = rect.min + egui::vec2(66.0, 14.0);
    ui.painter().text(at, egui::Align2::LEFT_TOP, title, theme::semibold(13.0), t.text);
    ui.painter().galley(at + egui::vec2(0.0, 20.0), galley, t.text_muted);
    theme::hand(response)
}
