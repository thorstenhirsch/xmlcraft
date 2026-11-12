//! The invoice template: a small declarative XML format (an HTML-like subset) that says what the visual invoice shows and where.
//!
//! ```xml
//! <invoice-template version="1" page="A4" margin="20mm" font-size="9.5" lang="de">
//!   <header> … </header>   <body> … </body>   <footer> … </footer>
//! </invoice-template>
//! ```
//!
//! Blocks: `<block>` (text with placeholders; `style` title, heading, normal, small or bold; `align` left, center or right; `if`),
//! `<columns widths="55% 45%">` with `<column>`s, `<fields>` with `<field label="…" value="…"/>` (a label and value list), `<table
//! rows="lines" widths="…">` with `<col title="…" value="…" sub="…" align="…"/>`, and `<totals/>`. Every block takes `if="path"`
//! (hidden when the model has no value there) and `top="45mm"` (the block starts no higher than that distance from the top of the
//! page: how the address of the built-in German template sits in the window of a DIN 5008 envelope).
//!
//! Reading is bounded (the template is at most [`MAX_TEMPLATE_BYTES`], nested at most [`MAX_DEPTH`] deep, with at most [`MAX_BLOCKS`]
//! blocks), and every problem is an error with its line and column, never a panic.

use std::ops::Range;

use xmlcraft_syntax::{Severity, parse};
use xmlcraft_text::Buffer;
use xmlcraft_xsd::dom::{self, Dom, El};

/// The largest template read.
pub const MAX_TEMPLATE_BYTES: usize = 1024 * 1024;
/// The deepest nesting of columns.
pub const MAX_DEPTH: usize = 8;
/// The most blocks of a template (all sections together).
pub const MAX_BLOCKS: usize = 5_000;
/// The most columns of a `<columns>` or a table.
pub const MAX_COLUMNS: usize = 24;

const MM: f64 = 72.0 / 25.4;

/// The language the template formats dates, numbers and the fixed labels in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    De,
    En,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Title,
    Heading,
    Normal,
    Small,
    Bold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// A filter of a placeholder (`{issue_date|date}`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    /// An ISO date in the form of the language (de `31.12.2025`, en `2025-12-31`).
    Date,
    /// An amount with the document's decimals, grouped, in the language's marks, with its currency code.
    Money,
    /// A rate with a percent sign.
    Percent,
    /// A number in the language's decimal mark (a quantity).
    Number,
    /// A unit code (UN/ECE Recommendation 20) as a short word where one is known (`C62` → `Stk.`/`pcs`).
    Unit,
    Upper,
}

/// A piece of a text: literal text or a placeholder.
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Text(String),
    Value { path: String, filter: Option<Filter>, range: Range<usize> },
}

/// A text with placeholders, and where it is in the template.
#[derive(Clone, Debug, PartialEq)]
pub struct Text {
    pub pieces: Vec<Piece>,
    pub range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Col {
    pub title: Text,
    pub value: Text,
    pub sub: Option<Text>,
    pub align: Align,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldItem {
    pub label: Text,
    pub value: Text,
    pub cond: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Text { style: Style, align: Align, text: Text },
    Columns { widths: Vec<f64>, columns: Vec<Vec<Block>> },
    Fields { items: Vec<FieldItem> },
    Table { rows: String, widths: Vec<f64>, cols: Vec<Col>, range: Range<usize> },
    Totals,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub kind: Kind,
    /// Hidden unless the model has a value at this path.
    pub cond: Option<String>,
    /// The least distance from the top of the page the block starts at, in points.
    pub top: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    /// Width and height in points.
    pub page: (f64, f64),
    pub margin: f64,
    pub font_size: f64,
    pub lang: Lang,
    pub header: Vec<Block>,
    pub body: Vec<Block>,
    pub footer: Vec<Block>,
}

/// A problem of a template, with its place (1-based line and column, the column counting characters).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateError {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the template, line {} column {}: {}", self.line, self.column, self.message)
    }
}

/// The built-in German template (DIN 5008 address window).
pub const DE: &str = include_str!("../templates/de.xml");
/// The built-in English template.
pub const EN: &str = include_str!("../templates/en.xml");

struct Reader<'a> {
    text: &'a str,
    buffer: Buffer,
    tree: &'a Dom,
    blocks: usize,
}

