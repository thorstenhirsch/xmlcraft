//! The layout: a template and an invoice model become pages of positioned text runs and rules (the "page model"), which the PDF
//! writer then draws. Nothing here knows about PDF.
//!
//! - Text wraps at spaces by the measured widths of the embedded faces (a word wider than its box is broken between characters).
//! - The body flows over as many pages as it needs: blocks are cut into fragments (a line of text, a row of a table) that do not
//!   break; a table repeats its header row on each page it continues on; a heading or a table header stays with what follows it.
//! - The header and the footer are drawn on every page, with `{page}` and `{pages}`.
//! - Bounded: at most [`MAX_PAGES`] pages, [`MAX_LINES_PER_TEXT`] lines of one text; a row taller than a page is cut and said.

use std::collections::BTreeSet;

use xmlcraft_einvoice::model::{Field, Invoice, Node};

use crate::font::{Face, Font};
use crate::format::apply;
use crate::template::{Align, Block, Col, Filter, Kind, Lang, Piece, Style, Template, Text};

/// The most pages of an invoice.
pub const MAX_PAGES: usize = 2_000;
/// The most lines one text wraps to.
pub const MAX_LINES_PER_TEXT: usize = 500;

const MM: f64 = 72.0 / 25.4;
/// The padding inside a table cell, on each side.
const PAD: f64 = 2.0;

/// A positioned item of a page. `y` counts down from the top of the page, in points; for text it is the baseline.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Text { x: f64, y: f64, size: f64, face: Face, gray: f64, text: String },
    Rule { x1: f64, y1: f64, x2: f64, y2: f64, width: f64, gray: f64 },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub items: Vec<Item>,
}

/// The laid-out invoice.
#[derive(Debug)]
pub struct Laid {
    pub size: (f64, f64),
    pub pages: Vec<Page>,
    pub warnings: Vec<String>,
}

/// A piece of the flow that does not break: its items relative to its top, and its height.
#[derive(Clone, Debug, Default)]
struct Frag {
    height: f64,
    items: Vec<Item>,
    /// Keep on the page of the fragment after it (a heading, a table header).
    keep_with_next: bool,
    /// The least distance from the top of the page it starts at (`top`).
    top: Option<f64>,
    /// The header row to draw again above this fragment when it starts a page (a table row).
    repeat: Option<Box<Frag>>,
}

fn shift(items: &[Item], dx: f64, dy: f64) -> impl Iterator<Item = Item> + '_ {
    items.iter().map(move |i| match i {
        Item::Text { x, y, size, face, gray, text } => Item::Text { x: x + dx, y: y + dy, size: *size, face: *face, gray: *gray, text: text.clone() },
        Item::Rule { x1, y1, x2, y2, width, gray } => Item::Rule { x1: x1 + dx, y1: y1 + dy, x2: x2 + dx, y2: y2 + dy, width: *width, gray: *gray },
    })
}

/// What a style draws with: size, face, gray, space before and after.
fn style(s: Style, base: f64) -> (f64, Face, f64, f64, f64) {
    match s {
        Style::Title => (base * 1.6, Face::Bold, 0.0, base * 0.6, base * 0.8),
        Style::Heading => (base * 1.15, Face::Bold, 0.0, base * 0.4, base * 0.2),
        Style::Normal => (base, Face::Regular, 0.0, 0.0, 0.0),
        Style::Small => (base * 0.8, Face::Regular, 0.35, 0.0, 0.0),
        Style::Bold => (base, Face::Bold, 0.0, 0.0, 0.0),
    }
}

fn line_height(size: f64) -> f64 {
    size * 1.3
}

/// What a placeholder found.
enum Found<'n> {
    Fields(Vec<&'n Field>),
    Text(String),
    Absent,
    Unknown,
}

