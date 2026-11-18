//! XBRL, the tool window of XBRL 2.1 instances and Inline XBRL documents. It follows the document being shown and reads it on a thread
//! a moment after the text stood still, with the taxonomy its `schemaRef`s name, through the resolver F8 uses
//! (`xmlcraft_engine::xbrl::report`: the open documents, the catalogs, the files, the web unless the person works offline). Four tabs:
//!
//! - **Facts**: a table of every fact (concept, value, context and its period, unit, decimals, dimensions), filtered by a text, sorted by
//!   any column, optionally only the facts that have findings. A click selects the fact (for Inline XBRL its tag) in the text.
//! - **Contexts** and **Units**: their ids, periods, entities, dimensions and measures, and how many facts use each (unused ones faintly).
//! - **Findings**: what F8 reports for the document (the checks of the instance, of the taxonomy, the calculations and XBRL Dimensions),
//!   counted by code; a click selects where each is.
//!
//! Above them, a summary: the kind of document, how many facts, contexts and units, what was read of the taxonomy and its dimensional
//! model. Everything shown belongs to the document and the version of its text it was read from: after an edit it is marked out of
//! date, and its clicks select nothing until it is read again.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use xmlcraft_engine::xbrl::TaxonomySummary;
use xmlcraft_xbrl::{Kind, Level};

use crate::http_pane::Outcome;
use crate::i18n::{fmt, tl};
use crate::kit::{self, Tone};
use crate::theme::{self, Tokens};
use crate::tools;
use crate::{Panel, XmlCraftApp, widgets};

/// The most bytes of a document the tool reads.
const MAX_BYTES: usize = 32 * 1024 * 1024;
/// How long the text is left alone before it is read again, in seconds.
const DEBOUNCE: f64 = 0.5;
/// The height of a row.
const ROW: f32 = 24.0;
/// The most characters of a value shown in the table (the text keeps all of it).
const VALUE_CHARS: usize = 120;

/// A document and a version of its text.
pub(crate) type Key = (u64, u64);

/// Which part of the tool is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Tab {
    #[default]
    Facts,
    Contexts,
    Units,
    Findings,
}

