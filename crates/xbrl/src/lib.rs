#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! XBRL instances: the contexts, units and facts of an XBRL 2.1 instance (`xbrli:xbrl`) or of an Inline XBRL document (`ix:` tags in
//! XHTML), and the checks of the specification that need no taxonomy.
//!
//! - **Layer:** L4, no UI and no I/O, so it compiles for the web.
//! - [`read`] reads either kind of document into an [`Instance`]; every context, unit and fact keeps its place in the text.
//! - [`check`] reports what is wrong with the instance on its own: a fact whose `contextRef` or `unitRef` names nothing, contexts and
//!   units that are defined twice or never used, a period that is no date or that ends before it starts, `decimals` and `precision`
//!   together, a nil fact with a value, a number that is not a number, facts that say two things about the same concept, context and
//!   unit. What needs the taxonomy (is this concept numeric, does it belong to this period type, does the calculation add up) is not
//!   checked here: the concept names are shown as the instance spells them.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

mod calc;
pub mod taxonomy;

use xmlcraft_syntax::{Severity, parse};
use xmlcraft_xsd::dom::{self, Dom, El};

/// The XBRL 2.1 instance namespace.
pub const XBRLI: &str = "http://www.xbrl.org/2003/instance";
/// The linkbase namespace (`schemaRef`, `linkbaseRef`).
pub const LINK: &str = "http://www.xbrl.org/2003/linkbase";
/// The XLink namespace.
pub const XLINK: &str = "http://www.w3.org/1999/xlink";
/// The Inline XBRL 1.1 namespace.
pub const IX: &str = "http://www.xbrl.org/2013/inlineXBRL";
/// The XBRL Dimensions instance namespace.
pub const XBRLDI: &str = "http://xbrl.org/2006/xbrldi";
const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";
const ISO4217: &str = "http://www.xbrl.org/2003/iso4217";

/// The most facts, contexts and units read.
const MAX_ITEMS: usize = 5_000_000;

/// A document that cannot be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XbrlError(pub String);

impl std::fmt::Display for XbrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for XbrlError {}

/// Which kind of document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An XBRL 2.1 instance (`xbrli:xbrl`).
    Instance,
    /// An Inline XBRL document (XHTML with `ix:` tags).
    Inline,
}

/// The period of a context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Period {
    Instant(String),
    Duration(String, String),
    Forever,
}

impl Period {
    /// `2025-12-31`, `2025-01-01 – 2025-12-31` or `forever`.
    pub fn label(&self) -> String {
        match self {
            Period::Instant(d) => d.clone(),
            Period::Duration(a, b) => format!("{a} – {b}"),
            Period::Forever => "forever".to_owned(),
        }
    }
}

/// A dimension of a context and its member (explicit: the member's name; typed: the text of its element).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dimension {
    pub dimension: String,
    pub member: String,
    pub typed: bool,
    /// In the `segment` or the `scenario`.
    pub in_scenario: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    pub id: String,
    pub entity_scheme: String,
    pub entity: String,
    pub period: Option<Period>,
    pub dimensions: Vec<Dimension>,
    pub span: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    pub id: String,
    /// The measures (`iso4217:EUR`), or for a divide `numerator / denominator`.
    pub measure: String,
    pub span: Range<usize>,
}

/// One fact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fact {
    /// The concept as `{namespace}local`, and as the document spells it (`us-gaap:Assets`).
    pub concept: String,
    pub name: String,
    pub value: String,
    pub context_ref: Option<String>,
    pub unit_ref: Option<String>,
    pub decimals: Option<String>,
    pub precision: Option<String>,
    pub nil: bool,
    pub id: Option<String>,
    /// The element (or, for Inline XBRL, the tag) in the text.
    pub span: Range<usize>,
    /// A problem with the value the reader found (an Inline number that is not one).
    pub value_problem: Option<String>,
}