impl Reader<'_> {
    fn error(&self, at: usize, message: impl Into<String>) -> TemplateError {
        let p = self.buffer.position(at.min(self.text.len()));
        TemplateError { message: message.into(), line: p.line.saturating_add(1), column: p.col.saturating_add(1) }
    }

    fn attr<'b>(&self, el: &'b El, name: &str) -> Option<&'b dom::Attr> {
        el.attrs.iter().find(|a| a.local == name && a.namespace.is_none())
    }

    /// Every attribute of `el` must be one of `allowed`.
    fn only(&self, el: &El, allowed: &[&str]) -> Result<(), TemplateError> {
        for a in &el.attrs {
            if a.namespace.is_some() || !allowed.contains(&a.local.as_str()) {
                let list = if allowed.is_empty() { "none".to_owned() } else { allowed.join(", ") };
                return Err(self.error(a.name_range.start, format!("<{}> has no attribute {} (it takes {list})", el.local, a.qname)));
            }
        }
        Ok(())
    }

    fn no_text(&self, el: &El) -> Result<(), TemplateError> {
        if !el.text.trim().is_empty() {
            return Err(self.error(el.range.start, format!("<{}> holds blocks, not text", el.local)));
        }
        Ok(())
    }
}

/// Read a template.
pub fn read(text: &str) -> Result<Template, TemplateError> {
    let at_start = |message: String| TemplateError { message, line: 1, column: 1 };
    if text.len() > MAX_TEMPLATE_BYTES {
        return Err(at_start(format!("the template is larger than {} KiB", MAX_TEMPLATE_BYTES / 1024)));
    }
    let parsed = parse(text);
    let buffer = Buffer::from_text(text);
    if let Some(d) = parsed.diagnostics().iter().find(|d| d.severity() == Severity::Error) {
        let p = buffer.position(d.span.start.min(text.len()));
        return Err(TemplateError {
            message: format!("the template is not well-formed: {}", d.message()),
            line: p.line.saturating_add(1),
            column: p.col.saturating_add(1),
        });
    }
    let tree = dom::build(&parsed.syntax());
    if let Some(why) = tree.truncated {
        return Err(at_start(format!("the template is too large to read ({why})")));
    }
    let mut r = Reader { text, buffer, tree: &tree, blocks: 0 };
    let root = tree.root().and_then(|i| tree.elements.get(i)).ok_or_else(|| at_start("the template has no root element".into()))?;
    if root.local != "invoice-template" || root.namespace.is_some() {
        return Err(r.error(root.name_range.start, format!("the root element is <invoice-template>, not <{}>", root.qname)));
    }
    r.only(root, &["version", "page", "margin", "font-size", "lang"])?;
    match r.attr(root, "version").map(|a| a.value.trim()) {
        Some("1") => {}
        Some(v) => return Err(r.error(root.name_range.start, format!("version {v} is not known; this XmlCraft reads version 1"))),
        None => return Err(r.error(root.name_range.start, "version=\"1\" is required")),
    }
    let page = match r.attr(root, "page").map(|a| (a.value.trim(), a.value_range.start)) {
        None | Some(("A4", _)) => (210.0 * MM, 297.0 * MM),
        Some(("Letter", _)) => (612.0, 792.0),
        Some((v, at)) => return Err(r.error(at, format!("page {v} is not known (A4 or Letter)"))),
    };
    let margin = match r.attr(root, "margin") {
        Some(a) => length(&a.value).filter(|m| *m >= 0.0 && *m * 2.0 < page.0.min(page.1) - 72.0).ok_or_else(|| {
            r.error(a.value_range.start, format!("margin {} is no length that leaves room on the page (20mm, 1.5cm, 0.75in, 54pt)", a.value))
        })?,
        None => 20.0 * MM,
    };
    let font_size = match r.attr(root, "font-size") {
        Some(a) => a
            .value
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|s| s.is_finite() && (4.0..=36.0).contains(s))
            .ok_or_else(|| r.error(a.value_range.start, format!("font-size {} is no number of points from 4 to 36", a.value)))?,
        None => 9.5,
    };
    let lang = match r.attr(root, "lang").map(|a| (a.value.trim(), a.value_range.start)) {
        None | Some(("de", _)) => Lang::De,
        Some(("en", _)) => Lang::En,
        Some((v, at)) => return Err(r.error(at, format!("lang {v} is not known (de or en)"))),
    };
    r.no_text(root)?;
    let mut sections: [Option<Vec<Block>>; 3] = [None, None, None];
    let children: Vec<&El> = root.children.iter().filter_map(|i| tree.elements.get(*i)).collect();
    for el in children {
        let slot = match el.local.as_str() {
            "header" => 0,
            "body" => 1,
            "footer" => 2,
            other => return Err(r.error(el.name_range.start, format!("<{other}> is not a section (header, body, footer)"))),
        };
        r.only(el, &[])?;
        if sections.get(slot).is_some_and(Option::is_some) {
            return Err(r.error(el.name_range.start, format!("<{}> is given twice", el.local)));
        }
        let blocks = blocks(&mut r, el, 0)?;
        if let Some(s) = sections.get_mut(slot) {
            *s = Some(blocks);
        }
    }
    let [header, body, footer] = sections;
    Ok(Template {
        page,
        margin,
        font_size,
        lang,
        header: header.unwrap_or_default(),
        body: body.ok_or_else(|| r.error(root.name_range.start, "the template has no <body>"))?,
        footer: footer.unwrap_or_default(),
    })
}