/// A fact as the table shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FactRow {
    pub concept: String,
    pub value: String,
    pub context: String,
    pub unit: String,
    pub decimals: String,
    pub dimensions: String,
    pub span: Range<usize>,
    /// The findings at the fact.
    pub findings: usize,
    /// What the filter looks in, in lower case.
    haystack: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ContextRow {
    pub id: String,
    pub period: String,
    pub entity: String,
    pub dimensions: String,
    pub uses: usize,
    pub span: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnitRow {
    pub id: String,
    pub measure: String,
    pub uses: usize,
    pub span: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FindingRow {
    pub code: &'static str,
    pub error: bool,
    pub message: String,
    pub span: Range<usize>,
}

/// What the thread read of a document, ready to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Reading {
    pub kind: Kind,
    pub facts: Vec<FactRow>,
    pub contexts: Vec<ContextRow>,
    pub units: Vec<UnitRow>,
    pub findings: Vec<FindingRow>,
    /// How many findings of each code.
    pub codes: Vec<(&'static str, usize)>,
    pub taxonomy: Option<TaxonomySummary>,
}

/// `{namespace}local` as `local` (a name the instance did not resolve stays as written).
fn local(key: &str) -> &str {
    key.rsplit_once('}').map_or(key, |(_, l)| l)
}

/// At most [`VALUE_CHARS`] characters of `text`, on one line.
fn cut(text: &str) -> String {
    let flat: String = text.chars().take(VALUE_CHARS + 1).map(|c| if c.is_control() { ' ' } else { c }).collect();
    if flat.chars().count() > VALUE_CHARS {
        let mut out: String = flat.chars().take(VALUE_CHARS).collect();
        out.push('…');
        out
    } else {
        flat
    }
}

fn dimensions_text(dimensions: &[xmlcraft_xbrl::Dimension]) -> String {
    dimensions
        .iter()
        .map(
            |d| if d.typed { format!("{}={}", local(&d.dimension), cut(&d.member)) } else { format!("{}={}", local(&d.dimension), local(&d.member)) },
        )
        .collect::<Vec<_>>()
        .join(", ")
}

/// Turn a report into what the tables show.
pub(crate) fn reading(report: xmlcraft_engine::xbrl::Report) -> Reading {
    let instance = &report.instance;
    let mut at: BTreeMap<usize, usize> = BTreeMap::new();
    for f in &report.findings {
        if !f.span.is_empty() {
            *at.entry(f.span.start).or_default() += 1;
        }
    }
    let mut context_uses: BTreeMap<&str, usize> = BTreeMap::new();
    let mut unit_uses: BTreeMap<&str, usize> = BTreeMap::new();
    for f in &instance.facts {
        if let Some(c) = f.context_ref.as_deref() {
            *context_uses.entry(c).or_default() += 1;
        }
        if let Some(u) = f.unit_ref.as_deref() {
            *unit_uses.entry(u).or_default() += 1;
        }
    }
    let contexts: BTreeMap<&str, &xmlcraft_xbrl::Context> = instance.contexts.iter().map(|c| (c.id.as_str(), c)).collect();
    let units: BTreeMap<&str, &xmlcraft_xbrl::Unit> = instance.units.iter().map(|u| (u.id.as_str(), u)).collect();
    let facts = instance
        .facts
        .iter()
        .map(|f| {
            let context = f.context_ref.as_deref().map(|id| match contexts.get(id).and_then(|c| c.period.as_ref()) {
                Some(p) => format!("{id} · {}", p.label()),
                None => id.to_owned(),
            });
            let unit = f.unit_ref.as_deref().map(|id| units.get(id).map_or_else(|| id.to_owned(), |u| u.measure.clone()));
            let dimensions = f.context_ref.as_deref().and_then(|id| contexts.get(id)).map(|c| dimensions_text(&c.dimensions)).unwrap_or_default();
            let value = if f.nil { "nil".to_owned() } else { cut(&f.value) };
            let decimals = f.decimals.clone().or_else(|| f.precision.as_ref().map(|p| format!("p{p}"))).unwrap_or_default();
            let haystack = format!("{}\n{}\n{}\n{}", f.name, value, dimensions, f.concept).to_lowercase();
            FactRow {
                concept: f.name.clone(),
                value,
                context: context.unwrap_or_default(),
                unit: unit.unwrap_or_default(),
                decimals,
                dimensions,
                span: f.span.clone(),
                findings: at.get(&f.span.start).copied().unwrap_or(0),
                haystack,
            }
        })
        .collect();
    let context_rows = instance
        .contexts
        .iter()
        .map(|c| ContextRow {
            id: c.id.clone(),
            period: c.period.as_ref().map(xmlcraft_xbrl::Period::label).unwrap_or_default(),
            entity: c.entity.clone(),
            dimensions: dimensions_text(&c.dimensions),
            uses: context_uses.get(c.id.as_str()).copied().unwrap_or(0),
            span: c.span.clone(),
        })
        .collect();
    let unit_rows = instance
        .units
        .iter()
        .map(|u| UnitRow {
            id: u.id.clone(),
            measure: u.measure.clone(),
            uses: unit_uses.get(u.id.as_str()).copied().unwrap_or(0),
            span: u.span.clone(),
        })
        .collect();
    let mut codes: BTreeMap<&'static str, usize> = BTreeMap::new();
    for f in &report.findings {
        *codes.entry(f.code).or_default() += 1;
    }
    let findings = report
        .findings
        .iter()
        .map(|f| FindingRow { code: f.code, error: f.level == Level::Error, message: f.message.clone(), span: f.span.clone() })
        .collect();
    Reading {
        kind: instance.kind,
        facts,
        contexts: context_rows,
        units: unit_rows,
        findings,
        codes: codes.into_iter().collect(),
        taxonomy: report.taxonomy,
    }
}

/// A column of the facts table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Column {
    #[default]
    Document,
    Concept,
    Value,
    Context,
    Unit,
    Decimals,
    Dimensions,
}

/// The order of the facts: by a column (the document's order when none), up or down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Sort {
    pub column: Column,
    pub descending: bool,
}

/// Two values compared as numbers when both are, else as text.
fn compare_values(a: &str, b: &str) -> Ordering {
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) => x.total_cmp(&y),
        _ => a.cmp(b),
    }
}

