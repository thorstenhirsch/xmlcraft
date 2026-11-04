//! The document header, above the text: where the caret is (a breadcrumb of the elements around it), which grammar the document follows,
//! whether it is valid, and whether schemas may be downloaded. What matters about a document is in one place, and each part is a control
//! for what to do about it.

use std::ops::Range;

use egui::{Align, Layout, Sense, Stroke, vec2};
use xmlcraft_engine::analysis::{Analysis, OutlineItem, Skipped};

use crate::i18n::{fmt, tl};
use crate::kit::{self, Tone};
use crate::theme::{self, Tokens};
use crate::{XmlCraftApp, icons};

const HEADER_HEIGHT: f32 = 40.0;
/// The most elements of the path shown; a deeper one is cut at its top with "…".
const MAX_CRUMBS: usize = 6;

/// The elements around the caret, outermost first: every element whose start and end tags enclose it.
pub(crate) fn element_path(outline: &[OutlineItem], caret: usize) -> Vec<&OutlineItem> {
    outline.iter().filter(|item| item.range.start <= caret && caret <= item.range.end).collect()
}

/// What kind of grammar a document follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GrammarKind {
    Dtd,
    Xsd,
}

/// The quoted strings of a DOCTYPE declaration, in order (the public identifier and the system identifier).
fn quoted(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(['"', '\'']) {
        let quote = rest[start..].chars().next().unwrap_or('"');
        let after = &rest[start + 1..];
        let Some(end) = after.find(quote) else { break };
        found.push(&after[..end]);
        rest = &after[end + 1..];
    }
    found
}

/// A short name for the DTD a DOCTYPE declaration names: the file of its system identifier, else its public identifier, else that it is
/// declared in the document.
pub(crate) fn dtd_label(doctype: &str) -> String {
    let (has_system, has_public) = external_id_keywords(doctype);
    let strings = quoted(doctype);
    let system = if has_system {
        strings.first().copied()
    } else if has_public {
        strings.get(1).copied()
    } else {
        None
    };
    let file = system.and_then(|s| s.rsplit('/').next()).filter(|f| !f.is_empty());
    match (file, strings.first()) {
        (Some(file), _) => file.to_owned(),
        (None, Some(public)) if has_public => (*public).to_owned(),
        _ => tl!("declared in the document").to_owned(),
    }
}

/// Whether the DOCTYPE's external identifier starts with SYSTEM or with PUBLIC: the keywords outside the quoted literals and before the
/// internal subset, so that a `SYSTEM` inside a public identifier or an entity declaration is not taken for one.
fn external_id_keywords(doctype: &str) -> (bool, bool) {
    let mut outside = String::new();
    let mut quote = None;
    for c in doctype.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                outside.push(' ');
            }
            None if c == '[' => break,
            None => outside.push(c),
        }
    }
    let words: Vec<&str> = outside.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    (words.contains(&"SYSTEM"), words.contains(&"PUBLIC"))
}

/// A short name for the schema a hint names: the file of its location, else its namespace.
pub(crate) fn schema_label(location: &str, namespace: Option<&str>) -> String {
    let file = location.rsplit(['/', '\\']).next().filter(|f| !f.is_empty());
    match (file, namespace) {
        (Some(file), _) => file.to_owned(),
        (None, Some(namespace)) => namespace.to_owned(),
        _ => tl!("a schema").to_owned(),
    }
}

/// What the grammar chip says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GrammarChip {
    pub kind: Option<GrammarKind>,
    pub label: String,
    pub tone: Tone,
    /// Shown on hover.
    pub tip: String,
}