/// An instance read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    pub kind: Kind,
    /// The `xlink:href` of each `link:schemaRef` (of the `ix:references` of Inline XBRL).
    pub schema_refs: Vec<String>,
    pub contexts: Vec<Context>,
    pub units: Vec<Unit>,
    pub facts: Vec<Fact>,
}

/// How serious a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
}

/// One thing the checks found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// A short code: `xbrl.unknown-context`.
    pub code: &'static str,
    pub level: Level,
    pub message: String,
    pub span: Range<usize>,
}

fn attr<'a>(el: &'a El, name: &str) -> Option<&'a str> {
    el.attrs.iter().find(|a| a.namespace.is_none() && a.local == name).map(|a| a.value.as_str())
}

fn attr_ns<'a>(el: &'a El, namespace: &str, name: &str) -> Option<&'a str> {
    el.attrs.iter().find(|a| a.namespace.as_deref() == Some(namespace) && a.local == name).map(|a| a.value.as_str())
}

fn child<'a>(tree: &'a Dom, el: &El, namespace: &str, local: &str) -> Option<&'a El> {
    el.children.iter().filter_map(|i| tree.elements.get(*i)).find(|c| c.local == local && c.namespace.as_deref() == Some(namespace))
}

fn children<'a>(tree: &'a Dom, el: &'a El) -> impl Iterator<Item = &'a El> + 'a {
    el.children.iter().filter_map(move |i| tree.elements.get(*i))
}

fn text(tree: &Dom, el: &El) -> String {
    tree.all_text.get(el.deep.clone()).unwrap_or_default().trim().to_owned()
}

/// The name an attribute value spells (`iso4217:EUR`), expanded with the prefixes in scope at `at`.
fn expand(tree: &Dom, at: usize, value: &str) -> String {
    let value = value.trim();
    match value.split_once(':') {
        Some((prefix, local)) => match tree.lookup(at, Some(prefix)) {
            Some(ns) => format!("{{{ns}}}{local}"),
            None => value.to_owned(),
        },
        None => value.to_owned(),
    }
}

fn index_of(tree: &Dom, el: &El) -> usize {
    tree.elements.iter().position(|e| std::ptr::eq(e, el)).unwrap_or(0)
}

/// Read an XBRL 2.1 instance or an Inline XBRL document.
pub fn read(xml: &str) -> Result<Instance, XbrlError> {
    let parsed = parse(xml);
    if parsed.diagnostics().iter().any(|d| d.severity() == Severity::Error) {
        return Err(XbrlError("the document is not well-formed XML".to_owned()));
    }
    let tree = dom::build(&parsed.syntax());
    let Some(root) = tree.elements.first() else { return Err(XbrlError("the document has no root element".to_owned())) };
    if root.local == "xbrl" && root.namespace.as_deref() == Some(XBRLI) {
        return Ok(read_instance(&tree, root));
    }
    if tree.elements.iter().any(|e| e.namespace.as_deref() == Some(IX)) {
        return Ok(read_inline(&tree));
    }
    Err(XbrlError("this is not an XBRL document (the root is not xbrli:xbrl and there are no Inline XBRL tags)".to_owned()))
}

/// Whether the text looks like an XBRL instance or an Inline XBRL document, without reading all of it (the root element and a quick
/// look for the Inline XBRL namespace).
pub fn is_xbrl(xml: &str) -> bool {
    (xml.contains(XBRLI) && xml.contains("xbrl")) || xml.contains(IX)
}