/// The facts the table lists, in order: those `filter` admits (any case, in the concept, the value or the dimensions), only those with
/// findings when `only_findings`, sorted by `sort`.
pub(crate) fn listed(facts: &[FactRow], filter: &str, only_findings: bool, sort: Sort) -> Vec<usize> {
    let wanted = filter.trim().to_lowercase();
    let mut rows: Vec<usize> = facts
        .iter()
        .enumerate()
        .filter(|(_, f)| (wanted.is_empty() || f.haystack.contains(&wanted)) && (!only_findings || f.findings > 0))
        .map(|(n, _)| n)
        .collect();
    if sort.column != Column::Document {
        let key = |f: &FactRow| -> String {
            match sort.column {
                Column::Concept | Column::Document => f.concept.clone(),
                Column::Value => f.value.clone(),
                Column::Context => f.context.clone(),
                Column::Unit => f.unit.clone(),
                Column::Decimals => f.decimals.clone(),
                Column::Dimensions => f.dimensions.clone(),
            }
        };
        rows.sort_by(|a, b| {
            let (Some(x), Some(y)) = (facts.get(*a), facts.get(*b)) else { return Ordering::Equal };
            let order = match sort.column {
                Column::Value | Column::Decimals => compare_values(&key(x), &key(y)),
                _ => key(x).cmp(&key(y)),
            };
            // (The document's order among equals, whichever way.)
            let order = if sort.descending { order.reverse() } else { order };
            order.then(a.cmp(b))
        });
    }
    rows
}

/// What the table lists now, and what it was worked out for.
#[derive(Debug, Default)]
struct View {
    /// The reading (its serial), the filter, the toggle and the order.
    made_for: Option<(u64, String, bool, Sort)>,
    rows: Vec<usize>,
}

/// The state of the XBRL tool window. A hidden tool keeps it, and the thread's answer lands in it whatever is shown.
#[derive(Debug, Default)]
pub(crate) struct XbrlTool {
    tab: Tab,
    /// What the reading is for now; a change starts the wait.
    key: Option<Key>,
    /// When (egui time, seconds) the wait for a quiet moment ends.
    due: Option<f64>,
    /// The request being waited for, and what it was asked for.
    waiting: Option<(u64, Key)>,
    /// The last answer: what it was for, its serial, and the reading or why there is none.
    shown: Option<(Key, u64, Result<Arc<Reading>, String>)>,
    filter: String,
    only_findings: bool,
    sort: Sort,
    /// The text the findings are filtered by (a code, or words of the message).
    finding_filter: String,
    view: View,
}

impl XbrlTool {
    /// What the tool shows, for `ui.state`.
    pub(crate) fn state(&self) -> serde_json::Value {
        let tab = match self.tab {
            Tab::Facts => "facts",
            Tab::Contexts => "contexts",
            Tab::Units => "units",
            Tab::Findings => "findings",
        };
        let shown = self.shown.as_ref().map(|((doc, version), _, r)| match r {
            Ok(r) => serde_json::json!({
                "document": doc, "version": version, "facts": r.facts.len(), "contexts": r.contexts.len(), "units": r.units.len(),
                "findings": r.findings.len(), "listed": self.view.rows.len(),
            }),
            Err(why) => serde_json::json!({ "document": doc, "version": version, "error": why }),
        });
        serde_json::json!({ "tab": tab, "reading": self.waiting.is_some(), "shown": shown })
    }
}

impl XmlCraftApp {
    /// XBRL Facts…: show the tool window.
    pub(crate) fn open_xbrl(&mut self) {
        self.show_tool(Panel::Xbrl);
    }

    /// Read the document being shown on a thread, with its taxonomy.
    fn start_xbrl_reading(&mut self, key: Key) {
        let Some(doc) = self.session.active_document() else { return };
        let text = doc.editor().text();
        if text.len() > MAX_BYTES {
            let limit = (MAX_BYTES / (1024 * 1024)).to_string();
            let serial = self.next_http_serial();
            self.xbrl.shown = Some((key, serial, Err(fmt(tl!("The document is larger than {limit} MB."), &[("limit", &limit)]))));
            return;
        }
        let location = doc.path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let open = self.open_documents();
        let network = self.network.clone();
        let serial = self.next_http_serial();
        let started = self.spawn_http(move || {
            let result = xmlcraft_engine::guard(|| {
                let resolver = network.resolver(Arc::new(xmlcraft_engine::validation::OpenDocuments::new(open)));
                xmlcraft_engine::xbrl::report(&text, &location, &resolver).map(|r| Box::new(reading(r)))
            })
            .unwrap_or_else(|_| Err(tl!("Reading the document failed (a bug); nothing was changed.").to_owned()));
            Outcome::XbrlReading { serial, result }
        });
        match started {
            Ok(()) => self.xbrl.waiting = Some((serial, key)),
            Err(why) => self.xbrl.shown = Some((key, serial, Err(why))),
        }
    }

