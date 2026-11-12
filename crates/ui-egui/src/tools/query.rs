//! Query, the tool window of XPath and XSLT. Both work on the document being shown.
//!
//! **XPath**: an expression evaluated with the document as the context (`xmlcraft_query`, on a worker thread), the items of the
//! result listed and filling the panel, and a click on a node selecting it in the text while the panel stays open.
//!
//! **XSLT**: a stylesheet applied to a document (`xmlcraft_query::transform`, on a worker thread), the result opened as a new
//! document. The roles follow the document: when it is a stylesheet it is the stylesheet and the source is chosen (from the open XML
//! documents or a file); otherwise it is the source and the stylesheet is chosen (the one it names, another open document or a file).
//! The documents themselves are never changed.

use std::ops::Range;
use std::path::PathBuf;

use xmlcraft_engine::xpath::{XPathJob, root_namespaces};
use xmlcraft_engine::xslt::{XsltJob, stylesheet_of};
use xmlcraft_query::{Evaluated, Options, Phase, QueryError, ResultItem};

use crate::bar_actions::BarKind;
use crate::i18n::{fmt, tl};
use crate::kit::{self, Tone};
use crate::tools::{self, Bound};
use crate::{Panel, XmlCraftApp, widgets};

/// The most bytes of a document or stylesheet the tool works on.
const MAX_BYTES: usize = 16 * 1024 * 1024;
/// The most characters of an item shown in the list (the whole text is in the tooltip up to this many more).
const SHOWN_CHARS: usize = 110;
/// The most items the list draws.
const LISTED: usize = 500;
/// The most steps of a path listed.
const STEPS_LISTED: usize = 24;
/// The height of a row of the list.
const ROW: f32 = 22.0;

/// Which part of the tool is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Tab {
    #[default]
    XPath,
    Xslt,
}

/// What an evaluation came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Items(Evaluated),
    Failed(String),
}

/// What an evaluation answered, and what it was asked on: the items' spans are only good for that version of that document.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Answer {
    doc: u64,
    version: u64,
    outcome: Outcome,
    trace: Option<xmlcraft_query::Trace>,
}

/// The other document of a transformation: the source when the shown document is a stylesheet, else the stylesheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Other {
    /// The stylesheet the shown document names, read from the open document with that path or from the file.
    Named(String),
    /// An open document (by id).
    Open(u64),
    /// A file.
    File(PathBuf),
}

/// What the stylesheet a document names was, for the version of the text it was read from.
#[derive(Clone, Debug, PartialEq, Eq)]
struct NamedSheet {
    doc: u64,
    version: u64,
    href: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Xslt {
    other: Option<Other>,
    named: Option<NamedSheet>,
    waiting: Option<u64>,
    /// What the result is named after, for the request being waited for.
    result_for: String,
    error: Option<String>,
}

/// The form of the Query tool window and what it found last.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct QueryTool {
    tab: Tab,
    expression: String,
    /// The request being waited for, and the document and version it was asked on.
    waiting: Option<(u64, u64, u64)>,
    answer: Option<Answer>,
    /// Also show each step of a path with the items it leaves and its time.
    trace_steps: bool,
    /// Give the keyboard to the expression field at the next frame.
    focus: bool,
    xslt: Xslt,
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

/// The extension a result is saved under, by what it looks like.
fn result_extension(text: &str) -> &'static str {
    let t = text.trim_start();
    if t.starts_with("<!DOCTYPE html") || t.starts_with("<!doctype html") || t.starts_with("<html") {
        "html"
    } else if t.starts_with('<') {
        "xml"
    } else if t.starts_with(['{', '[']) {
        "json"
    } else {
        "txt"
    }
}

/// The name of the result: that of the source with `-result` and the extension the result needs.
fn result_name(name: &str, extension: &str) -> String {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    format!("{stem}-result.{extension}")
}

/// What a frame of the tool asked for.
#[derive(Default)]
struct Asked {
    run: bool,
    jump: Option<Range<usize>>,
    pick_file: bool,
}

impl XmlCraftApp {
    /// Evaluate XPath…: show the Query panel on its XPath tab and give the keyboard to the expression.
    pub(crate) fn open_xpath(&mut self) {
        if !self.show_tool(Panel::Query) {
            return;
        }
        self.query.tab = Tab::XPath;
        self.query.focus = true;
    }