/// The names a path may have, below the invoice and in each list's rows.
const ADDRESS: &[&str] = &["lines", "line1", "line2", "line3", "city", "postcode", "subdivision", "country"];
const PARTY: &[&str] = &["name", "trading_name", "id", "vat_id", "tax_registration", "legal_id", "electronic_address", "reference"];
const CONTACT: &[&str] = &["name", "phone", "email"];
const TOP: &[&str] = &[
    "number",
    "type_code",
    "issue_date",
    "due_date",
    "currency",
    "buyer_reference",
    "order_reference",
    "payment_terms",
    "notes",
    "document_name",
    "lines",
    "vat_breakdown",
    "allowances_charges",
];
const PAYMENT: &[&str] = &["means_code", "reference", "iban", "bic", "account_name", "mandate"];
const TOTALS: &[&str] =
    &["line_total", "allowance_total", "charge_total", "tax_exclusive", "tax_total", "tax_inclusive", "prepaid", "rounding", "payable"];
const LINE: &[&str] = &[
    "id",
    "name",
    "description",
    "note",
    "quantity",
    "unit",
    "net_price",
    "line_total",
    "vat_category",
    "vat_rate",
    "seller_item_id",
    "period_start",
    "period_end",
];
const VAT: &[&str] = &["category", "rate", "taxable", "tax", "exemption_reason"];
const CHARGE: &[&str] = &["amount", "base", "percent", "reason", "vat_category", "vat_rate", "charge_indicator"];

fn row_names(list: &str) -> &'static [&'static str] {
    match list {
        "lines" => LINE,
        "vat_breakdown" => VAT,
        "allowances_charges" => CHARGE,
        _ => &[],
    }
}

/// Whether `path` names something of the model (whether or not this invoice has it).
fn known(path: &str, row: Option<&str>) -> bool {
    let parts: Vec<&str> = path.split('.').collect();
    let address = |rest: &[&str]| matches!(rest, [one] if ADDRESS.contains(one));
    match parts.as_slice() {
        [one] if row.is_some_and(|r| r == "notes" && *one == "text") => true,
        [one] if row.is_some_and(|r| row_names(r).contains(one)) => true,
        [one] if TOP.contains(one) => true,
        ["seller" | "buyer" | "payee", rest @ ..] => match rest {
            [one] => PARTY.contains(one),
            ["contact", one] => CONTACT.contains(one),
            ["address", rest @ ..] => address(rest),
            _ => false,
        },
        ["delivery", "date" | "party_name"] => true,
        ["delivery", "address", rest @ ..] => address(rest),
        ["payment", one] => PAYMENT.contains(one),
        ["totals", one] => TOTALS.contains(one),
        [list @ ("lines" | "vat_breakdown" | "allowances_charges"), index, one] => index.parse::<usize>().is_ok() && row_names(list).contains(one),
        _ => false,
    }
}

struct Ctx<'a> {
    t: &'a Template,
    invoice: &'a Invoice,
    tree: Node<'a>,
    regular: &'a Font,
    bold: &'a Font,
    warnings: BTreeSet<String>,
    currency: Option<String>,
}

impl<'a> Ctx<'a> {
    fn font(&self, face: Face) -> &Font {
        match face {
            Face::Regular => self.regular,
            Face::Bold => self.bold,
        }
    }

    fn warn(&mut self, w: String) {
        if self.warnings.len() < 200 {
            self.warnings.insert(w);
        }
    }

    fn document_name(&self) -> String {
        let credit = self.invoice.document == "Credit note" || self.invoice.type_code.as_ref().is_some_and(|t| t.text == "381");
        match (self.t.lang, credit) {
            (Lang::De, false) => "Rechnung",
            (Lang::De, true) => "Gutschrift",
            (Lang::En, false) => "Invoice",
            (Lang::En, true) => "Credit note",
        }
        .to_owned()
    }