    /// The answer of a reading thread, whatever is shown. An answer for an older request is dropped.
    pub(crate) fn xbrl_read(&mut self, serial: u64, result: Result<Box<Reading>, String>) {
        let tool = &mut self.xbrl;
        let Some((_, key)) = tool.waiting.take_if(|(s, _)| *s == serial) else { return };
        tool.shown = Some((key, serial, result.map(|r| Arc::new(*r))));
        self.workbench.set_attention(Panel::Xbrl);
    }
}

/// What a frame of the tool asked for.
#[derive(Default)]
struct Asked {
    jump: Option<Range<usize>>,
}

/// The body of the XBRL panel.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    let bound = tools::bound(app);
    let xbrl = app.active_bar_kind(ui.ctx()) == Some(crate::bar_actions::BarKind::Xbrl);
    let key = app.session.active_document().map(|d| (d.id(), d.editor().version()));
    let (Some(bound), Some(key)) = (bound, key) else {
        kit::tool_body(ui, false, |ui| {
            tools::needs(ui, tl!("XBRL needs an instance"), tl!("Open an XBRL 2.1 instance or an Inline XBRL document."));
        });
        return;
    };
    if !xbrl {
        kit::tool_body(ui, false, |ui| {
            tools::bound_line(ui, &bound.name);
            tools::needs(
                ui,
                tl!("This document is no XBRL instance"),
                tl!("XBRL works on XBRL 2.1 instances (xbrli:xbrl) and Inline XBRL documents (XHTML with ix: tags)."),
            );
        });
        return;
    }
    // A new text starts the wait for a quiet moment; when it is over, the thread starts.
    let now = ui.input(|i| i.time);
    let tool = &mut app.xbrl;
    if tool.key != Some(key) {
        tool.key = Some(key);
        tool.due = Some(now + DEBOUNCE);
    }
    if let Some(due) = tool.due {
        if now >= due && tool.waiting.is_none() {
            tool.due = None;
            app.start_xbrl_reading(key);
        } else if now < due {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(due - now));
        }
    }
    let mut asked = Asked::default();
    let tool = &mut app.xbrl;
    kit::tool_body(ui, false, |ui| {
        tools::bound_line(ui, &bound.name);
        let updating = tool.waiting.is_some() || tool.due.is_some() || tool.shown.as_ref().is_none_or(|(k, _, _)| *k != key);
        // Another document's reading is not shown; one of an older text is, without its clicks.
        let shown = tool.shown.as_ref().filter(|(k, _, _)| k.0 == key.0).map(|(k, serial, r)| (*k, *serial, r.clone()));
        if updating {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tl!("Reading the instance and its taxonomy…"));
            });
        }
        let Some((at, serial, shown)) = shown else { return };
        let live = at == key;
        let reading = match shown {
            Err(message) => {
                kit::banner(ui, Tone::Bad, &message);
                return;
            }
            Ok(reading) => reading,
        };
        if !live {
            kit::help(ui, tl!("The text changed since: what is shown is updated in a moment, and clicks select nothing until then."));
        }
        summary(ui, &reading);
        ui.add_space(kit::GAP);
        kit::segmented(
            ui,
            &mut tool.tab,
            &[(Tab::Facts, tl!("Facts")), (Tab::Contexts, tl!("Contexts")), (Tab::Units, tl!("Units")), (Tab::Findings, tl!("Findings"))],
        );
        ui.add_space(kit::GAP);
        let clicked = match tool.tab {
            Tab::Facts => facts(ui, tool, serial, &reading, live),
            Tab::Contexts => contexts(ui, &reading, live),
            Tab::Units => units(ui, &reading, live),
            Tab::Findings => findings(ui, &mut tool.finding_filter, &reading, live),
        };
        asked.jump = clicked;
    });
    if let Some(range) = asked.jump {
        app.jump_to(range);
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    fmt(if n == 1 { one } else { many }, &[("count", &n.to_string())])
}