/// A length: `20mm`, `2cm`, `1in`, `12pt` or a number of points.
pub fn length(value: &str) -> Option<f64> {
    let v = value.trim();
    let (number, factor) = if let Some(n) = v.strip_suffix("mm") {
        (n, MM)
    } else if let Some(n) = v.strip_suffix("cm") {
        (n, MM * 10.0)
    } else if let Some(n) = v.strip_suffix("in") {
        (n, 72.0)
    } else if let Some(n) = v.strip_suffix("pt") {
        (n, 1.0)
    } else {
        (v, 1.0)
    };
    let n: f64 = number.trim().parse().ok()?;
    (n.is_finite() && (0.0..=10_000.0).contains(&n)).then_some(n * factor)
}

fn widths(r: &Reader<'_>, el: &El, count: usize) -> Result<Vec<f64>, TemplateError> {
    let Some(a) = r.attr(el, "widths") else {
        let n = f64::from(u32::try_from(count.max(1)).unwrap_or(1));
        return Ok(vec![1.0 / n; count]);
    };
    let parts: Vec<&str> = a.value.split_whitespace().collect();
    if parts.len() != count {
        return Err(r.error(a.value_range.start, format!("widths gives {} widths for {count} columns", parts.len())));
    }
    let mut out = Vec::with_capacity(count);
    for p in parts {
        let w = p
            .strip_suffix('%')
            .and_then(|n| n.parse::<f64>().ok())
            .filter(|w| w.is_finite() && *w > 0.0 && *w <= 100.0)
            .ok_or_else(|| r.error(a.value_range.start, format!("{p} is no width (a percentage such as 25%)")))?;
        out.push(w / 100.0);
    }
    if out.iter().sum::<f64>() > 1.005 {
        return Err(r.error(a.value_range.start, "the widths add up to more than 100%"));
    }
    Ok(out)
}

fn blocks(r: &mut Reader<'_>, parent: &El, depth: usize) -> Result<Vec<Block>, TemplateError> {
    if depth > MAX_DEPTH {
        return Err(r.error(parent.name_range.start, format!("columns are nested more than {MAX_DEPTH} deep")));
    }
    r.no_text(parent)?;
    let tree = r.tree;
    let children: Vec<&El> = parent.children.iter().filter_map(|i| tree.elements.get(*i)).collect();
    let mut out = Vec::new();
    for el in children {
        r.blocks = r.blocks.saturating_add(1);
        if r.blocks > MAX_BLOCKS {
            return Err(r.error(el.name_range.start, format!("the template has more than {MAX_BLOCKS} blocks")));
        }
        out.push(block(r, el, depth)?);
    }
    Ok(out)
}

fn cond(r: &Reader<'_>, el: &El) -> Result<Option<String>, TemplateError> {
    match r.attr(el, "if") {
        None => Ok(None),
        Some(a) => {
            let p = a.value.trim();
            if !valid_path(p) {
                return Err(r.error(a.value_range.start, format!("if=\"{p}\" is no path (names and dots: buyer.reference)")));
            }
            Ok(Some(p.to_owned()))
        }
    }
}