    fn find<'n>(&'n self, path: &str, row: Option<(&str, &'n Node<'a>)>, page: Option<(usize, usize)>) -> Found<'n> {
        match (path, page) {
            ("page", Some((p, _))) => return Found::Text(p.to_string()),
            ("pages", Some((_, n))) => return Found::Text(n.to_string()),
            ("page" | "pages", None) => return Found::Unknown,
            ("document_name", _) => return Found::Text(self.document_name()),
            _ => {}
        }
        if let Some((list, node)) = row {
            if list == "notes" && path == "text" {
                return match node {
                    Node::Field(f) => Found::Fields(vec![f]),
                    _ => Found::Absent,
                };
            }
            if row_names(list).contains(&path) {
                return match node.get(path) {
                    Some(Node::Field(f)) => Found::Fields(vec![f]),
                    _ => Found::Absent,
                };
            }
        }
        if !known(path, row.map(|r| r.0)) {
            return Found::Unknown;
        }
        match self.tree.get(path) {
            Some(Node::Field(f)) => Found::Fields(vec![f]),
            Some(Node::List(items)) => {
                let fields: Vec<&Field> = items.iter().filter_map(|n| if let Node::Field(f) = n { Some(*f) } else { None }).collect();
                match (fields.is_empty(), items.is_empty()) {
                    (false, _) => Found::Fields(fields),
                    // A list of rows (lines, the VAT breakdown) is there when it has a row; it has no text of its own.
                    (true, false) => Found::Text(String::new()),
                    (true, true) => Found::Absent,
                }
            }
            _ => Found::Absent,
        }
    }

    /// Whether the model has a value at `path` (for `if`).
    fn present(&mut self, path: &str, row: Option<(&str, &Node<'a>)>) -> bool {
        match self.find(path, row, None) {
            Found::Fields(_) | Found::Text(_) => true,
            Found::Absent => false,
            Found::Unknown => {
                self.warn(format!("if=\"{path}\" names nothing of the invoice model; the block is hidden"));
                false
            }
        }
    }

    /// The text with its placeholders filled; `None` when it has placeholders and every one of them was empty.
    fn fill(&mut self, text: &Text, row: Option<(&str, &Node<'a>)>, page: Option<(usize, usize)>) -> Option<String> {
        let mut out = String::new();
        let mut placeholders = 0usize;
        let mut filled = 0usize;
        for piece in &text.pieces {
            match piece {
                Piece::Text(s) => out.push_str(s),
                Piece::Value { path, filter, .. } => {
                    placeholders = placeholders.saturating_add(1);
                    let value = match self.find(path, row, page) {
                        Found::Fields(fields) => {
                            let lang = self.t.lang;
                            let currency = self.currency.as_deref();
                            fields.iter().map(|f| apply(*filter, f, lang, currency)).collect::<Vec<_>>().join("\n")
                        }
                        Found::Text(s) => match filter {
                            Some(Filter::Upper) => s.to_uppercase(),
                            _ => s,
                        },
                        Found::Absent => String::new(),
                        Found::Unknown => {
                            let place = if page.is_none() && (path == "page" || path == "pages") {
                                " (page numbers are known in the header and the footer only)"
                            } else {
                                ""
                            };
                            self.warn(format!("{{{path}}} names nothing of the invoice model and is left empty{place}"));
                            String::new()
                        }
                    };
                    if !value.trim().is_empty() {
                        filled = filled.saturating_add(1);
                    }
                    out.push_str(&value);
                }
            }
        }
        if placeholders > 0 && filled == 0 {
            return None;
        }
        // Separators around missing values (`a ·  · b`) close up.
        if out.contains(" · ") || out.starts_with("· ") || out.ends_with(" ·") {
            out = out
                .split('\n')
                .map(|line| line.split('·').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · "))
                .collect::<Vec<_>>()
                .join("\n");
        }
        Some(out)
    }

    /// `text` broken into lines that fit `width` at `size` in `face`.
    fn wrap(&mut self, text: &str, size: f64, face: Face, width: f64) -> Vec<String> {
        let missing = self.font(face).missing(text);
        if !missing.is_empty() {
            let list: String = missing.iter().take(10).map(|c| format!("{c} (U+{:04X})", u32::from(*c))).collect::<Vec<_>>().join(", ");
            self.warn(format!("the embedded Inter face has no glyph for {list}; it is drawn as a box"));
        }
        let font = self.font(face);
        let mut lines = Vec::new();
        let mut cut = false;
        'paragraphs: for paragraph in text.split('\n') {
            let mut line = String::new();
            for word in paragraph.split(' ') {
                let candidate = if line.is_empty() { word.to_owned() } else { format!("{line} {word}") };
                if font.width(&candidate, size) <= width || line.is_empty() && font.width(word, size) <= width {
                    line = candidate;
                    continue;
                }
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                // A word wider than the box is broken between characters.
                for c in word.chars() {
                    let mut next = line.clone();
                    next.push(c);
                    if !line.is_empty() && font.width(&next, size) > width {
                        lines.push(std::mem::take(&mut line));
                        next = c.to_string();
                    }
                    line = next;
                    if lines.len() >= MAX_LINES_PER_TEXT {
                        cut = true;
                        break 'paragraphs;
                    }
                }
            }
            lines.push(line);
            if lines.len() >= MAX_LINES_PER_TEXT {
                cut = lines.len() > MAX_LINES_PER_TEXT || cut;
                break;
            }
        }
        lines.truncate(MAX_LINES_PER_TEXT);
        if cut {
            self.warn(format!("a text was cut at {MAX_LINES_PER_TEXT} lines"));
        }
        lines
    }

    fn x_for(&self, line: &str, size: f64, face: Face, x: f64, width: f64, align: Align) -> f64 {
        let w = self.font(face).width(line, size);
        match align {
            Align::Left => x,
            Align::Center => x + (width - w).max(0.0) / 2.0,
            Align::Right => x + (width - w).max(0.0),
        }
    }

    /// The lines of a text, one fragment each.
    fn text_frags(&mut self, text: &str, s: Style, align: Align, x: f64, width: f64) -> Vec<Frag> {
        let (size, face, gray, before, after) = style(s, self.t.font_size);
        let lh = line_height(size);
        let lines = self.wrap(text, size, face, width);
        let count = lines.len();
        lines
            .into_iter()
            .enumerate()
            .map(|(i, line)| {
                let top_pad = if i == 0 { before } else { 0.0 };
                let bottom_pad = if i + 1 == count { after } else { 0.0 };
                let lx = self.x_for(&line, size, face, x, width, align);
                Frag {
                    height: top_pad + lh + bottom_pad,
                    items: vec![Item::Text { x: lx, y: top_pad + size, size, face, gray, text: line }],
                    keep_with_next: matches!(s, Style::Title | Style::Heading) || i + 1 < count && count <= 3,
                    ..Frag::default()
                }
            })
            .collect()
    }

    fn blocks(&mut self, blocks: &[Block], x: f64, width: f64, page: Option<(usize, usize)>) -> Vec<Frag> {
        let mut out = Vec::new();
        for b in blocks {
            if let Some(c) = &b.cond
                && !self.present(c, None)
            {
                continue;
            }
            let mut frags = self.block(b, x, width, page);
            if let (Some(top), Some(first)) = (b.top, frags.first_mut()) {
                first.top = Some(top);
            }
            out.append(&mut frags);
        }
        out
    }

    fn block(&mut self, b: &Block, x: f64, width: f64, page: Option<(usize, usize)>) -> Vec<Frag> {
        match &b.kind {
            Kind::Text { style: s, align, text } => {
                if text.pieces.is_empty() {
                    // An empty block is a blank line.
                    return vec![Frag { height: line_height(self.t.font_size), ..Frag::default() }];
                }
                match self.fill(text, None, page) {
                    Some(filled) if !filled.trim().is_empty() => self.text_frags(&filled, *s, *align, x, width),
                    _ => Vec::new(),
                }
            }
            Kind::Columns { widths, columns } => self.columns(widths, columns, x, width, page),
            Kind::Fields { items } => self.fields(items, x, width),
            Kind::Table { rows, widths, cols, .. } => self.table(rows, widths, cols, x, width),
            Kind::Totals => self.totals(x, width),
        }
    }

    /// Columns side by side, as one fragment; when that is taller than a page, one after the other.
    fn columns(&mut self, widths: &[f64], columns: &[Vec<Block>], x: f64, width: f64, page: Option<(usize, usize)>) -> Vec<Frag> {
        let mut laid = Vec::new();
        let mut cx = x;
        for (i, (w, blocks)) in widths.iter().zip(columns).enumerate() {
            // A gap after every column but the last.
            let cw = (w * width - if i + 1 < columns.len() { 5.0 * MM } else { 0.0 }).max(10.0);
            laid.push(self.blocks(blocks, cx, cw, page));
            cx += w * width;
        }
        let heights: Vec<f64> = laid.iter().map(|frags| frags.iter().map(|f| f.height).sum()).collect();
        let height = heights.iter().copied().fold(0.0, f64::max);
        if height > self.body_height() {
            return laid.into_iter().flatten().collect();
        }
        let mut items = Vec::new();
        for frags in laid {
            let mut y = 0.0;
            for f in frags {
                items.extend(shift(&f.items, 0.0, y));
                y += f.height;
            }
        }
        vec![Frag { height, items, ..Frag::default() }]
    }

    /// The height a page has for the body (a generous bound used before the header and footer are known).
    fn body_height(&self) -> f64 {
        self.t.page.1 - 2.0 * self.t.margin - 20.0 * MM
    }

    fn fields(&mut self, items: &[crate::template::FieldItem], x: f64, width: f64) -> Vec<Frag> {
        let size = self.t.font_size;
        let lh = line_height(size);
        // The labels are as wide as the widest of them (with a gap), but no more than half the width.
        let mut widest: f64 = 0.0;
        for item in items {
            if let Some(label) = self.fill(&item.label, None, None) {
                widest = widest.max(self.regular.width(&label, size));
            }
        }
        let label_w = (widest + 3.0 * MM).min(width * 0.5);
        let value_w = width - label_w;
        let mut out = Vec::new();
        for item in items {
            if let Some(c) = &item.cond
                && !self.present(c, None)
            {
                continue;
            }
            let Some(value) = self.fill(&item.value, None, None).filter(|v| !v.trim().is_empty()) else { continue };
            let label = self.fill(&item.label, None, None).unwrap_or_default();
            let labels = self.wrap(&label, size, Face::Regular, label_w - PAD);
            let values = self.wrap(&value, size, Face::Regular, value_w);
            let mut items = Vec::new();
            for (i, l) in labels.iter().enumerate() {
                items.push(Item::Text { x, y: size + lh * i as f64, size, face: Face::Regular, gray: 0.35, text: l.clone() });
            }
            for (i, v) in values.iter().enumerate() {
                items.push(Item::Text { x: x + label_w, y: size + lh * i as f64, size, face: Face::Regular, gray: 0.0, text: v.clone() });
            }
            let n = labels.len().max(values.len()).max(1);
            out.push(Frag { height: lh * n as f64, items, ..Frag::default() });
        }
        out
    }

    fn rows(&self, list: &str) -> Vec<&Node<'a>> {
        match self.tree.get(list) {
            Some(Node::List(items)) => items.iter().collect(),
            _ => Vec::new(),
        }
    }

    fn table(&mut self, list: &str, widths: &[f64], cols: &[Col], x: f64, width: f64) -> Vec<Frag> {
        let rows: Vec<Node<'a>> = self.rows(list).into_iter().cloned().collect();
        if rows.is_empty() {
            return Vec::new();
        }
        let size = self.t.font_size * 0.9;
        let small = self.t.font_size * 0.75;
        let lh = line_height(size);
        let lhs = line_height(small);
        let xs: Vec<(f64, f64)> = {
            let mut at = x;
            widths
                .iter()
                .map(|w| {
                    let cw = w * width;
                    let r = (at + PAD, (cw - 2.0 * PAD).max(4.0));
                    at += cw;
                    r
                })
                .collect()
        };
        let right = x + widths.iter().sum::<f64>() * width;
        // The header row.
        let mut head_items = Vec::new();
        let mut head_lines = 1usize;
        for (col, (cx, cw)) in cols.iter().zip(&xs) {
            let title = self.fill(&col.title, None, None).unwrap_or_default();
            let lines = self.wrap(&title, size, Face::Bold, *cw);
            head_lines = head_lines.max(lines.len());
            for (i, l) in lines.into_iter().enumerate() {
                let lx = self.x_for(&l, size, Face::Bold, *cx, *cw, col.align);
                head_items.push(Item::Text { x: lx, y: PAD + size + lh * i as f64, size, face: Face::Bold, gray: 0.0, text: l });
            }
        }
        let head_h = PAD * 2.0 + lh * head_lines as f64;
        head_items.push(Item::Rule { x1: x, y1: head_h, x2: right, y2: head_h, width: 0.8, gray: 0.0 });
        let header = Frag { height: head_h, items: head_items, keep_with_next: true, ..Frag::default() };
        let mut out = vec![header.clone()];
        let max_row = self.body_height() - head_h - lh;
        for row in &rows {
            let mut items = Vec::new();
            let mut height: f64 = 0.0;
            for (col, (cx, cw)) in cols.iter().zip(&xs) {
                let value = self.fill(&col.value, Some((list, row)), None).unwrap_or_default();
                let lines = self.wrap(value.trim(), size, Face::Regular, *cw);
                let mut y = PAD;
                for l in lines {
                    let lx = self.x_for(&l, size, Face::Regular, *cx, *cw, col.align);
                    items.push(Item::Text { x: lx, y: y + size, size, face: Face::Regular, gray: 0.0, text: l });
                    y += lh;
                }
                if let Some(sub) = &col.sub
                    && let Some(sub) = self.fill(sub, Some((list, row)), None).filter(|s| !s.trim().is_empty())
                {
                    for l in self.wrap(sub.trim(), small, Face::Regular, *cw) {
                        let lx = self.x_for(&l, small, Face::Regular, *cx, *cw, col.align);
                        items.push(Item::Text { x: lx, y: y + small, size: small, face: Face::Regular, gray: 0.35, text: l });
                        y += lhs;
                    }
                }
                height = height.max(y + PAD);
            }
            if height > max_row {
                // A row taller than a page is cut at the page.
                items.retain(|i| matches!(i, Item::Text { y, .. } if *y <= max_row));
                height = max_row;
                self.warn("a table row is taller than a page and was cut".into());
            }
            items.push(Item::Rule { x1: x, y1: height, x2: right, y2: height, width: 0.3, gray: 0.75 });
            out.push(Frag { height, items, repeat: Some(Box::new(header.clone())), ..Frag::default() });
        }
        out
    }

    fn totals(&mut self, x: f64, width: f64) -> Vec<Frag> {
        let t = &self.invoice.totals;
        let labels: [(&str, &str, &Option<Field>); 9] = [
            ("Summe der Positionen", "Sum of lines", &t.line_total),
            ("Nachlässe", "Allowances", &t.allowance_total),
            ("Zuschläge", "Charges", &t.charge_total),
            ("Gesamtbetrag netto", "Total without VAT", &t.tax_exclusive),
            ("Umsatzsteuer", "VAT", &t.tax_total),
            ("Gesamtbetrag brutto", "Total with VAT", &t.tax_inclusive),
            ("Bereits gezahlt", "Paid", &t.prepaid),
            ("Rundung", "Rounding", &t.rounding),
            ("Zahlbetrag", "Amount due", &t.payable),
        ];
        let size = self.t.font_size;
        let lh = line_height(size);
        let lang = self.t.lang;
        let currency = self.currency.clone();
        let present: Vec<(String, String, bool)> = labels
            .iter()
            .enumerate()
            .filter_map(|(i, (de, en, f))| {
                let f = f.as_ref()?;
                let label = if lang == Lang::De { *de } else { *en };
                Some((label.to_owned(), apply(Some(Filter::Money), f, lang, currency.as_deref()), i == 8))
            })
            .collect();
        let mut out = Vec::new();
        for (label, amount, payable) in present {
            let face = if payable { Face::Bold } else { Face::Regular };
            let gap = if payable { 2.0 } else { 0.0 };
            let mut items = Vec::new();
            if payable {
                items.push(Item::Rule { x1: x, y1: 1.0, x2: x + width, y2: 1.0, width: 0.8, gray: 0.0 });
            }
            let label_lines = self.wrap(&label, size, face, width * 0.55);
            for (i, l) in label_lines.iter().enumerate() {
                items.push(Item::Text { x, y: gap + size + lh * i as f64, size, face, gray: 0.0, text: l.clone() });
            }
            let ax = self.x_for(&amount, size, face, x, width, Align::Right);
            items.push(Item::Text { x: ax, y: gap + size, size, face, gray: 0.0, text: amount });
            out.push(Frag { height: gap + lh * label_lines.len().max(1) as f64, items, ..Frag::default() });
        }
        out
    }
}