/// The summary: what the document is, and what was read of its taxonomy.
fn summary(ui: &mut egui::Ui, r: &Reading) {
    let kind = match r.kind {
        Kind::Instance => tl!("XBRL 2.1 instance"),
        Kind::Inline => tl!("Inline XBRL"),
    };
    let errors = r.findings.iter().filter(|f| f.error).count();
    let warnings = r.findings.len() - errors;
    ui.horizontal_wrapped(|ui| {
        let t = Tokens::get(ui.ctx());
        ui.label(egui::RichText::new(kind).font(theme::semibold(13.0)).color(t.text));
        let line = [
            plural(r.facts.len(), tl!("{count} fact"), tl!("{count} facts")),
            plural(r.contexts.len(), tl!("{count} context"), tl!("{count} contexts")),
            plural(r.units.len(), tl!("{count} unit"), tl!("{count} units")),
        ]
        .join(" · ");
        ui.label(egui::RichText::new(line).font(theme::regular(12.5)).color(t.text_muted));
        if errors > 0 {
            kit::chip(ui, Tone::Bad, None, &plural(errors, tl!("{count} error"), tl!("{count} errors")), false);
        }
        if warnings > 0 {
            kit::chip(ui, Tone::Warn, None, &plural(warnings, tl!("{count} warning"), tl!("{count} warnings")), false);
        }
        if errors == 0 && warnings == 0 {
            kit::chip(ui, Tone::Good, None, tl!("no findings"), false);
        }
    });
    let taxonomy = match &r.taxonomy {
        None => tl!("No taxonomy: the document has no schemaRef.").to_owned(),
        Some(t) => {
            let mut line = fmt(
                tl!("Taxonomy: {documents} read, {failed} not · {concepts} · {calculations}"),
                &[
                    ("documents", &plural(t.documents, tl!("{count} document"), tl!("{count} documents"))),
                    ("failed", &t.not_loaded.to_string()),
                    ("concepts", &plural(t.concepts, tl!("{count} concept"), tl!("{count} concepts"))),
                    ("calculations", &plural(t.calculations, tl!("{count} calculation"), tl!("{count} calculations"))),
                ],
            );
            if t.hypercubes + t.dimensions > 0 {
                line.push_str(" · ");
                let mut dimensions = plural(t.dimensions, tl!("{count} dimension"), tl!("{count} dimensions"));
                if t.typed_dimensions > 0 {
                    dimensions.push_str(&fmt(tl!(" ({typed} typed)"), &[("typed", &t.typed_dimensions.to_string())]));
                }
                line.push_str(&fmt(
                    tl!("dimensions: {hypercubes}, {dimensions}, {members}, {items}"),
                    &[
                        ("hypercubes", &plural(t.hypercubes, tl!("{count} hypercube"), tl!("{count} hypercubes"))),
                        ("dimensions", &dimensions),
                        ("members", &plural(t.members, tl!("{count} member"), tl!("{count} members"))),
                        ("items", &plural(t.primary_items, tl!("{count} primary item"), tl!("{count} primary items"))),
                    ],
                ));
            }
            line
        }
    };
    kit::help(ui, &taxonomy);
}

/// A column of a table: its title and its share of the width.
struct Col {
    title: &'static str,
    share: f32,
}

/// One row as a table draws it.
struct RowView<'a> {
    cells: Vec<&'a str>,
    /// The accessible name (and the tooltip).
    label: String,
    faint: bool,
    /// Drawn with the error colour in its first cell.
    flagged: bool,
}