fn read_context(tree: &Dom, el: &El) -> Option<Context> {
    let id = attr(el, "id")?.to_owned();
    let entity_el = child(tree, el, XBRLI, "entity");
    let identifier = entity_el.and_then(|e| child(tree, e, XBRLI, "identifier"));
    let period = child(tree, el, XBRLI, "period").and_then(|p| {
        if let Some(i) = child(tree, p, XBRLI, "instant") {
            Some(Period::Instant(text(tree, i)))
        } else if child(tree, p, XBRLI, "forever").is_some() {
            Some(Period::Forever)
        } else {
            let start = child(tree, p, XBRLI, "startDate")?;
            let end = child(tree, p, XBRLI, "endDate")?;
            Some(Period::Duration(text(tree, start), text(tree, end)))
        }
    });
    let mut dimensions = Vec::new();
    let mut add = |container: Option<&El>, in_scenario: bool| {
        let Some(container) = container else { return };
        for member in children(tree, container) {
            if member.namespace.as_deref() != Some(XBRLDI) {
                continue;
            }
            let at = index_of(tree, member);
            let dimension = attr(member, "dimension").map(|d| expand(tree, at, d)).unwrap_or_default();
            match member.local.as_str() {
                "explicitMember" => {
                    dimensions.push(Dimension { dimension, member: expand(tree, at, &text(tree, member)), typed: false, in_scenario })
                }
                "typedMember" => dimensions.push(Dimension { dimension, member: text(tree, member), typed: true, in_scenario }),
                _ => {}
            }
        }
    };
    add(entity_el.and_then(|e| child(tree, e, XBRLI, "segment")), false);
    add(child(tree, el, XBRLI, "scenario"), true);
    Some(Context {
        id,
        entity_scheme: identifier.and_then(|i| attr(i, "scheme")).unwrap_or_default().to_owned(),
        entity: identifier.map(|i| text(tree, i)).unwrap_or_default(),
        period,
        dimensions,
        span: el.range.clone(),
    })
}

fn read_unit(tree: &Dom, el: &El) -> Option<Unit> {
    let id = attr(el, "id")?.to_owned();
    let measures = |container: &El| -> Vec<String> {
        children(tree, container).filter(|m| m.local == "measure").map(|m| expand(tree, index_of(tree, m), &text(tree, m))).collect()
    };
    let short = |name: &str| {
        name.rsplit_once('}')
            .map_or(name.to_owned(), |(ns, local)| if ns.trim_start_matches('{') == ISO4217 { local.to_owned() } else { name.to_owned() })
    };
    let measure = match child(tree, el, XBRLI, "divide") {
        Some(divide) => {
            let top = child(tree, divide, XBRLI, "unitNumerator").map(measures).unwrap_or_default();
            let bottom = child(tree, divide, XBRLI, "unitDenominator").map(measures).unwrap_or_default();
            format!(
                "{} / {}",
                top.iter().map(|m| short(m)).collect::<Vec<_>>().join("*"),
                bottom.iter().map(|m| short(m)).collect::<Vec<_>>().join("*")
            )
        }
        None => measures(el).iter().map(|m| short(m)).collect::<Vec<_>>().join("*"),
    };
    Some(Unit { id, measure, span: el.range.clone() })
}

fn read_instance(tree: &Dom, root: &El) -> Instance {
    let mut instance = Instance { kind: Kind::Instance, schema_refs: Vec::new(), contexts: Vec::new(), units: Vec::new(), facts: Vec::new() };
    for el in children(tree, root) {
        if instance.contexts.len() + instance.units.len() + instance.facts.len() > MAX_ITEMS {
            break;
        }
        match (el.namespace.as_deref(), el.local.as_str()) {
            (Some(LINK), "schemaRef") => instance.schema_refs.push(attr_ns(el, XLINK, "href").unwrap_or_default().to_owned()),
            (Some(LINK), _) => {}
            (Some(XBRLI), "context") => instance.contexts.extend(read_context(tree, el)),
            (Some(XBRLI), "unit") => instance.units.extend(read_unit(tree, el)),
            (Some(XBRLI), _) => {}
            (ns, local) => {
                // Anything else directly under the root is a fact (footnote links are in the link namespace, handled above).
                let nil = attr_ns(el, XSI, "nil").is_some_and(|v| v == "true" || v == "1");
                instance.facts.push(Fact {
                    concept: format!("{{{}}}{local}", ns.unwrap_or_default()),
                    name: el.qname.clone(),
                    value: text(tree, el),
                    context_ref: attr(el, "contextRef").map(str::to_owned),
                    unit_ref: attr(el, "unitRef").map(str::to_owned),
                    decimals: attr(el, "decimals").map(str::to_owned),
                    precision: attr(el, "precision").map(str::to_owned),
                    nil,
                    id: attr(el, "id").map(str::to_owned),
                    span: el.range.clone(),
                    value_problem: None,
                });
            }
        }
    }
    instance
}