    /// Transform with XSLT…: show the Query panel on its XSLT tab.
    pub(crate) fn open_transform(&mut self) {
        if !self.show_tool(Panel::Query) {
            return;
        }
        self.query.tab = Tab::Xslt;
    }

    /// A file chosen with the XSLT tab's "File…": the other document of the transformation.
    pub(crate) fn picked_query_file(&mut self, path: &std::path::Path) {
        self.query.xslt.other = Some(Other::File(path.to_path_buf()));
        self.query.xslt.error = None;
        self.query.tab = Tab::Xslt;
        self.show_tool(Panel::Query);
    }

    /// Ask the worker to evaluate the expression on the document being shown.
    fn run_xpath(&mut self) {
        let Some(doc) = self.session.active_document() else { return };
        let (id, version) = (doc.id(), doc.editor().version());
        let failed = |outcome: Outcome| Some(Answer { doc: id, version, outcome, trace: None });
        if !doc.language().is_xml() {
            self.query.answer = failed(Outcome::Failed(tl!("XPath is evaluated on XML documents.").to_owned()));
            return;
        }
        let text = doc.editor().text();
        if text.len() > MAX_BYTES {
            let limit = (MAX_BYTES / (1024 * 1024)).to_string();
            self.query.answer = failed(Outcome::Failed(fmt(tl!("The document is larger than {limit} MB."), &[("limit", &limit)])));
            return;
        }
        self.generate_serial += 1;
        let serial = self.generate_serial;
        let options = Options { namespaces: root_namespaces(&text), ..Options::default() };
        self.xpath.submit(XPathJob { serial, text, expression: self.query.expression.clone(), options, trace: self.query.trace_steps });
        self.query.waiting = Some((serial, id, version));
    }

    /// Collect what the XPath worker finished, whatever else is open. Called every frame.
    pub(crate) fn pump_xpath(&mut self) {
        for done in self.xpath.poll() {
            let Some((serial, doc, version)) = self.query.waiting else { continue };
            if serial != done.serial {
                continue; // an older request
            }
            self.query.waiting = None;
            let outcome = match done.result {
                Ok(evaluated) => Outcome::Items(evaluated),
                Err(e) => Outcome::Failed(describe(&e)),
            };
            self.query.answer = Some(Answer { doc, version, outcome, trace: done.trace });
            self.workbench.set_attention(Panel::Query);
        }
    }

    /// The text and name of the other document of a transformation, with the shown document (whose folder the named stylesheet is
    /// relative to).
    fn other_text(&self, other: &Other, shown: u64) -> Result<(String, String), String> {
        match other {
            Other::Open(id) => self
                .session
                .position_of(*id)
                .and_then(|i| self.session.document(i))
                .map(|d| (d.name().to_owned(), d.editor().text()))
                .ok_or_else(|| tl!("That document is gone.").to_owned()),
            Other::Named(href) => {
                let base =
                    self.session.position_of(shown).and_then(|i| self.session.document(i)).and_then(|d| d.path().map(std::path::Path::to_path_buf));
                let wanted = match base.as_deref().and_then(std::path::Path::parent) {
                    Some(dir) => dir.join(href),
                    None => PathBuf::from(href),
                };
                // An open document with that path first (what is being edited), then the file.
                if let Some(doc) = self.session.documents().iter().find(|d| d.path().is_some_and(|p| p == wanted)) {
                    return Ok((doc.name().to_owned(), doc.editor().text()));
                }
                read_decoded(&wanted)
            }
            Other::File(path) => read_decoded(path),
        }
    }