fn top(r: &Reader<'_>, el: &El) -> Result<Option<f64>, TemplateError> {
    match r.attr(el, "top") {
        None => Ok(None),
        Some(a) => length(&a.value).map(Some).ok_or_else(|| r.error(a.value_range.start, format!("top {} is no length", a.value))),
    }
}

fn align(r: &Reader<'_>, el: &El) -> Result<Align, TemplateError> {
    match r.attr(el, "align").map(|a| (a.value.trim(), a.value_range.start)) {
        None | Some(("left", _)) => Ok(Align::Left),
        Some(("center", _)) => Ok(Align::Center),
        Some(("right", _)) => Ok(Align::Right),
        Some((v, at)) => Err(r.error(at, format!("align {v} is not known (left, center, right)"))),
    }
}

fn attr_text(r: &Reader<'_>, el: &El, name: &str, required: bool) -> Result<Option<Text>, TemplateError> {
    match r.attr(el, name) {
        Some(a) => Ok(Some(text(r, &a.value, a.value_range.clone())?)),
        None if required => Err(r.error(el.name_range.start, format!("<{}> needs {name}=\"…\"", el.local))),
        None => Ok(None),
    }
}

fn block(r: &mut Reader<'_>, el: &El, depth: usize) -> Result<Block, TemplateError> {
    let kind = match el.local.as_str() {
        "block" => {
            r.only(el, &["style", "align", "if", "top"])?;
            if let Some(child) = el.children.iter().filter_map(|i| r.tree.elements.get(*i)).next() {
                return Err(r.error(child.name_range.start, "<block> holds text, not elements"));
            }
            let style = match r.attr(el, "style").map(|a| (a.value.trim(), a.value_range.start)) {
                None | Some(("normal", _)) => Style::Normal,
                Some(("title", _)) => Style::Title,
                Some(("heading", _)) => Style::Heading,
                Some(("small", _)) => Style::Small,
                Some(("bold", _)) => Style::Bold,
                Some((v, at)) => return Err(r.error(at, format!("style {v} is not known (title, heading, normal, small, bold)"))),
            };
            // White space is collapsed as in HTML; the text's place is the element's.
            let collapsed = el.text.split_whitespace().collect::<Vec<_>>().join(" ");
            Kind::Text { style, align: align(r, el)?, text: text(r, &collapsed, el.range.clone())? }
        }
        "columns" => {
            r.only(el, &["widths", "if", "top"])?;
            r.no_text(el)?;
            let tree = r.tree;
            let cols: Vec<&El> = el.children.iter().filter_map(|i| tree.elements.get(*i)).collect();
            if cols.is_empty() || cols.len() > MAX_COLUMNS {
                return Err(r.error(el.name_range.start, format!("<columns> has 1 to {MAX_COLUMNS} <column>s")));
            }
            let mut columns = Vec::new();
            for c in &cols {
                if c.local != "column" {
                    return Err(r.error(c.name_range.start, format!("<columns> holds <column>s, not <{}>", c.local)));
                }
                r.only(c, &[])?;
                columns.push(blocks(r, c, depth.saturating_add(1))?);
            }
            Kind::Columns { widths: widths(r, el, cols.len())?, columns }
        }
        "fields" => {
            r.only(el, &["if", "top"])?;
            r.no_text(el)?;
            let tree = r.tree;
            let mut items = Vec::new();
            for f in el.children.iter().filter_map(|i| tree.elements.get(*i)) {
                if f.local != "field" {
                    return Err(r.error(f.name_range.start, format!("<fields> holds <field>s, not <{}>", f.local)));
                }
                r.only(f, &["label", "value", "if"])?;
                let (Some(label), Some(value)) = (attr_text(r, f, "label", true)?, attr_text(r, f, "value", true)?) else {
                    return Err(r.error(f.name_range.start, "<field> needs label and value"));
                };
                items.push(FieldItem { label, value, cond: cond(r, f)? });
            }
            Kind::Fields { items }
        }
        "table" => {
            r.only(el, &["rows", "widths", "if", "top"])?;
            r.no_text(el)?;
            let rows =
                r.attr(el, "rows").map(|a| a.value.trim().to_owned()).ok_or_else(|| r.error(el.name_range.start, "<table> needs rows=\"…\""))?;
            if !matches!(rows.as_str(), "lines" | "vat_breakdown" | "allowances_charges" | "notes") {
                let at = r.attr(el, "rows").map_or(el.name_range.start, |a| a.value_range.start);
                return Err(r.error(at, format!("rows={rows} is no list of the invoice (lines, vat_breakdown, allowances_charges, notes)")));
            }
            let tree = r.tree;
            let mut cols = Vec::new();
            for c in el.children.iter().filter_map(|i| tree.elements.get(*i)) {
                if c.local != "col" {
                    return Err(r.error(c.name_range.start, format!("<table> holds <col>s, not <{}>", c.local)));
                }
                r.only(c, &["title", "value", "sub", "align"])?;
                let title = attr_text(r, c, "title", false)?.unwrap_or(Text { pieces: Vec::new(), range: c.range.clone() });
                let Some(value) = attr_text(r, c, "value", true)? else { return Err(r.error(c.name_range.start, "<col> needs value")) };
                cols.push(Col { title, value, sub: attr_text(r, c, "sub", false)?, align: align(r, c)? });
            }
            if cols.is_empty() || cols.len() > MAX_COLUMNS {
                return Err(r.error(el.name_range.start, format!("<table> has 1 to {MAX_COLUMNS} <col>s")));
            }
            Kind::Table { rows, widths: widths(r, el, cols.len())?, cols, range: el.range.clone() }
        }
        "totals" => {
            r.only(el, &["if", "top"])?;
            r.no_text(el)?;
            if let Some(child) = el.children.iter().filter_map(|i| r.tree.elements.get(*i)).next() {
                return Err(r.error(child.name_range.start, "<totals/> is empty"));
            }
            Kind::Totals
        }
        other => {
            return Err(r.error(el.name_range.start, format!("<{other}> is not a block (block, columns, fields, table, totals)")));
        }
    };
    Ok(Block { kind, cond: cond(r, el)?, top: top(r, el)? })
}