/// A number as Inline XBRL writes it in the default format: digits with `,` or spaces between thousands and `.` before the fraction.
fn inline_number(raw: &str, scale: i32, negative: bool) -> Result<String, String> {
    let digits: String = raw.chars().filter(|c| !matches!(c, ',' | ' ' | '\u{a0}')).collect();
    let (int, frac) = digits.split_once('.').unwrap_or((digits.as_str(), ""));
    if int.is_empty() && frac.is_empty() || !int.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("\"{raw}\" is not a number in the default format (use ixt transformation formats for others)"));
    }
    // Move the decimal point by the scale.
    let mut all: Vec<char> = format!("{int}{frac}").chars().collect();
    let mut point = i64::try_from(int.len()).unwrap_or(0) + i64::from(scale);
    if point < 0 {
        let pad = usize::try_from(-point).unwrap_or(0).min(400);
        let mut zeros = vec!['0'; pad];
        zeros.append(&mut all);
        all = zeros;
        point = 0;
    }
    let point = usize::try_from(point).unwrap_or(0);
    while all.len() < point && all.len() < 400 {
        all.push('0');
    }
    let (a, b) = all.split_at(point.min(all.len()));
    let int_part: String = a.iter().collect::<String>().trim_start_matches('0').to_owned();
    let mut out = if int_part.is_empty() { "0".to_owned() } else { int_part };
    let frac_part: String = b.iter().collect::<String>().trim_end_matches('0').to_owned();
    if !frac_part.is_empty() {
        out.push('.');
        out.push_str(&frac_part);
    }
    Ok(if negative && out != "0" { format!("-{out}") } else { out })
}

fn read_inline(tree: &Dom) -> Instance {
    let mut instance = Instance { kind: Kind::Inline, schema_refs: Vec::new(), contexts: Vec::new(), units: Vec::new(), facts: Vec::new() };
    for el in &tree.elements {
        if instance.contexts.len() + instance.units.len() + instance.facts.len() > MAX_ITEMS {
            break;
        }
        match (el.namespace.as_deref(), el.local.as_str()) {
            (Some(LINK), "schemaRef") => instance.schema_refs.push(attr_ns(el, XLINK, "href").unwrap_or_default().to_owned()),
            (Some(XBRLI), "context") => instance.contexts.extend(read_context(tree, el)),
            (Some(XBRLI), "unit") => instance.units.extend(read_unit(tree, el)),
            (Some(IX), "nonNumeric" | "nonFraction" | "fraction") => {
                let at = index_of(tree, el);
                let name = attr(el, "name").unwrap_or_default();
                let nil = attr_ns(el, XSI, "nil").is_some_and(|v| v == "true" || v == "1");
                let raw = text(tree, el);
                let (value, value_problem) = if el.local == "nonFraction" && !nil {
                    let scale = attr(el, "scale").and_then(|s| s.trim().parse::<i32>().ok()).unwrap_or(0).clamp(-100, 100);
                    match inline_number(&raw, scale, attr(el, "sign") == Some("-")) {
                        Ok(v) => (v, None),
                        Err(why) if attr(el, "format").is_some() => {
                            (raw.clone(), Some(format!("{why} (the format {} is not applied)", attr(el, "format").unwrap_or_default())))
                        }
                        Err(why) => (raw.clone(), Some(why)),
                    }
                } else {
                    (raw, None)
                };
                instance.facts.push(Fact {
                    concept: expand(tree, at, name),
                    name: name.to_owned(),
                    value,
                    context_ref: attr(el, "contextRef").map(str::to_owned),
                    unit_ref: attr(el, "unitRef").map(str::to_owned),
                    decimals: attr(el, "decimals").map(str::to_owned),
                    precision: attr(el, "precision").map(str::to_owned),
                    nil,
                    id: attr(el, "id").map(str::to_owned),
                    span: el.range.clone(),
                    value_problem,
                });
            }
            _ => {}
        }
    }
    instance
}

