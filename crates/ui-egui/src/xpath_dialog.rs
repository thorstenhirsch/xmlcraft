//! The XPath window: an expression evaluated with the document being shown as the context (`xmlcraft_query`, on a worker thread), the
//! items of the result listed, and a click on a node selecting it in the text.

use std::ops::Range;

use xmlcraft_engine::xpath::{XPathJob, root_namespaces};
use xmlcraft_query::{Evaluated, Options, Phase, QueryError, ResultItem};

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::i18n::{fmt, tl};
use crate::kit::{self, Footer, Tone};
use crate::widgets;

/// The most bytes of a document the window evaluates on.
const MAX_BYTES: usize = 16 * 1024 * 1024;
/// The most characters of an item shown in the list (the whole text is in the tooltip up to this many more).
const SHOWN_CHARS: usize = 110;
/// The most items the list draws.
const LISTED: usize = 500;
/// The height of a row of the list.
const ROW: f32 = 22.0;

/// What an evaluation came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Items(Evaluated),
    Failed(String),
}

/// The XPath window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct XPathForm {
    source: u64,
    name: String,
    expression: String,
    /// The request being waited for.
    waiting: Option<u64>,
    outcome: Option<Outcome>,
    focused: bool,
    /// Also show each step of a path with the items it leaves and its time.
    trace_steps: bool,
    /// The trace of the last evaluation, when it was asked for.
    trace: Option<xmlcraft_query::Trace>,
}

/// The one line that says what an error is.
fn describe(e: &QueryError) -> String {
    let what = match e.phase {
        Phase::Document => tl!("The document cannot be read"),
        Phase::Compile => tl!("The expression is not valid"),
        Phase::Run => tl!("The expression failed"),
    };
    let mut text = if e.code.is_empty() { format!("{what}: {}", e.message) } else { format!("{what} ({}): {}", e.code, e.message) };
    if let Some(span) = &e.span {
        text.push_str(&fmt(tl!(" — at character {at} of the expression"), &[("at", &(span.start + 1).to_string())]));
    }
    text
}

/// One line for an item: what it is and where, then its value.
fn line(item: &ResultItem) -> String {
    let head = match (item.name.is_empty(), &item.path) {
        (false, Some(path)) => format!("{} {}  {path}", item.kind, item.name),
        (false, None) => format!("{} {}", item.kind, item.name),
        (true, Some(path)) => format!("{}  {path}", item.kind),
        (true, None) => item.kind.clone(),
    };
    let flat: String = item.text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut shown: String = flat.chars().take(SHOWN_CHARS).collect();
    if flat.chars().count() > SHOWN_CHARS {
        shown.push('…');
    }
    format!("{head}   {shown}")
}

impl XmlCraftApp {
    /// Open the XPath window on the document being shown (Evaluate XPath…).
    pub(crate) fn open_xpath(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        let Some(doc) = self.session.active_document() else { return };
        if !doc.language().is_xml() {
            self.status = tl!("XPath is evaluated on XML documents.").to_owned();
            return;
        }
        let (source, name) = (doc.id(), doc.name().to_owned());
        let expression = std::mem::take(&mut self.xpath_last);
        self.dialog = Some(Dialog::XPath(Box::new(XPathForm {
            source,
            name,
            expression,
            waiting: None,
            outcome: None,
            focused: false,
            trace_steps: false,
            trace: None,
        })));
    }

    /// Ask the worker to evaluate the expression of the form.
    fn run_xpath(&mut self, form: &mut XPathForm) {
        let Some(doc) = self.session.position_of(form.source).and_then(|i| self.session.document(i)) else {
            form.outcome = Some(Outcome::Failed(tl!("The document is gone.").to_owned()));
            return;
        };
        let text = doc.editor().text();
        if text.len() > MAX_BYTES {
            form.outcome =
                Some(Outcome::Failed(fmt(tl!("The document is larger than {limit} MB."), &[("limit", &(MAX_BYTES / (1024 * 1024)).to_string())])));
            return;
        }
        self.generate_serial += 1;
        let serial = self.generate_serial;
        let options = Options { namespaces: root_namespaces(&text), ..Options::default() };
        self.xpath.submit(XPathJob { serial, text, expression: form.expression.clone(), options, trace: form.trace_steps });
        self.xpath_last = form.expression.clone();
        form.waiting = Some(serial);
    }

    /// Collect what the worker finished. Called every frame.
    pub(crate) fn pump_xpath(&mut self) {
        for done in self.xpath.poll() {
            let Some(Dialog::XPath(form)) = &mut self.dialog else { continue }; // closed meanwhile
            if form.waiting != Some(done.serial) {
                continue; // an older request
            }
            form.waiting = None;
            form.trace = done.trace;
            form.outcome = Some(match done.result {
                Ok(evaluated) => Outcome::Items(evaluated),
                Err(e) => Outcome::Failed(describe(&e)),
            });
        }
    }

    /// Draw the window and act on what was done in it.
    pub(crate) fn show_xpath(&mut self, ctx: &egui::Context) {
        let Some(Dialog::XPath(mut form)) = self.dialog.take() else { return };
        let subtitle = fmt(tl!("An XPath 3.1 expression with {name} as the context."), &[("name", &form.name)]);
        let ((action, jump), closed) =
            kit::dialog(ctx, "xmlcraft-xpath", 680.0, "search", tl!("Evaluate XPath"), &subtitle, |ui| xpath_form(ui, &mut form));
        if let Some(range) = jump {
            self.xpath_last = form.expression.clone();
            self.jump_to(range);
            return;
        }
        match action {
            Some(Footer::Primary) => {
                self.run_xpath(&mut form);
                self.dialog = Some(Dialog::XPath(form));
            }
            Some(Footer::Cancel) => self.xpath_last = form.expression.clone(),
            _ if closed => self.xpath_last = form.expression.clone(),
            _ => self.dialog = Some(Dialog::XPath(form)),
        }
    }
}