fn valid_path(p: &str) -> bool {
    !p.is_empty() && p.len() <= 200 && p.split('.').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
}

/// Split `value` into text and placeholders (`{path}`, `{path|filter}`; `{{` and `}}` are braces). `range` is where it is.
fn text(r: &Reader<'_>, value: &str, range: Range<usize>) -> Result<Text, TemplateError> {
    let mut pieces = Vec::new();
    let mut literal = String::new();
    let mut rest = value;
    while let Some(c) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("{{") {
            literal.push('{');
            rest = after;
        } else if let Some(after) = rest.strip_prefix("}}") {
            literal.push('}');
            rest = after;
        } else if c == '{' {
            let Some(end) = rest.find('}') else { return Err(r.error(range.start, format!("a placeholder is not closed: {rest}"))) };
            let inner = rest.get(1..end).unwrap_or_default();
            let (path, filter) = match inner.split_once('|') {
                Some((p, f)) => (p.trim(), Some(f.trim())),
                None => (inner.trim(), None),
            };
            if !valid_path(path) {
                return Err(r.error(range.start, format!("{{{inner}}} is no placeholder (a path such as seller.name, then |filter)")));
            }
            let filter = match filter {
                None => None,
                Some("date") => Some(Filter::Date),
                Some("money") => Some(Filter::Money),
                Some("percent") => Some(Filter::Percent),
                Some("number") => Some(Filter::Number),
                Some("unit") => Some(Filter::Unit),
                Some("upper") => Some(Filter::Upper),
                Some(f) => {
                    return Err(r.error(range.start, format!("|{f} is no filter (date, money, percent, number, unit, upper)")));
                }
            };
            if !literal.is_empty() {
                pieces.push(Piece::Text(std::mem::take(&mut literal)));
            }
            pieces.push(Piece::Value { path: path.to_owned(), filter, range: range.clone() });
            rest = rest.get(end.saturating_add(1)..).unwrap_or_default();
        } else if c == '}' {
            return Err(r.error(range.start, "a } without its { (write }} for a brace)"));
        } else {
            literal.push(c);
            rest = rest.get(c.len_utf8()..).unwrap_or_default();
        }
    }
    if !literal.is_empty() {
        pieces.push(Piece::Text(literal));
    }
    Ok(Text { pieces, range })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(t: &str) -> TemplateError {
        read(t).expect_err("an error")
    }

    #[test]
    fn the_built_in_templates_are_read() {
        for t in [DE, EN] {
            let template = read(t).unwrap_or_else(|e| panic!("{e}"));
            assert!(!template.body.is_empty() && !template.footer.is_empty());
        }
        assert_eq!(read(DE).map(|t| t.lang), Ok(Lang::De));
        assert_eq!(read(EN).map(|t| t.lang), Ok(Lang::En));
    }

    #[test]
    fn placeholders_and_filters_are_read() {
        let t = read(
            r#"<invoice-template version="1"><body><block style="title">Rechnung {number} {{x}} {issue_date|date}</block></body></invoice-template>"#,
        )
        .expect("reads");
        let Some(Block { kind: Kind::Text { style, text, .. }, .. }) = t.body.first() else { panic!("a text block") };
        assert_eq!(*style, Style::Title);
        let shape: Vec<String> = text
            .pieces
            .iter()
            .map(|p| match p {
                Piece::Text(s) => format!("'{s}'"),
                Piece::Value { path, filter, .. } => format!("{path}:{filter:?}"),
            })
            .collect();
        assert_eq!(shape, ["'Rechnung '", "number:None", "' {x} '", "issue_date:Some(Date)"]);
        assert_eq!((t.page.0.round(), t.margin.round(), t.lang), (595.0, 57.0, Lang::De));
    }

    #[test]
    fn mistakes_are_located_errors() {
        let e = err("<invoice-template version=\"1\">\n<body>\n  <blok/>\n</body></invoice-template>");
        assert_eq!((e.line, e.column), (3, 4), "{e}");
        assert!(e.message.contains("<blok> is not a block"), "{e}");
        assert!(err("<invoice-template version=\"2\"><body/></invoice-template>").message.contains("version 2"));
        assert!(err("<invoice-template><body/></invoice-template>").message.contains("version=\"1\" is required"));
        assert!(err("<invoice-template version=\"1\"/>").message.contains("no <body>"));
        assert!(err("<invoice-template version=\"1\"><body><block>{seller.name|bogus}</block></body></invoice-template>").message.contains("|bogus"));
        assert!(err("<invoice-template version=\"1\"><body><block>{seller.name</block></body></invoice-template>").message.contains("not closed"));
        assert!(err("<invoice-template version=\"1\"><body><block>a } b</block></body></invoice-template>").message.contains("without its {"));
        assert!(
            err("<invoice-template version=\"1\"><body><block colour=\"red\"/></body></invoice-template>").message.contains("no attribute colour")
        );
        assert!(err("<invoice-template version=\"1\"><body><table rows=\"lines\" widths=\"50% 60%\"><col value=\"a\"/><col value=\"b\"/></table></body></invoice-template>").message.contains("more than 100%"));
        assert!(
            err("<invoice-template version=\"1\"><body><table rows=\"cats\"><col value=\"a\"/></table></body></invoice-template>")
                .message
                .contains("rows=cats")
        );
        assert!(err("<invoice-template version=\"1\" margin=\"300mm\"><body/></invoice-template>").message.contains("margin"));
        assert!(err("<invoice-template version=\"1\"><body><block if=\"a..b\"/></body></invoice-template>").message.contains("no path"));
        assert!(err("<invoice-template version=\"1\"><body><block>").message.contains("not well-formed"));
        assert!(err("<other/>").message.contains("<invoice-template>"));
        let deep = format!(
            "<invoice-template version=\"1\"><body>{}{}</body></invoice-template>",
            "<columns><column>".repeat(MAX_DEPTH + 2),
            "</column></columns>".repeat(MAX_DEPTH + 2)
        );
        assert!(err(&deep).message.contains("nested"));
        let big = format!("<invoice-template version=\"1\"><body><block>{}</block></body></invoice-template>", "x".repeat(MAX_TEMPLATE_BYTES));
        assert!(err(&big).message.contains("larger than"));
    }

    proptest::proptest! {
        #[test]
        fn any_template_text_is_answered_without_a_panic(body in ".{0,300}") {
            let _ = read(&format!("<invoice-template version=\"1\"><body>{body}</body></invoice-template>"));
            let _ = read(&body);
        }

        #[test]
        fn any_block_text_is_answered(text in "[^<&]{0,200}") {
            let _ = read(&format!("<invoice-template version=\"1\"><body><block>{text}</block></body></invoice-template>"));
        }
    }
}