fn is_date(text: &str) -> bool {
    let t = text.trim();
    // `2025-12-31`, or a date-time `2025-12-31T00:00:00` (the instant of a period may be one).
    let date = t.split('T').next().unwrap_or(t);
    let mut parts = date.split('-');
    let (Some(y), Some(m), Some(d), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else { return false };
    let (Ok(year), Ok(month), Ok(day)) = (y.parse::<i32>(), m.parse::<u32>(), d.parse::<u32>()) else { return false };
    if y.len() < 4 || m.len() != 2 || d.len() != 2 || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let last = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=last).contains(&day)
}

fn is_decimal(text: &str) -> bool {
    let t = text.trim();
    let t = t.strip_prefix(['+', '-']).unwrap_or(t);
    let (int, frac) = t.split_once('.').unwrap_or((t, ""));
    !(int.is_empty() && frac.is_empty()) && int.chars().all(|c| c.is_ascii_digit()) && frac.chars().all(|c| c.is_ascii_digit())
}

fn is_integer(text: &str) -> bool {
    let t = text.trim();
    let t = t.strip_prefix(['+', '-']).unwrap_or(t);
    !t.is_empty() && t.chars().all(|c| c.is_ascii_digit())
}

/// The checks of the specification that need no taxonomy.
pub fn check(instance: &Instance) -> Vec<Finding> {
    let mut found: Vec<Finding> = Vec::new();
    let mut add = |code: &'static str, level: Level, message: String, span: Range<usize>| found.push(Finding { code, level, message, span });
    if instance.schema_refs.is_empty() && instance.kind == Kind::Instance {
        add("xbrl.no-schema-ref", Level::Warning, "The instance has no schemaRef: it names no taxonomy.".to_owned(), 0..0);
    }
    let mut context_ids: HashMap<&str, usize> = HashMap::new();
    for c in &instance.contexts {
        if context_ids.insert(c.id.as_str(), 0).is_some() {
            add("xbrl.duplicate-context", Level::Error, format!("The context id {} is used by more than one context.", c.id), c.span.clone());
        }
        match &c.period {
            None => add("xbrl.bad-period", Level::Error, format!("The context {} has no valid period.", c.id), c.span.clone()),
            Some(Period::Instant(d)) if !is_date(d) => {
                add("xbrl.bad-period", Level::Error, format!("The instant {d:?} of the context {} is not a date.", c.id), c.span.clone())
            }
            Some(Period::Duration(a, b)) => {
                if !is_date(a) || !is_date(b) {
                    add(
                        "xbrl.bad-period",
                        Level::Error,
                        format!("The period {a:?} to {b:?} of the context {} has a start or an end that is not a date.", c.id),
                        c.span.clone(),
                    );
                } else if a.split('T').next() > b.split('T').next() {
                    add("xbrl.period-order", Level::Error, format!("The context {} ends ({b}) before it starts ({a}).", c.id), c.span.clone());
                }
            }
            _ => {}
        }
        if c.entity.is_empty() || c.entity_scheme.is_empty() {
            add("xbrl.bad-entity", Level::Error, format!("The context {} has no entity identifier with a scheme.", c.id), c.span.clone());
        }
    }
    let mut unit_ids: HashSet<&str> = HashSet::new();
    for u in &instance.units {
        if !unit_ids.insert(u.id.as_str()) {
            add("xbrl.duplicate-unit", Level::Error, format!("The unit id {} is used by more than one unit.", u.id), u.span.clone());
        }
        if u.measure.trim().is_empty() || u.measure.trim() == "/" {
            add("xbrl.empty-unit", Level::Error, format!("The unit {} has no measure.", u.id), u.span.clone());
        }
    }
    let mut used_contexts: HashSet<&str> = HashSet::new();
    let mut used_units: HashSet<&str> = HashSet::new();
    let mut seen: HashMap<(String, String, String), String> = HashMap::new();
    for f in &instance.facts {
        match f.context_ref.as_deref() {
            None => add("xbrl.no-context", Level::Error, format!("The fact {} has no contextRef.", f.name), f.span.clone()),
            Some(c) if !context_ids.contains_key(c) => add(
                "xbrl.unknown-context",
                Level::Error,
                format!("The fact {} refers to the context {c}, which is not defined.", f.name),
                f.span.clone(),
            ),
            Some(c) => {
                used_contexts.insert(c);
            }
        }
        if let Some(u) = f.unit_ref.as_deref() {
            if !unit_ids.contains(u) {
                add("xbrl.unknown-unit", Level::Error, format!("The fact {} refers to the unit {u}, which is not defined.", f.name), f.span.clone());
            } else {
                used_units.insert(u);
            }
            if !f.nil && f.value_problem.is_none() && !f.value.is_empty() && !is_decimal(&f.value) && !f.value.eq_ignore_ascii_case("nan") {
                add(
                    "xbrl.bad-number",
                    Level::Error,
                    format!("The value {:?} of {} is not a number, but the fact has a unit.", f.value, f.name),
                    f.span.clone(),
                );
            }
        } else if instance.kind == Kind::Instance && !f.nil && f.value.len() < 40 && is_decimal(&f.value) && f.decimals.is_some() {
            add(
                "xbrl.number-without-unit",
                Level::Error,
                format!("The fact {} has decimals but no unitRef: a number needs a unit.", f.name),
                f.span.clone(),
            );
        }
        if f.decimals.is_some() && f.precision.is_some() {
            add("xbrl.decimals-and-precision", Level::Error, format!("The fact {} has both decimals and precision.", f.name), f.span.clone());
        }
        if let Some(d) = &f.decimals
            && d != "INF"
            && !is_integer(d)
        {
            add("xbrl.bad-decimals", Level::Error, format!("The decimals {d:?} of {} is not an integer or INF.", f.name), f.span.clone());
        }
        if let Some(p) = &f.precision
            && p != "INF"
            && !is_integer(p)
        {
            add("xbrl.bad-precision", Level::Error, format!("The precision {p:?} of {} is not a number or INF.", f.name), f.span.clone());
        }
        if f.nil && !f.value.is_empty() {
            add("xbrl.nil-with-value", Level::Error, format!("The fact {} is nil but has a value.", f.name), f.span.clone());
        }
        if let Some(why) = &f.value_problem {
            add("xbrl.bad-number", Level::Error, format!("The fact {}: {why}.", f.name), f.span.clone());
        }
        if let (Some(c), false) = (f.context_ref.as_deref(), f.nil) {
            let key = (f.concept.clone(), c.to_owned(), f.unit_ref.clone().unwrap_or_default());
            match seen.get(&key) {
                Some(value) if *value == f.value => {
                    add(
                        "xbrl.duplicate-fact",
                        Level::Warning,
                        format!("The fact {} is stated twice with the same value for the same context.", f.name),
                        f.span.clone(),
                    );
                }
                Some(value) => add(
                    "xbrl.inconsistent-duplicate",
                    Level::Error,
                    format!("The fact {} is stated again for the same context with another value ({:?} and {value:?}).", f.name, f.value),
                    f.span.clone(),
                ),
                None => {
                    seen.insert(key, f.value.clone());
                }
            }
        }
    }
    for c in &instance.contexts {
        if !used_contexts.contains(c.id.as_str()) {
            add("xbrl.unused-context", Level::Warning, format!("The context {} is not used by any fact.", c.id), c.span.clone());
        }
    }
    for u in &instance.units {
        if !used_units.contains(u.id.as_str()) {
            add("xbrl.unused-unit", Level::Warning, format!("The unit {} is not used by any fact.", u.id), u.span.clone());
        }
    }
    found.sort_by_key(|f| (f.span.start, f.code));
    found
}