fn xpath_form(ui: &mut egui::Ui, form: &mut XPathForm) -> (Option<Footer>, Option<Range<usize>>) {
    let mut submit = false;
    kit::section(ui, tl!("Expression"), |ui| {
        let response = ui.add(
            egui::TextEdit::multiline(&mut form.expression)
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .code_editor()
                .hint_text(tl!("//item[@id = '7']/name")),
        );
        widgets::name_for_assistive_technology(ui, &response, tl!("The XPath expression"));
        if !form.focused {
            response.request_focus();
            form.focused = true;
        }
        if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command) {
            submit = true;
        }
        kit::help(ui, tl!("Ctrl/⌘+Enter evaluates. Prefixes declared on the root element can be used; elements in a default namespace are *:name."));
        ui.add(widgets::checkbox(&mut form.trace_steps, tl!("Show the steps: the items each step of a path leaves, and its time")));
    });
    let mut jump = None;
    if form.waiting.is_some() {
        ui.add_space(kit::GAP_GROUP);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Evaluating…"));
        });
    }
    match &form.outcome {
        None => {}
        Some(Outcome::Failed(message)) => {
            ui.add_space(kit::GAP_GROUP);
            kit::banner(ui, Tone::Bad, message);
        }
        Some(Outcome::Items(done)) => {
            kit::section(ui, tl!("Result"), |ui| {
                let count = done.count.to_string();
                kit::help(
                    ui,
                    &if done.count == 0 {
                        tl!("The result is empty.").to_owned()
                    } else if done.count > done.items.len() {
                        fmt(tl!("{count} items; the first {shown} are listed."), &[("count", &count), ("shown", &done.items.len().to_string())])
                    } else {
                        fmt(tl!("{count} items. Click a node to select it in the text."), &[("count", &count)])
                    },
                );
                egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
                    for item in done.items.iter().take(LISTED) {
                        let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::click());
                        let clickable = item.span.is_some();
                        let text = line(item);
                        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, clickable, text.clone()));
                        let t = crate::theme::Tokens::get(ui.ctx());
                        if clickable && response.hovered() {
                            ui.painter().rect_filled(rect, 4.0, t.hover);
                        }
                        let color = if clickable { t.text } else { t.text_muted };
                        ui.painter().with_clip_rect(rect).text(
                            rect.left_center() + egui::vec2(6.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            &text,
                            egui::FontId::monospace(12.5),
                            color,
                        );
                        let response = if item.cut { response.on_hover_text(tl!("The value is cut.")) } else { response };
                        if clickable && response.clicked() {
                            jump = item.span.clone();
                        }
                    }
                });
            });
        }
    }
    if let Some(trace) = form.trace.as_ref().filter(|_| form.trace_steps) {
        kit::section(ui, tl!("Steps"), |ui| {
            if !trace.is_path {
                kit::help(ui, tl!("This is not a path, so it is one step."));
            }
            let t = crate::theme::Tokens::get(ui.ctx());
            egui::ScrollArea::vertical().id_salt("xmlcraft-xpath-steps").max_height(160.0).auto_shrink([false, true]).show(ui, |ui| {
                for step in &trace.steps {
                    let label = match &step.error {
                        Some(e) => fmt(tl!("{step}  →  {error}"), &[("step", &step.step), ("error", &e.to_string())]),
                        None => fmt(
                            tl!("{step}  →  {count} items, {micros} µs"),
                            &[("step", &step.step), ("count", &step.count.to_string()), ("micros", &step.micros.to_string())],
                        ),
                    };
                    let color = if step.error.is_some() { t.text_muted } else { t.text };
                    ui.add(egui::Label::new(egui::RichText::new(label).monospace().color(color)).truncate());
                }
            });
        });
    }
    let can = form.waiting.is_none() && !form.expression.trim().is_empty();
    let pressed = match kit::footer(ui, tl!("Close"), tl!("Evaluate"), Some("search"), can, tl!("Type an expression first.")) {
        Footer::Nothing => None,
        other => Some(other),
    };
    let pressed = if submit && can { Some(Footer::Primary) } else { pressed };
    (pressed, jump)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, name: &str, path: Option<&str>, text: &str) -> ResultItem {
        ResultItem {
            kind: kind.to_owned(),
            name: name.to_owned(),
            path: path.map(str::to_owned),
            span: None,
            text: text.to_owned(),
            unmapped: false,
            cut: false,
        }
    }

    #[test]
    fn an_item_is_one_line_with_what_it_is_where_it_is_and_what_it_holds() {
        assert_eq!(line(&item("element", "i", Some("/r[1]/i[1]"), "<i/>")), "element i  /r[1]/i[1]   <i/>");
        assert_eq!(line(&item("xs:integer", "", None, "3")), "xs:integer   3");
        let long = item("xs:string", "", None, &"é".repeat(300));
        assert!(line(&long).ends_with('…') && line(&long).chars().count() < 140, "a long value is cut by characters, not bytes");
        assert!(!line(&item("text", "", None, "a\nb")).contains('\n'), "a line break does not break the list");
    }

    #[test]
    fn an_error_says_what_failed_its_code_and_where_in_the_expression() {
        let e = QueryError { phase: Phase::Compile, code: "XPST0003".into(), message: "bad".into(), span: Some(2..3) };
        assert_eq!(describe(&e), "The expression is not valid (XPST0003): bad — at character 3 of the expression");
        let e = QueryError { phase: Phase::Document, code: String::new(), message: "x".into(), span: None };
        assert_eq!(describe(&e), "The document cannot be read: x");
    }
}