fn grammar_chip(app: &XmlCraftApp) -> Option<GrammarChip> {
    let doc = app.session.active_document()?;
    let analysis = app.active_analysis()?;
    let helpers = app.views.get(&doc.id()).map(|v| &v.helpers);
    if let Some(range) = &analysis.doctype {
        let text = doc.editor().buffer().slice(range.clone()).unwrap_or_default();
        let label = dtd_label(&text);
        let loaded = helpers.is_some_and(|h| h.grammar.is_some());
        return Some(GrammarChip {
            kind: Some(GrammarKind::Dtd),
            tone: if loaded { Tone::Accent } else { Tone::Neutral },
            tip: if loaded {
                fmt(tl!("This document is checked against the DTD {name}. Click to assign another."), &[("name", &label)])
            } else {
                fmt(
                    tl!("The DTD {name} could not be loaded, so the document is not checked against it. Click to assign another."),
                    &[("name", &label)],
                )
            },
            label,
        });
    }
    if let Some(hint) = analysis.schema_hints.first() {
        let label = schema_label(&hint.location, hint.namespace.as_deref());
        let error = helpers.and_then(|h| h.xsd_error.clone());
        let loaded = helpers.is_some_and(|h| h.xsd.is_some());
        let (tone, tip) = match (&error, loaded) {
            (Some(why), _) => (Tone::Warn, fmt(tl!("The schema {name} could not be read: {why}"), &[("name", &label), ("why", why)])),
            (None, true) => {
                (Tone::Accent, fmt(tl!("This document is checked against the schema {name}. Click to assign another."), &[("name", &label)]))
            }
            (None, false) => (Tone::Neutral, fmt(tl!("Reading the schema {name}…"), &[("name", &label)])),
        };
        return Some(GrammarChip { kind: Some(GrammarKind::Xsd), label, tone, tip });
    }
    if let (Some(namespace), true) = (&analysis.root_namespace, helpers.is_some_and(|h| h.xsd.is_some())) {
        let label = schema_label("", Some(namespace));
        return Some(GrammarChip {
            kind: Some(GrammarKind::Xsd),
            tip: fmt(tl!("The schema that ships with the app for {name} helps with this document. Click to assign one."), &[("name", &label)]),
            label,
            tone: Tone::Neutral,
        });
    }
    Some(GrammarChip {
        kind: None,
        label: tl!("No schema").to_owned(),
        tone: Tone::Neutral,
        tip: tl!("This document follows no DTD or schema. Click to assign one.").to_owned(),
    })
}

/// What the validity pill says: how the document stands, from the well-formedness check and, when it has been asked for, the validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Standing {
    pub tone: Tone,
    pub icon: &'static str,
    pub text: String,
}

pub(crate) fn standing(analysis: Option<&Analysis>, validation: Option<(usize, bool)>) -> Standing {
    let plain = |tone, icon, text: &str| Standing { tone, icon, text: text.to_owned() };
    match analysis {
        None => plain(Tone::Neutral, "clock", tl!("Checking…")),
        Some(a) if a.skipped == Some(Skipped::Failed) => plain(Tone::Bad, "circle-x", tl!("The check failed")),
        Some(a) if a.skipped.is_some() => plain(Tone::Neutral, "info", tl!("Not checked")),
        Some(a) if a.error_count() > 0 => {
            Standing { tone: Tone::Bad, icon: "circle-x", text: fmt(tl!("{count} errors"), &[("count", &a.error_count().to_string())]) }
        }
        Some(_) => match validation {
            Some((errors, _)) if errors > 0 => {
                Standing { tone: Tone::Bad, icon: "circle-x", text: fmt(tl!("{count} validity errors"), &[("count", &errors.to_string())]) }
            }
            Some((_, true)) => plain(Tone::Good, "circle-check", tl!("Valid")),
            _ => plain(Tone::Good, "check", tl!("Well-formed")),
        },
    }
}