    /// Ask the worker to apply the stylesheet. `sheet_shown`: the document being shown is the stylesheet.
    fn run_transform(&mut self, sheet_shown: bool) {
        let Some(doc) = self.session.active_document() else { return };
        let (shown, shown_name, shown_text) = (doc.id(), doc.name().to_owned(), doc.editor().text());
        let Some(other) = self.query.xslt.other.clone() else {
            self.query.xslt.error = Some(if sheet_shown { tl!("Choose the document to transform.") } else { tl!("Choose a stylesheet.") }.to_owned());
            return;
        };
        let (other_name, other_text) = match self.other_text(&other, shown) {
            Ok(found) => found,
            Err(why) => {
                self.query.xslt.error = Some(why);
                return;
            }
        };
        let (xml, xslt, source_name) = if sheet_shown { (other_text, shown_text, other_name) } else { (shown_text, other_text, shown_name) };
        if xml.len() > MAX_BYTES || xslt.len() > MAX_BYTES {
            let limit = (MAX_BYTES / (1024 * 1024)).to_string();
            self.query.xslt.error = Some(fmt(tl!("A document is larger than {limit} MB."), &[("limit", &limit)]));
            return;
        }
        self.generate_serial += 1;
        let serial = self.generate_serial;
        self.xslt.submit(XsltJob { serial, xml, xslt });
        self.query.xslt.waiting = Some(serial);
        self.query.xslt.result_for = source_name;
        self.query.xslt.error = None;
    }

    /// Collect what the XSLT worker finished, whatever else is open: a result opens as a new document, a failure is said in the
    /// panel. Called every frame.
    pub(crate) fn pump_transform(&mut self) {
        for done in self.xslt.poll() {
            if self.query.xslt.waiting != Some(done.serial) {
                continue;
            }
            self.query.xslt.waiting = None;
            match done.result {
                Ok(made) => {
                    let name = result_name(&self.query.xslt.result_for, result_extension(&made.text));
                    self.open_generated(&name, &made.text);
                    self.status = fmt(tl!("Transformed into {name}"), &[("name", &name)]);
                }
                Err(e) => {
                    self.query.xslt.error = Some(e.to_string());
                    self.workbench.set_attention(Panel::Query);
                }
            }
        }
    }
}