/// Place fragments on pages between `top` and `bottom`.
fn paginate(frags: Vec<Frag>, top: f64, bottom: f64, warnings: &mut BTreeSet<String>) -> Result<Vec<Page>, String> {
    let mut pages = vec![Page::default()];
    let mut y = top;
    let frags: Vec<Frag> = frags;
    for (i, f) in frags.iter().enumerate() {
        if let Some(t) = f.top
            && pages.len() == 1
            && t > y
        {
            if t + f.height <= bottom {
                y = t;
            } else {
                warnings.insert(format!("a block placed at {:.0} mm from the top does not fit there", t / MM));
            }
        }
        // A fragment kept with the next one needs room for both.
        let need = f.height + if f.keep_with_next { frags.get(i + 1).map_or(0.0, |n| n.height) } else { 0.0 };
        if y + need > bottom && y > top + 0.01 {
            if pages.len() >= MAX_PAGES {
                return Err(format!("the invoice needs more than {MAX_PAGES} pages"));
            }
            pages.push(Page::default());
            y = top;
            if let Some(header) = &f.repeat {
                if let Some(p) = pages.last_mut() {
                    p.items.extend(shift(&header.items, 0.0, y));
                }
                y += header.height;
            }
        }
        if let Some(p) = pages.last_mut() {
            p.items.extend(shift(&f.items, 0.0, y));
        }
        y += f.height;
    }
    Ok(pages)
}