/// The header panel for the document being shown. Call after the side panels, so that it spans the width of the text only.
pub fn show(app: &mut XmlCraftApp, ui: &mut egui::Ui) {
    if !app.session.has_document() || app.session.active().is_none() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let (path, grammar, standing_now, offline) = {
        let caret = app.session.active_document().map_or(0, |d| d.editor().primary().head());
        let path: Vec<(String, Option<String>, Range<usize>)> = app
            .active_analysis()
            .map(|a| element_path(&a.outline, caret).into_iter().map(|i| (i.name.clone(), i.label.clone(), i.name_range.clone())).collect())
            .unwrap_or_default();
        let validation = app.active_validation().map(|v| (v.error_count(), v.is_valid()));
        (path, grammar_chip(app), standing(app.active_analysis(), validation), app.network_is_offline())
    };
    let mut jump = None;
    let mut command: Option<&'static str> = None;
    egui::Panel::top("document_header")
        .exact_size(HEADER_HEIGHT)
        .frame(egui::Frame::NONE.fill(t.panel).inner_margin(egui::Margin::symmetric(14, 0)).stroke(Stroke::new(1.0, t.divider)))
        .show(ui, |ui| {
            let full = ui.max_rect();
            // The chips first, from the right: a long path gives way to them, and not the other way round.
            let chips = ui.scope_builder(egui::UiBuilder::new().max_rect(full).layout(Layout::right_to_left(Align::Center)), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (icon, label, tip) = if offline {
                    ("cloud-off", tl!("Offline"), tl!("Nothing is downloaded. Click to allow downloading schemas."))
                } else {
                    ("cloud", tl!("Online"), tl!("Schemas are downloaded when a document names one that nothing here has. Click to work offline."))
                };
                if kit::chip(ui, if offline { Tone::Warn } else { Tone::Neutral }, Some(icon), label, true).on_hover_text(tip).clicked() {
                    command = Some("xml.work_offline");
                }
                if kit::chip(ui, standing_now.tone, Some(standing_now.icon), &standing_now.text, true)
                    .on_hover_text(tl!("Show the problems"))
                    .clicked()
                {
                    command = Some("view.messages");
                }
                if let Some(chip) = &grammar {
                    let (icon, label) = match chip.kind {
                        Some(GrammarKind::Dtd) => ("file-code", format!("DTD · {}", chip.label)),
                        Some(GrammarKind::Xsd) => ("book-open", format!("XSD · {}", chip.label)),
                        None => ("plus", chip.label.clone()),
                    };
                    if kit::chip(ui, chip.tone, Some(icon), &label, true).on_hover_text(&chip.tip).clicked() {
                        command = Some("xml.assign_schema");
                    }
                }
            });
            let crumbs = egui::Rect::from_min_max(full.min, egui::pos2((chips.response.rect.left() - 14.0).max(full.min.x), full.max.y));
            ui.scope_builder(egui::UiBuilder::new().max_rect(crumbs).layout(Layout::left_to_right(Align::Center)), |ui| {
                jump = breadcrumb(ui, &path, crumbs.width());
            });
        });
    if let Some(range) = jump {
        app.jump_to(range.start..range.start);
    }
    if let Some(id) = command {
        app.execute(id);
    }
}