/// A file read and decoded.
fn read_decoded(path: &std::path::Path) -> Result<(String, String), String> {
    let loaded = xmlcraft_platform::files::read_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let decoded = xmlcraft_text::encoding::decode(&loaded.bytes, &xmlcraft_text::encoding::sniff(&loaded.bytes));
    let name = path.file_name().map_or_else(|| path.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
    Ok((name, decoded.text))
}

/// What can be chosen as the other document: the label and where it comes from. Recomputed every frame from the session.
fn choices(app: &mut XmlCraftApp, bound: &Bound, sheet_shown: bool) -> Vec<(String, Other)> {
    let mut found: Vec<(String, Other)> = Vec::new();
    if !sheet_shown {
        // The stylesheet the document names: looked for again only when its text changed (it is a parse).
        let version = app.session.active_document().map_or(0, |d| d.editor().version());
        let current = app.query.xslt.named.as_ref().is_some_and(|n| n.doc == bound.id && n.version == version);
        if !current {
            let href = app.session.active_document().and_then(|d| stylesheet_of(&d.editor().text()));
            app.query.xslt.named = Some(NamedSheet { doc: bound.id, version, href });
        }
        if let Some(href) = app.query.xslt.named.as_ref().and_then(|n| n.href.clone()) {
            found.push((fmt(tl!("The one the document names ({href})"), &[("href", &href)]), Other::Named(href)));
        }
    }
    for other in app.session.documents().iter().filter(|d| d.id() != bound.id && d.language().is_xml()) {
        found.push((other.name().to_owned(), Other::Open(other.id())));
    }
    if let Some(Other::File(path)) = &app.query.xslt.other {
        let name = path.file_name().map_or_else(|| path.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
        found.push((fmt(tl!("{name} (file)"), &[("name", &name)]), Other::File(path.clone())));
    }
    found
}

/// The body of the Query panel.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    let bound = tools::bound(app);
    let sheet_shown = app.active_bar_kind(ui.ctx()) == Some(BarKind::Stylesheet);
    let usable = bound.as_ref().is_some_and(|b| b.xml);
    let choices = match &bound {
        Some(b) if usable && app.query.tab == Tab::Xslt => choices(app, b, sheet_shown),
        _ => Vec::new(),
    };
    // A choice that is gone (a document closed) gives way to the first one, and a missing one to the first.
    if app.query.tab == Tab::Xslt && !choices.iter().any(|(_, o)| Some(o) == app.query.xslt.other.as_ref()) {
        app.query.xslt.other = choices.first().map(|(_, o)| o.clone());
    }
    let (primary, icon, can, why_not) = match app.query.tab {
        Tab::XPath => (
            tl!("Evaluate"),
            "search",
            usable && app.query.waiting.is_none() && !app.query.expression.trim().is_empty(),
            tl!("Type an expression first."),
        ),
        Tab::Xslt => (
            tl!("Transform"),
            "wand-sparkles",
            usable && app.query.xslt.waiting.is_none() && app.query.xslt.other.is_some(),
            if sheet_shown { tl!("Choose the document to transform.") } else { tl!("Choose a stylesheet.") },
        ),
    };
    let mut asked = Asked { run: kit::tool_footer(ui, primary, Some(icon), can, why_not), ..Asked::default() };
    let version = app.session.active_document().map_or(0, |d| d.editor().version());
    kit::tool_body(ui, false, |ui| {
        kit::segmented(ui, &mut app.query.tab, &[(Tab::XPath, tl!("XPath")), (Tab::Xslt, tl!("XSLT"))]);
        ui.add_space(kit::GAP);
        let Some(bound) = &bound else {
            tools::needs(ui, tl!("Query needs an XML document"), tl!("Open one to evaluate XPath or to transform it with XSLT."));
            return;
        };
        tools::bound_line(ui, &bound.name);
        if !bound.xml {
            tools::needs(ui, tl!("Query needs an XML document"), tl!("XPath and XSLT work on XML; this document is not XML."));
            return;
        }
        ui.add_space(kit::GAP);
        match app.query.tab {
            Tab::XPath => {
                let (submit, jump) = tools::columns(ui, &mut app.query, xpath_form, |ui, tool| xpath_results(ui, tool, bound.id, version));
                asked.run |= submit && can;
                asked.jump = jump;
            }
            Tab::Xslt => {
                let pick = xslt_form(ui, &mut app.query.xslt, &choices, sheet_shown);
                asked.pick_file = pick;
            }
        }
    });
    if asked.run && can {
        match app.query.tab {
            Tab::XPath => app.run_xpath(),
            Tab::Xslt => app.run_transform(sheet_shown),
        }
    }
    if let Some(range) = asked.jump {
        app.jump_to(range);
    }
    if asked.pick_file {
        app.pick_query_file(ui.ctx());
    }
}

/// The expression and its options. True when Cmd/Ctrl+Enter was pressed in the expression.
fn xpath_form(ui: &mut egui::Ui, tool: &mut QueryTool) -> bool {
    let mut submit = false;
    let response = ui.add(
        egui::TextEdit::multiline(&mut tool.expression)
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .code_editor()
            .hint_text(tl!("//item[@id = '7']/name")),
    );
    widgets::name_for_assistive_technology(ui, &response, tl!("The XPath expression"));
    if std::mem::take(&mut tool.focus) {
        response.request_focus();
    }
    if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command) {
        submit = true;
    }
    kit::help(ui, tl!("Ctrl/⌘+Enter evaluates. Prefixes declared on the root element can be used; elements in a default namespace are *:name."));
    ui.add(widgets::checkbox(&mut tool.trace_steps, tl!("Show the steps: the items each step of a path leaves, and its time")));
    submit
}

/// What the last evaluation found, filling the rest of the panel. Returns the span of the node clicked.
fn xpath_results(ui: &mut egui::Ui, tool: &mut QueryTool, doc: u64, version: u64) -> Option<Range<usize>> {
    ui.add_space(kit::GAP);
    if tool.waiting.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Evaluating…"));
        });
    }
    // The answer is for the document and the text it was asked on: another document's is not shown here, and a changed text's spans
    // no longer fit (the items stay, the clicks do not).
    let Some(answer) = tool.answer.as_ref().filter(|a| a.doc == doc) else {
        if tool.waiting.is_none() {
            kit::help(ui, tl!("Type an expression and press Cmd/Ctrl+Enter. The items are listed here; a click on a node selects it in the text."));
        }
        return None;
    };
    let current = answer.version == version;
    let done = match &answer.outcome {
        Outcome::Failed(message) => {
            kit::banner(ui, Tone::Bad, message);
            return None;
        }
        Outcome::Items(done) => done,
    };
    let count = done.count.to_string();
    let mut summary = if done.count == 0 {
        tl!("The result is empty.").to_owned()
    } else if done.count > done.items.len() {
        fmt(tl!("{count} items; the first {shown} are listed."), &[("count", &count), ("shown", &done.items.len().to_string())])
    } else {
        fmt(tl!("{count} items. Click a node to select it in the text."), &[("count", &count)])
    };
    if !current {
        summary = fmt(tl!("{summary} The document changed since: evaluate again."), &[("summary", &summary)]);
    }
    kit::help(ui, &summary);
    if let Some(trace) = answer.trace.as_ref().filter(|_| tool.trace_steps) {
        steps(ui, trace);
    }
    let t = crate::theme::Tokens::get(ui.ctx());
    let mut jump = None;
    egui::ScrollArea::vertical().id_salt("xmlcraft-xpath-items").auto_shrink([false, false]).show(ui, |ui| {
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
            let response = if item.cut { response.on_hover_text(tl!("The value is cut.")) } else { response };
            if clickable && crate::theme::hand(response).clicked() {
                jump = item.span.clone();
            }
        }
    });
    jump
}