/// Lay out `invoice` with `template`, measuring with the two faces.
pub fn lay_out(t: &Template, invoice: &Invoice, regular: &Font, bold: &Font) -> Result<Laid, String> {
    let mut ctx = Ctx {
        t,
        invoice,
        tree: invoice.tree(),
        regular,
        bold,
        warnings: BTreeSet::new(),
        currency: invoice.currency.as_ref().map(|c| c.text.clone()),
    };
    let width = t.page.0 - 2.0 * t.margin;
    let x = t.margin;
    // The header and footer with page numbers as wide as the largest they can be, for their heights.
    let probe = Some((MAX_PAGES, MAX_PAGES));
    let header_h: f64 = ctx.blocks(&t.header, x, width, probe).iter().map(|f| f.height).sum();
    let footer_h: f64 = ctx.blocks(&t.footer, x, width, probe).iter().map(|f| f.height).sum();
    let gap = 4.0 * MM;
    let top = t.margin + if header_h > 0.0 { header_h + gap } else { 0.0 };
    let bottom = t.page.1 - t.margin - if footer_h > 0.0 { footer_h + gap } else { 0.0 };
    if bottom - top < 72.0 {
        return Err("the header and the footer leave less than an inch for the body".into());
    }
    let body = ctx.blocks(&t.body, x, width, None);
    let mut warnings = std::mem::take(&mut ctx.warnings);
    let mut pages = paginate(body, top, bottom, &mut warnings)?;
    let n = pages.len();
    for (i, page) in pages.iter_mut().enumerate() {
        let numbers = Some((i + 1, n));
        let mut y = t.margin;
        for f in ctx.blocks(&t.header, x, width, numbers) {
            page.items.extend(shift(&f.items, 0.0, y));
            y += f.height;
        }
        let mut y = t.page.1 - t.margin - footer_h;
        for f in ctx.blocks(&t.footer, x, width, numbers) {
            page.items.extend(shift(&f.items, 0.0, y));
            y += f.height;
        }
    }
    warnings.append(&mut ctx.warnings);
    if invoice.lines_total > invoice.lines.len() {
        warnings.insert(format!("only the first {} of {} lines are shown", invoice.lines.len(), invoice.lines_total));
    }
    Ok(Laid { size: t.page, pages, warnings: warnings.into_iter().collect() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_paths_and_typos_are_told_apart() {
        for p in [
            "number",
            "seller.name",
            "buyer.address.city",
            "buyer.contact.email",
            "totals.payable",
            "payment.iban",
            "delivery.address.line1",
            "lines.3.name",
        ] {
            assert!(known(p, None), "{p}");
        }
        for p in ["nummer", "seller.nmae", "buyer.address.town", "totals", "lines.x.name", "seller.contact.fax"] {
            assert!(!known(p, None), "{p}");
        }
        assert!(known("net_price", Some("lines")) && !known("net_price", None) && known("rate", Some("vat_breakdown")));
    }
}
