//! The XQuery tab of the Query tool window: a query run on a worker thread (`xmlcraft_xquery`), with the document being shown as
//! its context item when it is XML, or the document's own text as the query when it is an XQuery (`.xq`, `.xquery`, …). The items
//! of the result are listed (a click on a node of the document selects it in the text); the result serialized can be opened as a
//! new document. External variables are given in a table like the XSLT parameters. Nothing is changed.

use std::ops::Range;

use xmlcraft_engine::xquery::XQueryJob;
use xmlcraft_xquery::{Evaluated, Options, ParamValue, Phase, QueryError, Serialization, Variable};

use crate::i18n::{fmt, tl};
use crate::kit::{self, Tone};
use crate::tools::query::line;
use crate::tools::xpath_lab::{self, Marks};
use crate::{Panel, XmlCraftApp, widgets};

/// The most bytes of a document or query the tab works on.
const MAX_BYTES: usize = 16 * 1024 * 1024;
/// The most items the list draws.
const LISTED: usize = 500;
/// The most variables in the table.
const MAX_VARIABLES: usize = 64;
const ROW: f32 = 22.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum VarKind {
    #[default]
    Text,
    Number,
    Boolean,
}

impl VarKind {
    fn label(self) -> &'static str {
        match self {
            VarKind::Text => tl!("string"),
            VarKind::Number => tl!("number"),
            VarKind::Boolean => tl!("boolean"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct VarRow {
    name: String,
    value: String,
    kind: VarKind,
}

/// How the result is written when it is opened as a document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Method {
    #[default]
    Xml,
    Json,
    Text,
    Adaptive,
}

impl Method {
    fn serialization(self) -> Serialization {
        match self {
            Method::Xml => Serialization::Xml,
            Method::Json => Serialization::Json,
            Method::Text => Serialization::Text,
            Method::Adaptive => Serialization::Adaptive,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Method::Xml => "xml",
            Method::Json => "json",
            Method::Text | Method::Adaptive => "txt",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Failure {
    text: String,
    /// Where in the query.
    span: Option<Range<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Answer {
    /// The document that was the context, and its version (the items' places are only good for that text).
    context: Option<(u64, u64)>,
    /// The query that was run (an error's place is in it).
    query: String,
    outcome: Result<(Evaluated, Option<String>), Failure>,
    method: Method,
}

/// The XQuery tab's form and its last answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct XQueryTab {
    pub(crate) query: String,
    vars: Vec<VarRow>,
    method: Method,
    waiting: Option<u64>,
    answer: Option<Answer>,
    /// Give the keyboard to the query field at the next frame.
    pub(crate) focus: bool,
    /// Run as soon as the tab is drawn (the command bar's Run on an XQuery document).
    pub(crate) run_now: bool,
}

/// What a frame of the tab asked for.
#[derive(Default)]
pub(crate) struct Asked {
    pub(crate) run: bool,
    pub(crate) jump: Option<Range<usize>>,
    pub(crate) open_result: bool,
}

fn describe(e: &QueryError) -> String {
    let what = match e.phase {
        Phase::Document => tl!("The document cannot be read"),
        Phase::Compile => tl!("The query is not valid"),
        Phase::Run => tl!("The query failed"),
    };
    if e.code.is_empty() { format!("{what}: {}", e.message) } else { format!("{what}: {} {}", e.code, e.message) }
}

/// Line and column (1-based, in characters) of a byte offset of `text`.
fn line_column(text: &str, at: usize) -> (usize, usize) {
    let before = text.get(..at.min(text.len())).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

/// The external variables a query declares (`declare variable $x external;`), when it reads.
fn declared_variables(query: &str) -> Vec<String> {
    let Ok(module) = xmlcraft_xquery::parse::parse(query, &[]) else { return Vec::new() };
    module
        .prolog
        .iter()
        .filter_map(|d| match &d.value {
            xmlcraft_xquery::ast::Decl::Variable { name, external: true, .. } => Some(xmlcraft_xquery::ast::written(name)),
            _ => None,
        })
        .collect()
}

impl XQueryTab {
    /// Whether Run can be pressed: nothing is running, and there is a query (typed, or the shown document).
    pub(crate) fn can_run(&self, shown_query: bool) -> bool {
        self.waiting.is_none() && (shown_query || !self.query.trim().is_empty())
    }

    fn variables(&self) -> Result<Vec<Variable>, String> {
        let mut out = Vec::new();
        for row in self.vars.iter().filter(|r| !r.name.trim().is_empty()) {
            let value = match row.kind {
                VarKind::Text => ParamValue::String(row.value.clone()),
                VarKind::Number => {
                    if row.value.trim().parse::<f64>().is_err() {
                        return Err(fmt(tl!("${name} is not a number."), &[("name", row.name.trim())]));
                    }
                    ParamValue::Number(row.value.trim().to_owned())
                }
                VarKind::Boolean => match row.value.trim() {
                    "true" | "1" => ParamValue::Boolean(true),
                    "false" | "0" | "" => ParamValue::Boolean(false),
                    _ => return Err(fmt(tl!("${name} is true or false."), &[("name", row.name.trim())])),
                },
            };
            out.push(Variable { name: row.name.trim().to_owned(), value });
        }
        Ok(out)
    }

    /// Add rows for the external variables the query declares that the table does not have.
    fn read_variables(&mut self, query: &str) {
        for name in declared_variables(query) {
            if self.vars.len() < MAX_VARIABLES && !self.vars.iter().any(|r| r.name == name) {
                self.vars.push(VarRow { name, ..VarRow::default() });
            }
        }
    }
}

impl XmlCraftApp {
    /// Run XQuery…: show the Query panel on its XQuery tab. On an XQuery document, its text is the query and it is run at once.
    pub(crate) fn open_xquery(&mut self) {
        if !self.show_tool(Panel::Query) {
            return;
        }
        self.query.tab = crate::tools::query::Tab::XQuery;
        let shown_is_query = self.session.active_document().is_some_and(|d| d.language().is_xquery());
        if shown_is_query {
            self.query.xquery.run_now = true;
        } else {
            self.query.xquery.focus = true;
        }
    }

    /// Ask the worker to run the query: the shown XQuery document, or the tab's query on the shown document.
    pub(crate) fn run_xquery(&mut self) {
        let tab = &mut self.query.xquery;
        let fail = |tab: &mut XQueryTab, text: String| {
            tab.answer = Some(Answer { context: None, query: String::new(), outcome: Err(Failure { text, span: None }), method: tab.method })
        };
        let variables = match tab.variables() {
            Ok(v) => v,
            Err(why) => return fail(tab, why),
        };
        let doc = self.session.active_document();
        let (query, context, context_id) = match doc {
            Some(d) if d.language().is_xquery() => (d.editor().text(), None, None),
            Some(d) if d.language().is_xml() => (tab.query.clone(), Some(d.editor().text()), Some((d.id(), d.editor().version()))),
            _ => (tab.query.clone(), None, None),
        };
        if query.len() > MAX_BYTES || context.as_ref().is_some_and(|c| c.len() > MAX_BYTES) {
            let limit = (MAX_BYTES / (1024 * 1024)).to_string();
            return fail(tab, fmt(tl!("The query or the document is larger than {limit} MB."), &[("limit", &limit)]));
        }
        let mut namespaces = Vec::new();
        if let Some(text) = &context {
            namespaces = xmlcraft_engine::xpath::root_namespaces(text);
        }
        self.generate_serial += 1;
        let serial = self.generate_serial;
        let options = Options { namespaces, variables, serialize: Some(tab.method.serialization()), ..Options::default() };
        tab.answer =
            Some(Answer { context: context_id, query: query.clone(), outcome: Err(Failure { text: String::new(), span: None }), method: tab.method });
        tab.waiting = Some(serial);
        self.xquery.submit(XQueryJob { serial, query, context, options });
    }

    /// Collect what the worker finished. Called every frame.
    pub(crate) fn pump_xquery(&mut self) {
        for done in self.xquery.poll() {
            if self.query.xquery.waiting != Some(done.serial) {
                continue;
            }
            self.query.xquery.waiting = None;
            let Some(answer) = self.query.xquery.answer.as_mut() else { continue };
            answer.outcome = match done.result {
                Ok(o) => Ok((o.result, o.serialized)),
                Err(e) => Err(Failure { text: describe(&e), span: e.span.clone() }),
            };
            self.workbench.set_attention(Panel::Query);
        }
    }

    /// Open the last result, serialized, as a new document.
    pub(crate) fn open_xquery_result(&mut self) {
        let Some(answer) = &self.query.xquery.answer else { return };
        let Ok((_, Some(text))) = &answer.outcome else { return };
        let name = format!("query-result.{}", answer.method.extension());
        let text = text.clone();
        self.open_generated(&name, &text);
        self.status = fmt(tl!("The result is in {name}"), &[("name", &name)]);
    }
}

/// The tab: the query (unless the shown document is the query), its variables, how the result is written, and the result.
pub(crate) fn show(ui: &mut egui::Ui, app: &mut XmlCraftApp) -> Asked {
    let mut asked = Asked { run: std::mem::take(&mut app.query.xquery.run_now), ..Asked::default() };
    let shown = app.session.active_document().map(|d| (d.id(), d.editor().version(), d.language(), d.name().to_owned()));
    let shown_query = shown.as_ref().is_some_and(|s| s.2.is_xquery());
    let query_text = if shown_query { app.session.active_document().map(|d| d.editor().text()).unwrap_or_default() } else { String::new() };
    let tab = &mut app.query.xquery;
    match &shown {
        Some((_, _, language, name)) if language.is_xquery() => {
            kit::help(ui, &fmt(tl!("Runs {name}, with no context item. Ctrl/⌘+Enter in the editor's command bar: Run."), &[("name", name)]));
        }
        Some((_, _, language, name)) if language.is_xml() => {
            crate::tools::bound_line(ui, name);
            kit::help(ui, tl!("The document is the context item. Ctrl/⌘+Enter runs."));
        }
        _ => kit::help(ui, tl!("No XML document is shown: the query runs with no context item. Ctrl/⌘+Enter runs.")),
    }
    ui.add_space(kit::GAP);
    if !shown_query {
        let marks = Marks {
            selected: tab.answer.as_ref().filter(|a| a.query == tab.query).and_then(|a| a.outcome.as_ref().err()).and_then(|f| f.span.clone()),
            ..Marks::default()
        };
        let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| {
            let job = xpath_lab::marked(ui, text.as_str(), &marks, wrap);
            ui.fonts_mut(|f| f.layout_job(job))
        };
        let output = egui::TextEdit::multiline(&mut tab.query)
            .desired_rows(8)
            .desired_width(f32::INFINITY)
            .code_editor()
            .hint_text("for $i in //item\norder by $i/@n\nreturn <row>{ $i/name/text() }</row>")
            .layouter(&mut layouter)
            .show(ui);
        let response = output.response.clone();
        widgets::name_for_assistive_technology(ui, &response, tl!("The XQuery query"));
        if std::mem::take(&mut tab.focus) {
            response.request_focus();
        }
        if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command) {
            asked.run = true;
        }
    }
    ui.add_space(kit::GAP);
    kit::section(ui, tl!("External variables"), |ui| {
        let mut remove = None;
        for (i, row) in tab.vars.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let name = ui.add(egui::TextEdit::singleline(&mut row.name).desired_width(90.0).hint_text(tl!("name")));
                widgets::name_for_assistive_technology(ui, &name, tl!("The variable's name"));
                egui::ComboBox::from_id_salt(("xmlcraft-xquery-var-kind", i)).selected_text(row.kind.label()).width(70.0).show_ui(ui, |ui| {
                    for k in [VarKind::Text, VarKind::Number, VarKind::Boolean] {
                        ui.selectable_value(&mut row.kind, k, k.label());
                    }
                });
                let value = ui.add(egui::TextEdit::singleline(&mut row.value).desired_width(f32::INFINITY));
                widgets::name_for_assistive_technology(ui, &value, tl!("The variable's value"));
                if ui.small_button("×").on_hover_text(tl!("Remove")).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            tab.vars.remove(i);
        }
        ui.horizontal(|ui| {
            if ui.small_button(tl!("Add variable")).clicked() && tab.vars.len() < MAX_VARIABLES {
                tab.vars.push(VarRow::default());
            }
            if ui.small_button(tl!("Read from the query")).clicked() {
                let q = if shown_query { query_text.clone() } else { tab.query.clone() };
                tab.read_variables(&q);
            }
        });
    });
    ui.add_space(kit::GAP);
    ui.horizontal(|ui| {
        ui.label(tl!("Result as"));
        let labels = [(Method::Xml, "XML"), (Method::Json, "JSON"), (Method::Text, tl!("text")), (Method::Adaptive, tl!("adaptive"))];
        let current = labels.iter().find(|(m, _)| *m == tab.method).map_or("XML", |(_, l)| *l);
        egui::ComboBox::from_id_salt("xmlcraft-xquery-method").selected_text(current).width(90.0).show_ui(ui, |ui| {
            for (m, l) in labels {
                ui.selectable_value(&mut tab.method, m, l);
            }
        });
    });
    ui.horizontal(|ui| {
        let can_open = tab.answer.as_ref().is_some_and(|a| matches!(&a.outcome, Ok((_, Some(_)))));
        let r = ui.add_enabled(can_open, egui::Button::new(tl!("Open result as a document")));
        widgets::name_for_assistive_technology(ui, &r, tl!("Open result as a document"));
        if r.clicked() {
            asked.open_result = true;
        }
    });
    ui.add_space(kit::GAP);
    if tab.waiting.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Running…"));
        });
        return asked;
    }
    let Some(answer) = &tab.answer else {
        kit::help(ui, tl!("The items of the result are listed here; a click on a node of the document selects it in the text."));
        return asked;
    };
    let done = match &answer.outcome {
        Err(f) if f.text.is_empty() => return asked,
        Err(f) => {
            let place = f.span.as_ref().map(|s| {
                let (l, c) = line_column(&answer.query, s.start);
                fmt(tl!(" (line {line}, column {column} of the query)"), &[("line", &l.to_string()), ("column", &c.to_string())])
            });
            kit::banner(ui, Tone::Bad, &format!("{}{}", f.text, place.unwrap_or_default()));
            return asked;
        }
        Ok((done, _)) => done,
    };
    let current = answer.context.is_some() && answer.context == shown.as_ref().map(|s| (s.0, s.1));
    let count = done.count.to_string();
    kit::help(
        ui,
        &if done.count == 0 {
            tl!("The result is empty.").to_owned()
        } else if done.count > done.items.len() {
            fmt(tl!("{count} items; the first {shown} are listed."), &[("count", &count), ("shown", &done.items.len().to_string())])
        } else {
            fmt(tl!("{count} items."), &[("count", &count)])
        },
    );
    let t = crate::theme::Tokens::get(ui.ctx());
    egui::ScrollArea::vertical().id_salt("xmlcraft-xquery-items").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for item in done.items.iter().take(LISTED) {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::click());
            let clickable = item.span.is_some() && current;
            let text = line(item);
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, clickable, text.clone()));
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
            if clickable && crate::theme::hand(response).clicked() {
                asked.jump = item.span.clone();
            }
        }
    });
    asked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_are_read_from_the_query_and_checked() {
        let mut tab = XQueryTab::default();
        tab.read_variables("declare variable $n external; declare variable $p:x external; declare variable $y := 1; $n");
        // (`p` is not declared, so the query does not read: no rows.)
        assert!(tab.vars.is_empty());
        tab.read_variables("declare variable $n external; declare variable $y := 1; declare variable $s external := 'd'; $n");
        let names: Vec<&str> = tab.vars.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["n", "s"]);
        tab.vars[0].kind = VarKind::Number;
        tab.vars[0].value = "x".into();
        assert!(tab.variables().is_err());
        tab.vars[0].value = "3".into();
        assert_eq!(tab.variables().map(|v| v.len()), Ok(2));
    }

    #[test]
    fn a_place_is_a_line_and_a_column() {
        assert_eq!(line_column("ab\ncd", 4), (2, 2));
        assert_eq!(line_column("é", 2), (1, 2));
    }
}