/// A table filling the rest of the panel: a header (whose titles sort when `sorted` says by which and how) and `count` rows that `row`
/// describes, only those in view drawn. Returns the row clicked (when `live`) and the header clicked.
fn table<'a>(
    ui: &mut egui::Ui,
    salt: &str,
    cols: &[Col],
    sorted: Option<(usize, bool)>,
    count: usize,
    live: bool,
    row: impl Fn(usize) -> Option<RowView<'a>>,
) -> (Option<usize>, Option<usize>) {
    let t = Tokens::get(ui.ctx());
    let syntax = crate::editor_view::SyntaxTokens::for_kind(t.kind);
    let width = ui.available_width().max(120.0);
    let total: f32 = cols.iter().map(|c| c.share).sum::<f32>().max(0.01);
    let widths: Vec<f32> = cols.iter().map(|c| width * c.share / total).collect();
    let mut header = None;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, ROW), egui::Sense::hover());
    let mut x = rect.left();
    for (n, (col, w)) in cols.iter().zip(&widths).enumerate() {
        let cell = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(*w, ROW));
        x += w;
        let arrow = match sorted {
            Some((c, false)) if c == n => " ▲",
            Some((c, true)) if c == n => " ▼",
            _ => "",
        };
        let title = format!("{}{arrow}", crate::i18n::t(col.title));
        if sorted.is_some() {
            let response = ui.interact(cell, ui.scope_id().with((salt, "head", n)), egui::Sense::click());
            let name = fmt(tl!("Sort by {column}"), &[("column", crate::i18n::t(col.title))]);
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone()));
            if response.hovered() {
                ui.painter().rect_filled(cell, 3.0, t.hover);
            }
            if theme::hand(response).on_hover_text(name.clone()).clicked() {
                header = Some(n);
            }
        }
        ui.painter().with_clip_rect(cell.shrink2(egui::vec2(4.0, 0.0))).text(
            egui::pos2(cell.left() + 6.0, cell.center().y),
            egui::Align2::LEFT_CENTER,
            title,
            theme::medium(11.5),
            t.text_muted,
        );
    }
    ui.painter().hline(rect.x_range(), rect.bottom(), egui::Stroke::new(1.0, t.divider));
    let mut clicked = None;
    egui::ScrollArea::vertical().id_salt(salt).auto_shrink([false, false]).show_rows(ui, ROW, count, |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for n in range {
            let Some(view) = row(n) else { continue };
            let (rect, response) = ui.allocate_exact_size(egui::vec2(width, ROW), egui::Sense::click());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, live, view.label.clone()));
            if live && response.hovered() {
                ui.painter().rect_filled(rect, 4.0, t.hover);
            }
            let mut x = rect.left();
            for (i, (text, w)) in view.cells.iter().zip(&widths).enumerate() {
                let cell = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(*w, ROW));
                x += w;
                let color = if view.flagged && i == 0 {
                    syntax.error
                } else if view.faint || !live {
                    t.text_faint
                } else {
                    t.text
                };
                ui.painter().with_clip_rect(cell.shrink2(egui::vec2(4.0, 0.0))).text(
                    egui::pos2(cell.left() + 6.0, cell.center().y),
                    egui::Align2::LEFT_CENTER,
                    *text,
                    theme::regular(12.0),
                    color,
                );
            }
            let response = response.on_hover_text(view.label.clone());
            if live && theme::hand(response).clicked() {
                clicked = Some(n);
            }
        }
    });
    (clicked, header)
}

/// The Facts tab. Returns the range of the fact clicked.
fn facts(ui: &mut egui::Ui, tool: &mut XbrlTool, serial: u64, r: &Reading, live: bool) -> Option<Range<usize>> {
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 220.0).max(160.0);
        let response = ui.add(widgets::line(&mut tool.filter).hint_text(tl!("Filter by concept, value or dimension")).desired_width(width));
        widgets::name_for_assistive_technology(ui, &response, tl!("Filter the facts"));
        ui.add(widgets::checkbox(&mut tool.only_findings, tl!("Only facts with findings")));
    });
    let wanted = (serial, tool.filter.clone(), tool.only_findings, tool.sort);
    if tool.view.made_for.as_ref() != Some(&wanted) {
        tool.view.rows = listed(&r.facts, &tool.filter, tool.only_findings, tool.sort);
        tool.view.made_for = Some(wanted);
    }
    let rows = &tool.view.rows;
    let count = if rows.len() == r.facts.len() {
        plural(r.facts.len(), tl!("{count} fact. Click it to select it in the text."), tl!("{count} facts. Click one to select it in the text."))
    } else {
        fmt(tl!("{shown} of {count} facts."), &[("shown", &rows.len().to_string()), ("count", &r.facts.len().to_string())])
    };
    kit::help(ui, &count);
    const COLUMNS: [Column; 6] = [Column::Concept, Column::Value, Column::Context, Column::Unit, Column::Decimals, Column::Dimensions];
    let cols = [
        Col { title: "Concept", share: 0.24 },
        Col { title: "Value", share: 0.15 },
        Col { title: "Context", share: 0.21 },
        Col { title: "Unit", share: 0.08 },
        Col { title: "Decimals", share: 0.07 },
        Col { title: "Dimensions", share: 0.25 },
    ];
    let sorted = COLUMNS.iter().position(|c| *c == tool.sort.column).map(|n| (n, tool.sort.descending));
    let (clicked, header) = table(ui, "xmlcraft-xbrl-facts", &cols, Some(sorted.unwrap_or((usize::MAX, false))), rows.len(), live, |n| {
        let f = r.facts.get(*rows.get(n)?)?;
        let label = if f.dimensions.is_empty() {
            format!("{} = {} · {}", f.concept, f.value, f.context)
        } else {
            format!("{} = {} · {} · {}", f.concept, f.value, f.context, f.dimensions)
        };
        Some(RowView {
            cells: vec![f.concept.as_str(), f.value.as_str(), f.context.as_str(), f.unit.as_str(), f.decimals.as_str(), f.dimensions.as_str()],
            label,
            faint: false,
            flagged: f.findings > 0,
        })
    });
    // A click on a header sorts by it; again, the other way; a third time, back to the document's order.
    if let Some(column) = header.and_then(|n| COLUMNS.get(n).copied()) {
        tool.sort = match tool.sort {
            Sort { column: c, descending: false } if c == column => Sort { column, descending: true },
            Sort { column: c, descending: true } if c == column => Sort::default(),
            _ => Sort { column, descending: false },
        };
    }
    clicked.and_then(|n| rows.get(n)).and_then(|i| r.facts.get(*i)).map(|f| f.span.clone())
}