/// The steps of a path, one line each.
fn steps(ui: &mut egui::Ui, trace: &xmlcraft_query::Trace) {
    let t = crate::theme::Tokens::get(ui.ctx());
    if !trace.is_path {
        kit::help(ui, tl!("This is not a path, so it is one step."));
    }
    for step in trace.steps.iter().take(STEPS_LISTED) {
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
    if trace.steps.len() > STEPS_LISTED {
        kit::help(ui, &fmt(tl!("and {more} more steps"), &[("more", &(trace.steps.len() - STEPS_LISTED).to_string())]));
    }
    ui.add_space(kit::GAP);
}

/// The XSLT tab: which document is the stylesheet and which the source. True when "File…" was clicked.
fn xslt_form(ui: &mut egui::Ui, xslt: &mut Xslt, choices: &[(String, Other)], sheet_shown: bool) -> bool {
    let mut pick = false;
    let (role, what) = if sheet_shown {
        (tl!("This document is the stylesheet"), tl!("Transform this document"))
    } else {
        (tl!("This document is the source"), tl!("Stylesheet"))
    };
    kit::help(ui, role);
    ui.add_space(kit::GAP);
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(what).font(crate::theme::medium(11.5)).color(t.text_muted));
    let current = choices.iter().find(|(_, o)| Some(o) == xslt.other.as_ref()).map(|(label, _)| label.clone()).unwrap_or_default();
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 90.0).max(120.0);
        let combo = egui::ComboBox::from_id_salt("xmlcraft-xslt-other").selected_text(current).width(width).show_ui(ui, |ui| {
            for (label, other) in choices {
                if ui.selectable_label(xslt.other.as_ref() == Some(other), label).clicked() {
                    xslt.other = Some(other.clone());
                    xslt.error = None;
                }
            }
        });
        let label = if sheet_shown { tl!("The document to transform") } else { tl!("The stylesheet") };
        widgets::name_for_assistive_technology(ui, &combo.response, label);
        if ui.button(tl!("File…")).clicked() {
            pick = true;
        }
    });
    if choices.is_empty() {
        kit::help(
            ui,
            if sheet_shown {
                tl!("No other XML document is open. Open the document to transform, or choose a file.")
            } else {
                tl!("The document names no stylesheet, and no other document is open to use as one. Open a stylesheet, or choose a file.")
            },
        );
    }
    ui.add_space(4.0);
    kit::help(ui, tl!("The engine implements part of XSLT 3.0; what it does not implement is said, and nothing is changed."));
    if xslt.waiting.is_some() {
        ui.add_space(kit::GAP);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Transforming…"));
        });
    }
    if let Some(message) = &xslt.error {
        ui.add_space(kit::GAP);
        kit::banner(ui, Tone::Bad, message);
    }
    pick
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

    #[test]
    fn a_result_is_named_after_its_source_and_what_it_looks_like() {
        assert_eq!(result_name("order.xml", "html"), "order-result.html");
        assert_eq!(result_name("a.b.xml", "txt"), "a.b-result.txt");
        assert_eq!(result_name("Untitled-1", "xml"), "Untitled-1-result.xml");
        assert_eq!(result_extension("<!DOCTYPE html><html/>"), "html");
        assert_eq!(result_extension("  <r/>"), "xml");
        assert_eq!(result_extension("{\"a\": 1}"), "json");
        assert_eq!(result_extension("plain"), "txt");
    }
}