/// The path drawn as a breadcrumb, in the room it has: each name selects its element. Returns the name range to jump to.
fn breadcrumb(ui: &mut egui::Ui, path: &[(String, Option<String>, Range<usize>)], room: f32) -> Option<Range<usize>> {
    let t = Tokens::get(ui.ctx());
    let mut jump = None;
    let skipped = path.len().saturating_sub(MAX_CRUMBS);
    let shown = path.len().saturating_sub(skipped).max(1);
    // What one crumb may take: the room, less the "…" and a separator for each crumb, shared out.
    let share = ((room - 24.0 - 14.0 * shown as f32) / shown as f32 - 12.0).max(40.0);
    ui.allocate_ui_with_layout(vec2(room, HEADER_HEIGHT - 8.0), Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if skipped > 0 {
            ui.label(egui::RichText::new("…").font(theme::regular(12.5)).color(t.text_faint));
        }
        for (i, (name, label, range)) in path.iter().enumerate().skip(skipped) {
            if i > skipped || skipped > 0 {
                let middle = ui.max_rect().center().y;
                icons::paint(
                    ui,
                    egui::Rect::from_min_size(egui::pos2(ui.cursor().min.x, middle - 6.0), vec2(12.0, 12.0)),
                    "chevron-right",
                    11.0,
                    t.text_faint,
                );
                ui.add_space(12.0);
            }
            let last = i + 1 == path.len();
            let font = if last { theme::semibold(12.5) } else { theme::medium(12.5) };
            let text = match label {
                Some(label) if last => format!("{name}  {label}"),
                _ => name.clone(),
            };
            // Each crumb gets an equal share of the room at most; a longer name is elided (and shown whole on hover).
            let colour = if last { t.text } else { t.text_muted };
            let mut job = egui::text::LayoutJob::simple_singleline(text.clone(), font, colour);
            job.wrap = egui::text::TextWrapping { max_width: share, max_rows: 1, break_anywhere: true, ..Default::default() };
            let galley = ui.painter().layout_job(job);
            let elided = galley.elided;
            let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 12.0, 24.0), Sense::click());
            let name = fmt(tl!("Select element {name}"), &[("name", &text)]);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
            let press = theme::Press::track(ui, &resp);
            press.wash(ui, rect, 6, false);
            ui.painter().galley(rect.center() - galley.size() / 2.0 + press.offset(), galley, if last { t.text } else { t.text_muted });
            let resp = if elided { resp.on_hover_text(&text) } else { resp };
            if theme::hand(resp).clicked() {
                jump = Some(range.clone());
            }
        }
    });
    jump
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(depth: usize, name: &str, range: Range<usize>) -> OutlineItem {
        OutlineItem {
            depth,
            name: name.to_owned(),
            label: None,
            name_range: range.start + 1..range.start + 1 + name.len(),
            start_tag: range.clone(),
            end_tag: None,
            end_name_range: None,
            range,
        }
    }

    #[test]
    fn the_path_is_the_elements_around_the_caret_outermost_first() {
        let outline = vec![item(0, "a", 0..40), item(1, "b", 3..20), item(2, "c", 6..12), item(1, "d", 21..38)];
        let names = |caret| element_path(&outline, caret).iter().map(|i| i.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names(8), ["a", "b", "c"]);
        assert_eq!(names(25), ["a", "d"]);
        assert_eq!(names(1), ["a"]);
        assert_eq!(names(99), Vec::<&str>::new(), "past the root");
        assert_eq!(names(0), ["a"], "at the very start");
        assert_eq!(names(40), ["a"], "right after the end of the root is still in it (the caret is at its edge)");
        assert_eq!(names(20), ["a", "b"], "and at the end of a child");
        assert_eq!(names(41), Vec::<&str>::new());
        assert!(element_path(&[], 0).is_empty());
    }

    #[test]
    fn a_doctype_is_named_by_its_dtd_file_its_public_identifier_or_where_it_is() {
        assert_eq!(dtd_label("<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.1//EN\" \"http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd\">"), "xhtml11.dtd");
        assert_eq!(dtd_label("<!DOCTYPE r SYSTEM \"r.dtd\">"), "r.dtd");
        assert_eq!(dtd_label("<!DOCTYPE r PUBLIC \"-//X//DTD R//EN\">"), "-//X//DTD R//EN");
        assert_eq!(dtd_label("<!DOCTYPE r [<!ELEMENT r EMPTY>]>"), "declared in the document");
        assert_eq!(dtd_label("<!DOCTYPE r SYSTEM 'a/b/'>"), "declared in the document", "no file name in it");
        assert_eq!(
            dtd_label("<!DOCTYPE r PUBLIC \"-//X//SYSTEM R//EN\" \"http://x/r.dtd\">"),
            "r.dtd",
            "SYSTEM inside a public identifier is not the keyword"
        );
        assert_eq!(dtd_label("<!DOCTYPE r [<!ENTITY e SYSTEM \"e.txt\">]>"), "declared in the document", "an entity is not the DTD");
        assert_eq!(dtd_label(""), "declared in the document");
        assert_eq!(dtd_label("<!DOCTYPE r SYSTEM \"unterminated"), "declared in the document");
    }

    #[test]
    fn a_schema_is_named_by_its_file_or_its_namespace() {
        assert_eq!(schema_label("../s/library.xsd", None), "library.xsd");
        assert_eq!(schema_label("C:\\s\\library.xsd", None), "library.xsd");
        assert_eq!(schema_label("http://example.com/s/", Some("urn:x")), "urn:x");
        assert_eq!(schema_label("", Some("urn:x")), "urn:x");
        assert_eq!(schema_label("", None), "a schema");
    }

    #[test]
    fn the_standing_says_how_the_document_is() {
        assert_eq!(standing(None, None).text, "Checking…");
        let analysis = |text: &str| xmlcraft_engine::analysis::analyze(text, 0);
        let good = analysis("<a/>");
        assert_eq!((standing(Some(&good), None).tone, standing(Some(&good), None).text), (Tone::Good, "Well-formed".to_owned()));
        assert_eq!(standing(Some(&good), Some((0, true))).text, "Valid");
        assert_eq!(standing(Some(&good), Some((1, false))).tone, Tone::Bad, "one validity error is an error");
        assert_eq!(standing(Some(&good), Some((2, false))).text, "2 validity errors");
        assert_eq!(standing(Some(&good), Some((2, false))).tone, Tone::Bad);
        let bad = analysis("<a>");
        assert_eq!(standing(Some(&bad), Some((0, true))).tone, Tone::Bad, "a text that is not well-formed is not valid whatever a validation said");
        assert!(standing(Some(&bad), None).text.ends_with("errors"));
        // A check that was not made, or that failed, is not a verdict.
        let mut skipped = analysis("<a/>");
        skipped.skipped = Some(Skipped::TooLarge);
        assert_eq!((standing(Some(&skipped), Some((0, true))).tone, standing(Some(&skipped), None).text), (Tone::Neutral, "Not checked".to_owned()));
        skipped.skipped = Some(Skipped::Failed);
        assert_eq!((standing(Some(&skipped), None).tone, standing(Some(&skipped), None).text), (Tone::Bad, "The check failed".to_owned()));
    }
}