fn uses_text(uses: usize) -> String {
    if uses == 0 { tl!("unused").to_owned() } else { plural(uses, tl!("{count} fact"), tl!("{count} facts")) }
}

/// The Contexts tab.
fn contexts(ui: &mut egui::Ui, r: &Reading, live: bool) -> Option<Range<usize>> {
    let unused = r.contexts.iter().filter(|c| c.uses == 0).count();
    kit::help(
        ui,
        &fmt(
            tl!("{contexts}, {unused} unused. Click one to select it in the text."),
            &[("contexts", &plural(r.contexts.len(), tl!("{count} context"), tl!("{count} contexts"))), ("unused", &unused.to_string())],
        ),
    );
    let cols = [
        Col { title: "Context", share: 0.18 },
        Col { title: "Period", share: 0.22 },
        Col { title: "Entity", share: 0.16 },
        Col { title: "Dimensions", share: 0.32 },
        Col { title: "Used by", share: 0.12 },
    ];
    let uses: Vec<String> = r.contexts.iter().map(|c| uses_text(c.uses)).collect();
    let (clicked, _) = table(ui, "xmlcraft-xbrl-contexts", &cols, None, r.contexts.len(), live, |n| {
        let c = r.contexts.get(n)?;
        let used = uses.get(n).map_or("", String::as_str);
        Some(RowView {
            cells: vec![c.id.as_str(), c.period.as_str(), c.entity.as_str(), c.dimensions.as_str(), used],
            label: format!("{} · {} · {used}", c.id, c.period),
            faint: c.uses == 0,
            flagged: false,
        })
    });
    clicked.and_then(|n| r.contexts.get(n)).map(|c| c.span.clone())
}

/// The Units tab.
fn units(ui: &mut egui::Ui, r: &Reading, live: bool) -> Option<Range<usize>> {
    kit::help(
        ui,
        &plural(r.units.len(), tl!("{count} unit. Click it to select it in the text."), tl!("{count} units. Click one to select it in the text.")),
    );
    let cols = [Col { title: "Unit", share: 0.25 }, Col { title: "Measure", share: 0.5 }, Col { title: "Used by", share: 0.25 }];
    let uses: Vec<String> = r.units.iter().map(|u| uses_text(u.uses)).collect();
    let (clicked, _) = table(ui, "xmlcraft-xbrl-units", &cols, None, r.units.len(), live, |n| {
        let u = r.units.get(n)?;
        let used = uses.get(n).map_or("", String::as_str);
        Some(RowView {
            cells: vec![u.id.as_str(), u.measure.as_str(), used],
            label: format!("{} · {} · {used}", u.id, u.measure),
            faint: u.uses == 0,
            flagged: false,
        })
    });
    clicked.and_then(|n| r.units.get(n)).map(|u| u.span.clone())
}

/// A finding passes the filter when its code or its message has the text (any case).
fn admits(filter: &str, f: &FindingRow) -> bool {
    let wanted = filter.trim().to_lowercase();
    wanted.is_empty() || f.code.to_lowercase().contains(&wanted) || f.message.to_lowercase().contains(&wanted)
}

/// The Findings tab: the counts by code (a click filters by it), then the findings.
fn findings(ui: &mut egui::Ui, filter: &mut String, r: &Reading, live: bool) -> Option<Range<usize>> {
    if r.findings.is_empty() {
        kit::help(ui, tl!("No findings: the checks of the instance, its taxonomy, the calculations and the dimensions found nothing."));
        return None;
    }
    let response = ui.add(widgets::line(filter).hint_text(tl!("Filter by code or text")).desired_width(f32::INFINITY));
    widgets::name_for_assistive_technology(ui, &response, tl!("Filter the findings"));
    ui.horizontal_wrapped(|ui| {
        for (code, n) in &r.codes {
            let label = format!("{code} {n}");
            let selected = filter.trim() == *code;
            if kit::toggle_chip(ui, selected, &label).clicked() {
                *filter = if selected { String::new() } else { (*code).to_owned() };
            }
        }
    });
    let rows: Vec<&FindingRow> = r.findings.iter().filter(|f| admits(filter, f)).collect();
    let summary = if rows.len() == r.findings.len() {
        plural(r.findings.len(), tl!("{count} finding. Click it to select where it is."), tl!("{count} findings. Click one to select where it is."))
    } else {
        fmt(tl!("{shown} of {count} findings."), &[("shown", &rows.len().to_string()), ("count", &r.findings.len().to_string())])
    };
    kit::help(ui, &summary);
    let t = Tokens::get(ui.ctx());
    let syntax = crate::editor_view::SyntaxTokens::for_kind(t.kind);
    let mut jump = None;
    egui::ScrollArea::vertical().id_salt("xmlcraft-xbrl-findings").auto_shrink([false, false]).show_rows(ui, ROW + 2.0, rows.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for f in rows.get(range).unwrap_or_default() {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW + 2.0), egui::Sense::click());
            let clickable = live && !f.span.is_empty();
            let label = format!("{}: {}", f.code, f.message);
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, clickable, label.clone()));
            if clickable && response.hovered() {
                ui.painter().rect_filled(rect, 4.0, t.hover);
            }
            let color = if f.error { syntax.error } else { syntax.warning };
            let chip = ui.painter().layout_no_wrap(f.code.to_owned(), theme::semibold(11.0), color);
            let chip_rect = egui::Rect::from_min_size(rect.left_center() + egui::vec2(4.0, -9.0), egui::vec2(chip.size().x + 12.0, 18.0));
            ui.painter().rect_filled(chip_rect, 9.0, color.gamma_multiply(if t.dark() { 0.22 } else { 0.12 }));
            ui.painter().galley(chip_rect.center() - chip.size() / 2.0, chip, color);
            let text_left = chip_rect.right() + 8.0;
            let text_rect = egui::Rect::from_min_max(egui::pos2(text_left, rect.top()), rect.right_bottom());
            ui.painter().with_clip_rect(text_rect).text(
                egui::pos2(text_left, rect.center().y),
                egui::Align2::LEFT_CENTER,
                cut(&f.message),
                theme::regular(12.0),
                if live { t.text } else { t.text_muted },
            );
            let response = response.on_hover_text(f.message.clone());
            if clickable && theme::hand(response).clicked() {
                jump = Some(f.span.clone());
            }
        }
    });
    jump
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(concept: &str, value: &str, findings: usize) -> FactRow {
        FactRow {
            concept: concept.into(),
            value: value.into(),
            context: "c".into(),
            unit: String::new(),
            decimals: String::new(),
            dimensions: if concept == "eg:B" { "Region=North".into() } else { String::new() },
            span: 0..1,
            findings,
            haystack: format!("{concept}\n{value}\n{}", if concept == "eg:B" { "region=north" } else { "" }).to_lowercase(),
        }
    }

    #[test]
    fn the_facts_are_filtered_in_any_case_and_sorted_by_a_column_with_numbers_as_numbers() {
        let facts = vec![fact("eg:C", "10", 0), fact("eg:A", "9", 1), fact("eg:B", "100", 0)];
        assert_eq!(listed(&facts, "", false, Sort::default()), [0, 1, 2], "the document's order");
        assert_eq!(listed(&facts, "EG:a", false, Sort::default()), [1]);
        assert_eq!(listed(&facts, "north", false, Sort::default()), [2], "the dimensions are looked in");
        assert_eq!(listed(&facts, "", true, Sort::default()), [1], "only those with findings");
        assert_eq!(listed(&facts, "", false, Sort { column: Column::Concept, descending: false }), [1, 2, 0]);
        assert_eq!(listed(&facts, "", false, Sort { column: Column::Value, descending: false }), [1, 0, 2], "9 < 10 < 100");
        assert_eq!(listed(&facts, "", false, Sort { column: Column::Value, descending: true }), [2, 0, 1]);
        assert!(listed(&facts, "nothing", false, Sort::default()).is_empty());
    }

    #[test]
    fn a_value_is_one_line_of_bounded_length() {
        assert_eq!(cut("a\nb"), "a b");
        let long = cut(&"é".repeat(VALUE_CHARS + 5));
        assert!(long.ends_with('…') && long.chars().count() == VALUE_CHARS + 1);
        assert_eq!(local("{urn:x}Name"), "Name");
        assert_eq!(local("p:Name"), "p:Name");
    }
}
