//! A neutral invoice model: what a UBL `Invoice` or `CreditNote`, or a UN/CEFACT `CrossIndustryInvoice`, says, read into one shape
//! that does not depend on the syntax (the business terms of EN 16931, named in plain words). It is what the visual rendering of an
//! invoice (`einvoice_render_pdf`) and the `einvoice_model` tool read.
//!
//! - Every field is optional: a document that lacks a term has `None`, never an error, and a document that is no invoice has no model.
//! - Every field keeps the text the document has (references replaced, outer white space trimmed) and the byte range of that text in
//!   the source, so a view can point back at it. Amounts, quantities and rates also have an exact decimal ([`Decimal`]; never a
//!   float), and dates a calendar date when they are written as the syntax says (`2026-11-01` in UBL, `20261101` with format `102` in
//!   CII).
//! - Reading is bounded: at most [`MAX_LINES`] lines (the rest are counted), [`MAX_ROWS`] entries in the other lists, and
//!   [`MAX_FIELD_CHARS`] characters of one field (cut at a character boundary and said in [`Invoice::cut`]). It never panics.

use std::ops::Range;

use chrono::NaiveDate;
use xmlcraft_syntax::parse;
use xmlcraft_xsd::dom::{self, Dom, El};

use crate::{CII, CII_RAM, Syntax, UBL_CBC, UBL_CREDIT_NOTE, UBL_INVOICE};

const UBL_CAC: &str = "urn:oasis:names:specification:ubl:schema:xsd:CommonAggregateComponents-2";
const CII_UDT: &str = "urn:un:unece:uncefact:data:standard:UnqualifiedDataType:100";

/// The most invoice lines read (the others are counted in [`Invoice::lines_total`]).
pub const MAX_LINES: usize = 100_000;
/// The most entries of the other lists (notes, allowances and charges, VAT breakdown rows, address lines).
pub const MAX_ROWS: usize = 10_000;
/// The most characters of one field.
pub const MAX_FIELD_CHARS: usize = 10_000;
/// The most digits of a decimal (more is kept as text, without a decimal).
const MAX_DIGITS: usize = 36;

/// An exact decimal number: `mantissa` × 10^−`scale`. `19`, `19.00` and `-0.5` keep their own scale (the digits the document wrote).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decimal {
    pub mantissa: i128,
    pub scale: u32,
}

impl Decimal {
    /// An `xs:decimal` lexical value (`-12.50`, `+3`, `.5`); none for anything else (an exponent, a comma, too many digits).
    pub fn parse(text: &str) -> Option<Decimal> {
        let t = text.trim();
        let (negative, t) = match t.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, t.strip_prefix('+').unwrap_or(t)),
        };
        let (whole, fraction) = t.split_once('.').unwrap_or((t, ""));
        if (whole.is_empty() && fraction.is_empty()) || !whole.bytes().chain(fraction.bytes()).all(|b| b.is_ascii_digit()) {
            return None;
        }
        let whole = whole.trim_start_matches('0');
        if whole.len().saturating_add(fraction.len()) > MAX_DIGITS {
            return None;
        }
        let digits = format!("{whole}{fraction}");
        let magnitude: i128 = if digits.trim_start_matches('0').is_empty() { 0 } else { digits.parse().ok()? };
        let scale = u32::try_from(fraction.len()).ok()?;
        Some(Decimal { mantissa: if negative { magnitude.checked_neg()? } else { magnitude }, scale })
    }

    pub fn is_negative(self) -> bool {
        self.mantissa < 0
    }

    /// The digits before and after the decimal point of the magnitude (`-1234.50` is `("1234", "50")`).
    pub fn digits(self) -> (String, String) {
        let all = self.mantissa.unsigned_abs().to_string();
        let scale = usize::try_from(self.scale).unwrap_or(usize::MAX);
        if all.len() > scale {
            let at = all.len().saturating_sub(scale);
            // ASCII digits: every index is a character boundary.
            (all.get(..at).unwrap_or("0").to_owned(), all.get(at..).unwrap_or("").to_owned())
        } else {
            ("0".to_owned(), format!("{}{all}", "0".repeat(scale.saturating_sub(all.len()))))
        }
    }
}

/// One field: the text the document has for it and where that text is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    /// The text, references replaced and outer white space trimmed (at most [`MAX_FIELD_CHARS`] characters).
    pub text: String,
    /// The byte range of the text in the source (of the element when its text is not written literally).
    pub range: Range<usize>,
    /// The number, for an amount, a quantity or a rate written as an `xs:decimal`.
    pub decimal: Option<Decimal>,
    /// The date, for a date written the way its syntax says.
    pub date: Option<NaiveDate>,
    /// For an amount, the `currencyID` it is given in (the invoice currency when it names none); for a quantity, its `unitCode`; for an
    /// electronic address, its scheme.
    pub unit: Option<String>,
}

impl Field {
    /// What the field says in a neutral form: the ISO date for a date, the text otherwise.
    pub fn normalized(&self) -> String {
        match self.date {
            Some(d) => d.format("%Y-%m-%d").to_string(),
            None => self.text.clone(),
        }
    }
}

/// A postal address.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Address {
    /// The street lines, in order (UBL `StreetName`, `AdditionalStreetName`, `AddressLine`; CII `LineOne` to `LineThree`).
    pub lines: Vec<Field>,
    pub city: Option<Field>,
    pub postcode: Option<Field>,
    pub subdivision: Option<Field>,
    pub country: Option<Field>,
}

impl Address {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.city.is_none() && self.postcode.is_none() && self.subdivision.is_none() && self.country.is_none()
    }
}

/// A contact person or point.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Contact {
    pub name: Option<Field>,
    pub phone: Option<Field>,
    pub email: Option<Field>,
}

/// The seller, the buyer or the payee.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Party {
    /// The legal or registered name (BT-27, BT-44; the trading name when there is no other).
    pub name: Option<Field>,
    pub trading_name: Option<Field>,
    /// The party's identifier (BT-29, BT-46, BT-60).
    pub id: Option<Field>,
    pub address: Address,
    /// The VAT identifier (BT-31, BT-48).
    pub vat_id: Option<Field>,
    /// The tax registration identifier other than VAT (BT-32: `FC` in CII).
    pub tax_registration: Option<Field>,
    /// The legal registration identifier (BT-30, BT-47: a commercial register number).
    pub legal_id: Option<Field>,
    pub contact: Contact,
    /// The electronic address (BT-34, BT-49), with its scheme as `unit`.
    pub electronic_address: Option<Field>,
}

/// The delivery.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Delivery {
    pub date: Option<Field>,
    pub party_name: Option<Field>,
    pub address: Address,
}

/// How to pay (the first payment means of the document).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Payment {
    /// The payment means code (UNTDID 4461: `58` SEPA credit transfer, `59` SEPA direct debit, …).
    pub means_code: Option<Field>,
    /// The remittance information (BT-83).
    pub reference: Option<Field>,
    pub iban: Option<Field>,
    pub bic: Option<Field>,
    pub account_name: Option<Field>,
    /// The direct debit mandate reference (BT-89).
    pub mandate: Option<Field>,
}

/// A document-level allowance or charge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllowanceCharge {
    /// `true` for a charge, `false` for an allowance.
    pub charge: bool,
    /// The charge indicator as written (`true` or `false`).
    pub indicator: Option<Field>,
    pub amount: Option<Field>,
    pub base: Option<Field>,
    pub percent: Option<Field>,
    pub reason: Option<Field>,
    pub vat_category: Option<Field>,
    pub vat_rate: Option<Field>,
}

/// One row of the VAT breakdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VatRow {
    pub category: Option<Field>,
    pub rate: Option<Field>,
    pub taxable: Option<Field>,
    pub tax: Option<Field>,
    pub exemption_reason: Option<Field>,
}

/// The document totals.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    /// The sum of the line net amounts (BT-106).
    pub line_total: Option<Field>,
    pub allowance_total: Option<Field>,
    pub charge_total: Option<Field>,
    /// The total without VAT (BT-109).
    pub tax_exclusive: Option<Field>,
    /// The VAT total in the invoice currency (BT-110).
    pub tax_total: Option<Field>,
    /// The total with VAT (BT-112).
    pub tax_inclusive: Option<Field>,
    pub prepaid: Option<Field>,
    pub rounding: Option<Field>,
    /// The amount due for payment (BT-115).
    pub payable: Option<Field>,
}

/// An invoice line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub id: Option<Field>,
    pub name: Option<Field>,
    pub description: Option<Field>,
    pub note: Option<Field>,
    /// The quantity, with its unit code as `unit` (and in [`Line::unit`]).
    pub quantity: Option<Field>,
    /// The unit code of the quantity (UN/ECE Recommendation 20: `C62` one, `HUR` hour).
    pub unit: Option<Field>,
    pub net_price: Option<Field>,
    pub line_total: Option<Field>,
    pub vat_category: Option<Field>,
    pub vat_rate: Option<Field>,
    pub seller_item_id: Option<Field>,
    pub period_start: Option<Field>,
    pub period_end: Option<Field>,
}

/// What an invoice says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invoice {
    pub syntax: Syntax,
    /// `Invoice` or `Credit note` (the root element).
    pub document: String,
    pub number: Option<Field>,
    /// The type code (UNTDID 1001: `380` invoice, `381` credit note, …).
    pub type_code: Option<Field>,
    pub issue_date: Option<Field>,
    pub due_date: Option<Field>,
    pub currency: Option<Field>,
    /// The buyer reference (BT-10; the Leitweg-ID of XRechnung).
    pub buyer_reference: Option<Field>,
    pub order_reference: Option<Field>,
    pub notes: Vec<Field>,
    pub seller: Party,
    pub buyer: Party,
    pub payee: Option<Party>,
    pub delivery: Delivery,
    pub payment: Payment,
    pub payment_terms: Option<Field>,
    pub allowances_charges: Vec<AllowanceCharge>,
    pub vat_breakdown: Vec<VatRow>,
    pub totals: Totals,
    pub lines: Vec<Line>,
    /// How many lines the document has (more than `lines.len()` when it has more than [`MAX_LINES`]).
    pub lines_total: usize,
    /// What reading had to leave out or cut, in words (a list cut at its limit, a field cut at [`MAX_FIELD_CHARS`], a tree cut by the
    /// reader's limits).
    pub cut: Vec<String>,
}

/// One value of the model, addressed by a dotted path (`seller.address.city`; [`Invoice::lookup`]).
#[derive(Clone, Debug, PartialEq)]
pub enum Node<'a> {
    Field(&'a Field),
    Group(Vec<(&'static str, Node<'a>)>),
    List(Vec<Node<'a>>),
}

impl<'a> Node<'a> {
    /// The value at `path` below this node (`a.b.c`); `None` when nothing is there.
    pub fn get(&self, path: &str) -> Option<&Node<'a>> {
        let mut node = self;
        for part in path.split('.').filter(|p| !p.is_empty()) {
            node = match node {
                Node::Group(children) => children.iter().find(|(name, _)| *name == part).map(|(_, n)| n)?,
                Node::List(items) => items.get(part.parse::<usize>().ok()?.checked_sub(1)?)?,
                Node::Field(_) => return None,
            };
        }
        Some(node)
    }

    /// The fields below this node, with their dotted paths (list items numbered from 1).
    pub fn flatten(&self, prefix: &str, out: &mut Vec<(String, &'a Field)>) {
        let join = |name: &str| if prefix.is_empty() { name.to_owned() } else { format!("{prefix}.{name}") };
        match self {
            Node::Field(f) => out.push((prefix.to_owned(), f)),
            Node::Group(children) => children.iter().for_each(|(name, n)| n.flatten(&join(name), out)),
            Node::List(items) => items.iter().enumerate().for_each(|(i, n)| n.flatten(&join(&i.saturating_add(1).to_string()), out)),
        }
    }
}

fn opt<'a>(out: &mut Vec<(&'static str, Node<'a>)>, name: &'static str, f: &'a Option<Field>) {
    if let Some(f) = f {
        out.push((name, Node::Field(f)));
    }
}

fn address_node(a: &Address) -> Node<'_> {
    let mut g = Vec::new();
    g.push(("lines", Node::List(a.lines.iter().map(Node::Field).collect())));
    for (name, line) in ["line1", "line2", "line3"].into_iter().zip(a.lines.iter()) {
        g.push((name, Node::Field(line)));
    }
    opt(&mut g, "city", &a.city);
    opt(&mut g, "postcode", &a.postcode);
    opt(&mut g, "subdivision", &a.subdivision);
    opt(&mut g, "country", &a.country);
    Node::Group(g)
}

fn party_node<'a>(p: &'a Party, reference: &'a Option<Field>) -> Node<'a> {
    let mut g = Vec::new();
    opt(&mut g, "name", &p.name);
    opt(&mut g, "trading_name", &p.trading_name);
    opt(&mut g, "id", &p.id);
    g.push(("address", address_node(&p.address)));
    opt(&mut g, "vat_id", &p.vat_id);
    opt(&mut g, "tax_registration", &p.tax_registration);
    opt(&mut g, "legal_id", &p.legal_id);
    let mut c = Vec::new();
    opt(&mut c, "name", &p.contact.name);
    opt(&mut c, "phone", &p.contact.phone);
    opt(&mut c, "email", &p.contact.email);
    g.push(("contact", Node::Group(c)));
    opt(&mut g, "electronic_address", &p.electronic_address);
    // The buyer reference is the buyer's: `buyer.reference` names it too.
    opt(&mut g, "reference", reference);
    Node::Group(g)
}

const NONE: Option<Field> = None;

impl Invoice {
    /// The model as a tree of named values: the names [`Invoice::lookup`] and the templates use.
    pub fn tree(&self) -> Node<'_> {
        let mut g = Vec::new();
        opt(&mut g, "number", &self.number);
        opt(&mut g, "type_code", &self.type_code);
        opt(&mut g, "issue_date", &self.issue_date);
        opt(&mut g, "due_date", &self.due_date);
        opt(&mut g, "currency", &self.currency);
        opt(&mut g, "buyer_reference", &self.buyer_reference);
        opt(&mut g, "order_reference", &self.order_reference);
        g.push(("notes", Node::List(self.notes.iter().map(Node::Field).collect())));
        g.push(("seller", party_node(&self.seller, &NONE)));
        g.push(("buyer", party_node(&self.buyer, &self.buyer_reference)));
        if let Some(p) = &self.payee {
            g.push(("payee", party_node(p, &NONE)));
        }
        let mut d = Vec::new();
        opt(&mut d, "date", &self.delivery.date);
        opt(&mut d, "party_name", &self.delivery.party_name);
        d.push(("address", address_node(&self.delivery.address)));
        g.push(("delivery", Node::Group(d)));
        let p = &self.payment;
        let mut pay = Vec::new();
        opt(&mut pay, "means_code", &p.means_code);
        opt(&mut pay, "reference", &p.reference);
        opt(&mut pay, "iban", &p.iban);
        opt(&mut pay, "bic", &p.bic);
        opt(&mut pay, "account_name", &p.account_name);
        opt(&mut pay, "mandate", &p.mandate);
        g.push(("payment", Node::Group(pay)));
        opt(&mut g, "payment_terms", &self.payment_terms);
        let allowances = self
            .allowances_charges
            .iter()
            .map(|a| {
                let mut r = Vec::new();
                opt(&mut r, "amount", &a.amount);
                opt(&mut r, "base", &a.base);
                opt(&mut r, "percent", &a.percent);
                opt(&mut r, "reason", &a.reason);
                opt(&mut r, "vat_category", &a.vat_category);
                opt(&mut r, "vat_rate", &a.vat_rate);
                opt(&mut r, "charge_indicator", &a.indicator);
                Node::Group(r)
            })
            .collect();
        g.push(("allowances_charges", Node::List(allowances)));
        let vat = self
            .vat_breakdown
            .iter()
            .map(|v| {
                let mut r = Vec::new();
                opt(&mut r, "category", &v.category);
                opt(&mut r, "rate", &v.rate);
                opt(&mut r, "taxable", &v.taxable);
                opt(&mut r, "tax", &v.tax);
                opt(&mut r, "exemption_reason", &v.exemption_reason);
                Node::Group(r)
            })
            .collect();
        g.push(("vat_breakdown", Node::List(vat)));
        let t = &self.totals;
        let mut tot = Vec::new();
        opt(&mut tot, "line_total", &t.line_total);
        opt(&mut tot, "allowance_total", &t.allowance_total);
        opt(&mut tot, "charge_total", &t.charge_total);
        opt(&mut tot, "tax_exclusive", &t.tax_exclusive);
        opt(&mut tot, "tax_total", &t.tax_total);
        opt(&mut tot, "tax_inclusive", &t.tax_inclusive);
        opt(&mut tot, "prepaid", &t.prepaid);
        opt(&mut tot, "rounding", &t.rounding);
        opt(&mut tot, "payable", &t.payable);
        g.push(("totals", Node::Group(tot)));
        let lines = self
            .lines
            .iter()
            .map(|l| {
                let mut r = Vec::new();
                opt(&mut r, "id", &l.id);
                opt(&mut r, "name", &l.name);
                opt(&mut r, "description", &l.description);
                opt(&mut r, "note", &l.note);
                opt(&mut r, "quantity", &l.quantity);
                opt(&mut r, "unit", &l.unit);
                opt(&mut r, "net_price", &l.net_price);
                opt(&mut r, "line_total", &l.line_total);
                opt(&mut r, "vat_category", &l.vat_category);
                opt(&mut r, "vat_rate", &l.vat_rate);
                opt(&mut r, "seller_item_id", &l.seller_item_id);
                opt(&mut r, "period_start", &l.period_start);
                opt(&mut r, "period_end", &l.period_end);
                Node::Group(r)
            })
            .collect();
        g.push(("lines", Node::List(lines)));
        Node::Group(g)
    }

    /// The field at a dotted path (`seller.address.city`, `lines.1.name`), when there is one.
    pub fn lookup(&self, path: &str) -> Option<Field> {
        match self.tree().get(path)? {
            Node::Field(f) => Some((*f).clone()),
            _ => None,
        }
    }
}

/// What kind of value a field holds, which decides what is parsed from its text.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    /// An amount: a decimal and its `currencyID`.
    Amount,
    /// A quantity: a decimal and its `unitCode`.
    Quantity,
    /// A rate or another plain number.
    Number,
    /// A UBL date (`xs:date`), or a CII `DateTimeString` (format `102`: `YYYYMMDD`).
    Date,
}

struct Reader<'a> {
    source: &'a str,
    tree: &'a Dom,
    cut: Vec<String>,
    /// The invoice currency, once read: the currency of an amount that names none (CII writes `currencyID` on the VAT total only).
    currency: Option<String>,
}

impl<'a> Reader<'a> {
    fn children(&self, el: &'a El) -> impl Iterator<Item = &'a El> + 'a {
        let tree = self.tree;
        el.children.iter().filter_map(move |i| tree.elements.get(*i))
    }

    /// The children of `el` that are `namespace`:`local`.
    fn all(&self, el: &'a El, namespace: &'static str, local: &'static str) -> impl Iterator<Item = &'a El> + 'a {
        self.children(el).filter(move |c| c.local == local && c.namespace.as_deref() == Some(namespace))
    }

    fn one(&self, el: &'a El, namespace: &'static str, local: &'static str) -> Option<&'a El> {
        self.all(el, namespace, local).next()
    }

    /// The element at a path of `(namespace, local)` steps below `el` (the first match of each step).
    fn path(&self, el: &'a El, steps: &[(&'static str, &'static str)]) -> Option<&'a El> {
        steps.iter().try_fold(el, |at, (ns, local)| self.one(at, ns, local))
    }

    /// The byte range of the trimmed text content of `el`, when its content is written as plain text.
    fn content_range(&self, el: &El) -> Range<usize> {
        let whole = el.range.clone();
        let Some(text) = self.source.get(whole.clone()) else { return whole };
        // The start tag ends at the first `>` after its name and its last attribute value (a value may hold `>`).
        let after = el.attrs.iter().map(|a| a.value_range.end).chain(std::iter::once(el.name_range.end)).max().unwrap_or(whole.start);
        let from = after.saturating_sub(whole.start);
        let Some(close) = text.get(from..).and_then(|t| t.find('>')).and_then(|i| from.checked_add(i)) else { return whole };
        if text.get(..close).is_some_and(|t| t.ends_with('/')) {
            return whole;
        }
        let open_end = close.saturating_add(1);
        let end_tag = text.rfind("</").filter(|e| *e >= open_end).unwrap_or(text.len());
        let Some(inner) = text.get(open_end..end_tag) else { return whole };
        let lead = inner.len().saturating_sub(inner.trim_start().len());
        let trimmed = inner.trim();
        let start = whole.start.saturating_add(open_end).saturating_add(lead);
        start..start.saturating_add(trimmed.len())
    }

    fn field(&mut self, el: Option<&'a El>, kind: Kind) -> Option<Field> {
        let el = el?;
        let raw = el.text.trim();
        if raw.is_empty() {
            return None;
        }
        let text = match raw.char_indices().nth(MAX_FIELD_CHARS) {
            Some((at, _)) => {
                self.note(format!("the text of {} was cut at {MAX_FIELD_CHARS} characters", el.qname));
                raw.get(..at).unwrap_or_default().to_owned()
            }
            None => raw.to_owned(),
        };
        let attr = |name: &str| el.attrs.iter().find(|a| a.local == name && a.namespace.is_none()).map(|a| a.value.trim().to_owned());
        let (decimal, date, unit) = match kind {
            Kind::Text => (None, None, None),
            Kind::Amount => (Decimal::parse(&text), None, attr("currencyID").filter(|c| !c.is_empty()).or_else(|| self.currency.clone())),
            Kind::Quantity => (Decimal::parse(&text), None, attr("unitCode")),
            Kind::Number => (Decimal::parse(&text), None, None),
            Kind::Date => {
                let format = attr("format");
                let date = match format.as_deref() {
                    Some("102") => NaiveDate::parse_from_str(&text, "%Y%m%d").ok(),
                    Some(_) => None,
                    None => NaiveDate::parse_from_str(&text, "%Y-%m-%d").ok(),
                };
                (None, date, None)
            }
        };
        // The place of the text itself when it is written literally (no references, no CDATA, no comments), else of the element.
        let content = self.content_range(el);
        let range = if self.source.get(content.clone()) == Some(raw) { content } else { el.range.clone() };
        Some(Field { text, range, decimal, date, unit })
    }

    fn note(&mut self, what: String) {
        if self.cut.len() < 100 && !self.cut.contains(&what) {
            self.cut.push(what);
        }
    }

    /// Up to `max` items, noting the rest.
    fn bounded<T>(&mut self, items: impl Iterator<Item = T>, max: usize, what: &str) -> Vec<T> {
        let mut out = Vec::new();
        let mut more = 0usize;
        for item in items {
            if out.len() < max {
                out.push(item);
            } else {
                more = more.saturating_add(1);
            }
        }
        if more > 0 {
            self.note(format!("{more} more {what} than the {max} read"));
        }
        out
    }
}

/// Read the invoice model of `source`, or none when it is not a UBL Invoice or CreditNote or a CII CrossIndustryInvoice.
pub fn read(source: &str) -> Option<Invoice> {
    let parsed = parse(source);
    let tree = dom::build(&parsed.syntax());
    let root = tree.root().and_then(|i| tree.elements.get(i)).or_else(|| tree.elements.first())?;
    let mut r = Reader { source, tree: &tree, cut: Vec::new(), currency: None };
    if let Some(why) = tree.truncated {
        r.note(format!("the document was read only in part ({why})"));
    }
    let namespace = root.namespace.as_deref().unwrap_or("");
    let invoice = match (namespace, root.local.as_str()) {
        (UBL_INVOICE, "Invoice") => ubl(&mut r, root, false),
        (UBL_CREDIT_NOTE, "CreditNote") => ubl(&mut r, root, true),
        (CII, "CrossIndustryInvoice") => cii(&mut r, root),
        _ => return None,
    };
    Some(Invoice { cut: r.cut, ..invoice })
}

fn empty(syntax: Syntax, document: &str) -> Invoice {
    Invoice {
        syntax,
        document: document.to_owned(),
        number: None,
        type_code: None,
        issue_date: None,
        due_date: None,
        currency: None,
        buyer_reference: None,
        order_reference: None,
        notes: Vec::new(),
        seller: Party::default(),
        buyer: Party::default(),
        payee: None,
        delivery: Delivery::default(),
        payment: Payment::default(),
        payment_terms: None,
        allowances_charges: Vec::new(),
        vat_breakdown: Vec::new(),
        totals: Totals::default(),
        lines: Vec::new(),
        lines_total: 0,
        cut: Vec::new(),
    }
}

// ---- UBL 2.1 (the EN 16931 UBL syntax binding) ----

const CBC: &str = UBL_CBC;
const CAC: &str = UBL_CAC;

fn ubl<'a>(r: &mut Reader<'a>, root: &'a El, credit_note: bool) -> Invoice {
    let mut inv = empty(Syntax::Ubl, if credit_note { "Credit note" } else { "Invoice" });
    inv.number = r.field(r.one(root, CBC, "ID"), Kind::Text);
    inv.type_code = r.field(r.one(root, CBC, if credit_note { "CreditNoteTypeCode" } else { "InvoiceTypeCode" }), Kind::Text);
    inv.issue_date = r.field(r.one(root, CBC, "IssueDate"), Kind::Date);
    inv.currency = r.field(r.one(root, CBC, "DocumentCurrencyCode"), Kind::Text);
    r.currency = inv.currency.as_ref().map(|c| c.text.clone());
    inv.buyer_reference = r.field(r.one(root, CBC, "BuyerReference"), Kind::Text);
    inv.order_reference = r.field(r.path(root, &[(CAC, "OrderReference"), (CBC, "ID")]), Kind::Text);
    let notes: Vec<&El> = r.all(root, CBC, "Note").collect();
    inv.notes = r.bounded(notes.into_iter(), MAX_ROWS, "notes").into_iter().filter_map(|n| r.field(Some(n), Kind::Text)).collect();
    if let Some(p) = r.path(root, &[(CAC, "AccountingSupplierParty"), (CAC, "Party")]) {
        inv.seller = ubl_party(r, p);
    }
    if let Some(p) = r.path(root, &[(CAC, "AccountingCustomerParty"), (CAC, "Party")]) {
        inv.buyer = ubl_party(r, p);
    }
    inv.payee = r.one(root, CAC, "PayeeParty").map(|p| ubl_party(r, p));
    if let Some(d) = r.one(root, CAC, "Delivery") {
        inv.delivery.date = r.field(r.one(d, CBC, "ActualDeliveryDate"), Kind::Date);
        inv.delivery.party_name = r.field(r.path(d, &[(CAC, "DeliveryParty"), (CAC, "PartyName"), (CBC, "Name")]), Kind::Text);
        if let Some(a) = r.path(d, &[(CAC, "DeliveryLocation"), (CAC, "Address")]) {
            inv.delivery.address = ubl_address(r, a);
        }
    }
    let means = r.one(root, CAC, "PaymentMeans");
    if let Some(m) = means {
        let p = &mut inv.payment;
        p.means_code = r.field(r.one(m, CBC, "PaymentMeansCode"), Kind::Text);
        p.reference = r.field(r.one(m, CBC, "PaymentID"), Kind::Text);
        let account = r.one(m, CAC, "PayeeFinancialAccount");
        p.iban = r.field(account.and_then(|a| r.one(a, CBC, "ID")), Kind::Text);
        p.account_name = r.field(account.and_then(|a| r.one(a, CBC, "Name")), Kind::Text);
        p.bic = r.field(account.and_then(|a| r.path(a, &[(CAC, "FinancialInstitutionBranch"), (CBC, "ID")])), Kind::Text);
        p.mandate = r.field(r.path(m, &[(CAC, "PaymentMandate"), (CBC, "ID")]), Kind::Text);
    }
    inv.due_date = r.field(r.one(root, CBC, "DueDate"), Kind::Date);
    if inv.due_date.is_none() {
        // A credit note has no `DueDate`; its payment means may have one.
        inv.due_date = r.field(means.and_then(|m| r.one(m, CBC, "PaymentDueDate")), Kind::Date);
    }
    inv.payment_terms = r.field(r.path(root, &[(CAC, "PaymentTerms"), (CBC, "Note")]), Kind::Text);
    let charges: Vec<&El> = r.all(root, CAC, "AllowanceCharge").collect();
    inv.allowances_charges = r
        .bounded(charges.into_iter(), MAX_ROWS, "allowances and charges")
        .into_iter()
        .map(|a| AllowanceCharge {
            charge: r.one(a, CBC, "ChargeIndicator").is_some_and(|c| c.text.trim() == "true"),
            indicator: r.field(r.one(a, CBC, "ChargeIndicator"), Kind::Text),
            amount: r.field(r.one(a, CBC, "Amount"), Kind::Amount),
            base: r.field(r.one(a, CBC, "BaseAmount"), Kind::Amount),
            percent: r.field(r.one(a, CBC, "MultiplierFactorNumeric"), Kind::Number),
            reason: r.field(r.one(a, CBC, "AllowanceChargeReason"), Kind::Text),
            vat_category: r.field(r.path(a, &[(CAC, "TaxCategory"), (CBC, "ID")]), Kind::Text),
            vat_rate: r.field(r.path(a, &[(CAC, "TaxCategory"), (CBC, "Percent")]), Kind::Number),
        })
        .collect();
    // The VAT total in the document currency: the `TaxTotal` whose amount is given in it (a second one may give the VAT in the tax
    // currency, BT-111), else the first.
    let currency = inv.currency.as_ref().map(|c| c.text.clone());
    let tax_totals: Vec<&El> = r.all(root, CAC, "TaxTotal").collect();
    let in_currency = |t: &El| {
        r.one(t, CBC, "TaxAmount").and_then(|a| a.attrs.iter().find(|x| x.local == "currencyID")).map(|x| x.value.trim().to_owned()) == currency
    };
    let tax_total = tax_totals.iter().find(|t| in_currency(t)).or(tax_totals.first()).copied();
    if let Some(t) = tax_total {
        inv.totals.tax_total = r.field(r.one(t, CBC, "TaxAmount"), Kind::Amount);
        let subtotals: Vec<&El> = r.all(t, CAC, "TaxSubtotal").collect();
        inv.vat_breakdown = r
            .bounded(subtotals.into_iter(), MAX_ROWS, "VAT breakdown rows")
            .into_iter()
            .map(|s| VatRow {
                category: r.field(r.path(s, &[(CAC, "TaxCategory"), (CBC, "ID")]), Kind::Text),
                rate: r.field(r.path(s, &[(CAC, "TaxCategory"), (CBC, "Percent")]), Kind::Number),
                taxable: r.field(r.one(s, CBC, "TaxableAmount"), Kind::Amount),
                tax: r.field(r.one(s, CBC, "TaxAmount"), Kind::Amount),
                exemption_reason: r.field(r.path(s, &[(CAC, "TaxCategory"), (CBC, "TaxExemptionReason")]), Kind::Text),
            })
            .collect();
    }
    if let Some(m) = r.one(root, CAC, "LegalMonetaryTotal") {
        let t = &mut inv.totals;
        t.line_total = r.field(r.one(m, CBC, "LineExtensionAmount"), Kind::Amount);
        t.allowance_total = r.field(r.one(m, CBC, "AllowanceTotalAmount"), Kind::Amount);
        t.charge_total = r.field(r.one(m, CBC, "ChargeTotalAmount"), Kind::Amount);
        t.tax_exclusive = r.field(r.one(m, CBC, "TaxExclusiveAmount"), Kind::Amount);
        t.tax_inclusive = r.field(r.one(m, CBC, "TaxInclusiveAmount"), Kind::Amount);
        t.prepaid = r.field(r.one(m, CBC, "PrepaidAmount"), Kind::Amount);
        t.rounding = r.field(r.one(m, CBC, "PayableRoundingAmount"), Kind::Amount);
        t.payable = r.field(r.one(m, CBC, "PayableAmount"), Kind::Amount);
    }
    let (line_name, quantity_name) = if credit_note { ("CreditNoteLine", "CreditedQuantity") } else { ("InvoiceLine", "InvoicedQuantity") };
    let lines: Vec<&El> = r.all(root, CAC, line_name).collect();
    inv.lines_total = lines.len();
    inv.lines = r
        .bounded(lines.into_iter(), MAX_LINES, "lines")
        .into_iter()
        .map(|l| {
            let item = r.one(l, CAC, "Item");
            let quantity_el = r.one(l, CBC, quantity_name);
            let quantity = r.field(quantity_el, Kind::Quantity);
            Line {
                id: r.field(r.one(l, CBC, "ID"), Kind::Text),
                name: r.field(item.and_then(|i| r.one(i, CBC, "Name")), Kind::Text),
                description: r.field(item.and_then(|i| r.one(i, CBC, "Description")), Kind::Text),
                note: r.field(r.one(l, CBC, "Note"), Kind::Text),
                unit: unit_field(quantity_el),
                quantity,
                net_price: r.field(r.path(l, &[(CAC, "Price"), (CBC, "PriceAmount")]), Kind::Amount),
                line_total: r.field(r.one(l, CBC, "LineExtensionAmount"), Kind::Amount),
                vat_category: r.field(item.and_then(|i| r.path(i, &[(CAC, "ClassifiedTaxCategory"), (CBC, "ID")])), Kind::Text),
                vat_rate: r.field(item.and_then(|i| r.path(i, &[(CAC, "ClassifiedTaxCategory"), (CBC, "Percent")])), Kind::Number),
                seller_item_id: r.field(item.and_then(|i| r.path(i, &[(CAC, "SellersItemIdentification"), (CBC, "ID")])), Kind::Text),
                period_start: r.field(r.path(l, &[(CAC, "InvoicePeriod"), (CBC, "StartDate")]), Kind::Date),
                period_end: r.field(r.path(l, &[(CAC, "InvoicePeriod"), (CBC, "EndDate")]), Kind::Date),
            }
        })
        .collect();
    inv
}

/// The unit code of a quantity as a field of its own, at the place of the `unitCode` attribute value.
fn unit_field(el: Option<&El>) -> Option<Field> {
    let el = el?;
    let a = el.attrs.iter().find(|a| a.local == "unitCode" && a.namespace.is_none())?;
    let text: String = a.value.trim().chars().take(MAX_FIELD_CHARS).collect();
    if text.is_empty() {
        return None;
    }
    Some(Field { text, range: a.value_range.clone(), decimal: None, date: None, unit: None })
}

fn ubl_address<'a>(r: &mut Reader<'a>, a: &'a El) -> Address {
    let mut lines = Vec::new();
    for name in ["StreetName", "AdditionalStreetName"] {
        if let Some(f) = r.field(r.one(a, CBC, name), Kind::Text) {
            lines.push(f);
        }
    }
    let more: Vec<&El> = r.all(a, CAC, "AddressLine").filter_map(|l| r.one(l, CBC, "Line")).collect();
    for l in r.bounded(more.into_iter(), MAX_ROWS, "address lines") {
        if let Some(f) = r.field(Some(l), Kind::Text) {
            lines.push(f);
        }
    }
    Address {
        lines,
        city: r.field(r.one(a, CBC, "CityName"), Kind::Text),
        postcode: r.field(r.one(a, CBC, "PostalZone"), Kind::Text),
        subdivision: r.field(r.one(a, CBC, "CountrySubentity"), Kind::Text),
        country: r.field(r.path(a, &[(CAC, "Country"), (CBC, "IdentificationCode")]), Kind::Text),
    }
}

fn ubl_party<'a>(r: &mut Reader<'a>, p: &'a El) -> Party {
    let registration = r.field(r.path(p, &[(CAC, "PartyLegalEntity"), (CBC, "RegistrationName")]), Kind::Text);
    let trading = r.field(r.path(p, &[(CAC, "PartyName"), (CBC, "Name")]), Kind::Text);
    let schemes: Vec<&El> = r.all(p, CAC, "PartyTaxScheme").collect();
    let scheme_is_vat = |s: &El| r.path(s, &[(CAC, "TaxScheme"), (CBC, "ID")]).is_some_and(|i| i.text.trim() == "VAT");
    let vat = schemes.iter().find(|s| scheme_is_vat(s)).and_then(|s| r.one(s, CBC, "CompanyID"));
    let other = schemes.iter().find(|s| !scheme_is_vat(s)).and_then(|s| r.one(s, CBC, "CompanyID"));
    let contact = r.one(p, CAC, "Contact");
    let address = r.one(p, CAC, "PostalAddress").map(|a| ubl_address(r, a)).unwrap_or_default();
    let endpoint = r.one(p, CBC, "EndpointID");
    let mut electronic_address = r.field(endpoint, Kind::Text);
    if let (Some(f), Some(e)) = (electronic_address.as_mut(), endpoint) {
        f.unit = e.attrs.iter().find(|a| a.local == "schemeID").map(|a| a.value.trim().to_owned());
    }
    Party {
        name: registration.clone().or_else(|| trading.clone()),
        trading_name: trading.filter(|t| registration.as_ref().is_none_or(|r| r.text != t.text)),
        id: r.field(r.path(p, &[(CAC, "PartyIdentification"), (CBC, "ID")]), Kind::Text),
        address,
        vat_id: r.field(vat, Kind::Text),
        tax_registration: r.field(other, Kind::Text),
        legal_id: r.field(r.path(p, &[(CAC, "PartyLegalEntity"), (CBC, "CompanyID")]), Kind::Text),
        contact: Contact {
            name: r.field(contact.and_then(|c| r.one(c, CBC, "Name")), Kind::Text),
            phone: r.field(contact.and_then(|c| r.one(c, CBC, "Telephone")), Kind::Text),
            email: r.field(contact.and_then(|c| r.one(c, CBC, "ElectronicMail")), Kind::Text),
        },
        electronic_address,
    }
}

// ---- UN/CEFACT Cross Industry Invoice D16B (the EN 16931 CII syntax binding) ----

const RAM: &str = CII_RAM;
const RSM: &str = CII;

/// A CII date: `<udt:DateTimeString format="102">` inside `el`.
fn cii_date<'a>(r: &mut Reader<'a>, el: Option<&'a El>) -> Option<Field> {
    let el = el?;
    let s = r.one(el, CII_UDT, "DateTimeString");
    r.field(s, Kind::Date)
}

fn cii<'a>(r: &mut Reader<'a>, root: &'a El) -> Invoice {
    let mut inv = empty(Syntax::Cii, "Invoice");
    if let Some(d) = r.one(root, RSM, "ExchangedDocument") {
        inv.number = r.field(r.one(d, RAM, "ID"), Kind::Text);
        inv.type_code = r.field(r.one(d, RAM, "TypeCode"), Kind::Text);
        let issue = r.one(d, RAM, "IssueDateTime");
        inv.issue_date = cii_date(r, issue);
        let notes: Vec<&El> = r.all(d, RAM, "IncludedNote").filter_map(|n| r.one(n, RAM, "Content")).collect();
        inv.notes = r.bounded(notes.into_iter(), MAX_ROWS, "notes").into_iter().filter_map(|n| r.field(Some(n), Kind::Text)).collect();
    }
    let Some(tx) = r.one(root, RSM, "SupplyChainTradeTransaction") else { return inv };
    if let Some(a) = r.one(tx, RAM, "ApplicableHeaderTradeAgreement") {
        inv.buyer_reference = r.field(r.one(a, RAM, "BuyerReference"), Kind::Text);
        inv.order_reference = r.field(r.path(a, &[(RAM, "BuyerOrderReferencedDocument"), (RAM, "IssuerAssignedID")]), Kind::Text);
        if let Some(p) = r.one(a, RAM, "SellerTradeParty") {
            inv.seller = cii_party(r, p);
        }
        if let Some(p) = r.one(a, RAM, "BuyerTradeParty") {
            inv.buyer = cii_party(r, p);
        }
    }
    if let Some(d) = r.one(tx, RAM, "ApplicableHeaderTradeDelivery") {
        let event = r.path(d, &[(RAM, "ActualDeliverySupplyChainEvent"), (RAM, "OccurrenceDateTime")]);
        inv.delivery.date = cii_date(r, event);
        if let Some(ship) = r.one(d, RAM, "ShipToTradeParty") {
            inv.delivery.party_name = r.field(r.one(ship, RAM, "Name"), Kind::Text);
            if let Some(a) = r.one(ship, RAM, "PostalTradeAddress") {
                inv.delivery.address = cii_address(r, a);
            }
        }
    }
    if let Some(s) = r.one(tx, RAM, "ApplicableHeaderTradeSettlement") {
        inv.currency = r.field(r.one(s, RAM, "InvoiceCurrencyCode"), Kind::Text);
        r.currency = inv.currency.as_ref().map(|c| c.text.clone());
        inv.payee = r.one(s, RAM, "PayeeTradeParty").map(|p| cii_party(r, p));
        inv.payment.reference = r.field(r.one(s, RAM, "PaymentReference"), Kind::Text);
        if let Some(m) = r.one(s, RAM, "SpecifiedTradeSettlementPaymentMeans") {
            let account = r.one(m, RAM, "PayeePartyCreditorFinancialAccount");
            let p = &mut inv.payment;
            p.means_code = r.field(r.one(m, RAM, "TypeCode"), Kind::Text);
            p.iban = r.field(account.and_then(|a| r.one(a, RAM, "IBANID")), Kind::Text);
            p.account_name = r.field(account.and_then(|a| r.one(a, RAM, "AccountName")), Kind::Text);
            p.bic = r.field(r.path(m, &[(RAM, "PayeeSpecifiedCreditorFinancialInstitution"), (RAM, "BICID")]), Kind::Text);
        }
        let terms = r.one(s, RAM, "SpecifiedTradePaymentTerms");
        inv.payment_terms = r.field(terms.and_then(|t| r.one(t, RAM, "Description")), Kind::Text);
        let due = terms.and_then(|t| r.one(t, RAM, "DueDateDateTime"));
        inv.due_date = cii_date(r, due);
        inv.payment.mandate = r.field(terms.and_then(|t| r.one(t, RAM, "DirectDebitMandateID")), Kind::Text);
        let taxes: Vec<&El> = r.all(s, RAM, "ApplicableTradeTax").collect();
        inv.vat_breakdown = r
            .bounded(taxes.into_iter(), MAX_ROWS, "VAT breakdown rows")
            .into_iter()
            .map(|t| VatRow {
                category: r.field(r.one(t, RAM, "CategoryCode"), Kind::Text),
                rate: r.field(r.one(t, RAM, "RateApplicablePercent"), Kind::Number),
                taxable: r.field(r.one(t, RAM, "BasisAmount"), Kind::Amount),
                tax: r.field(r.one(t, RAM, "CalculatedAmount"), Kind::Amount),
                exemption_reason: r.field(r.one(t, RAM, "ExemptionReason"), Kind::Text),
            })
            .collect();
        let charges: Vec<&El> = r.all(s, RAM, "SpecifiedTradeAllowanceCharge").collect();
        inv.allowances_charges = r
            .bounded(charges.into_iter(), MAX_ROWS, "allowances and charges")
            .into_iter()
            .map(|a| {
                let tax = r.one(a, RAM, "CategoryTradeTax");
                AllowanceCharge {
                    charge: r.path(a, &[(RAM, "ChargeIndicator"), (CII_UDT, "Indicator")]).is_some_and(|c| c.text.trim() == "true"),
                    indicator: r.field(r.path(a, &[(RAM, "ChargeIndicator"), (CII_UDT, "Indicator")]), Kind::Text),
                    amount: r.field(r.one(a, RAM, "ActualAmount"), Kind::Amount),
                    base: r.field(r.one(a, RAM, "BasisAmount"), Kind::Amount),
                    percent: r.field(r.one(a, RAM, "CalculationPercent"), Kind::Number),
                    reason: r.field(r.one(a, RAM, "Reason"), Kind::Text),
                    vat_category: r.field(tax.and_then(|t| r.one(t, RAM, "CategoryCode")), Kind::Text),
                    vat_rate: r.field(tax.and_then(|t| r.one(t, RAM, "RateApplicablePercent")), Kind::Number),
                }
            })
            .collect();
        if let Some(m) = r.one(s, RAM, "SpecifiedTradeSettlementHeaderMonetarySummation") {
            let currency = inv.currency.as_ref().map(|c| c.text.clone());
            // The VAT total in the invoice currency (a second `TaxTotalAmount` may give it in the tax currency, BT-111).
            let tax_totals: Vec<&El> = r.all(m, RAM, "TaxTotalAmount").collect();
            let tax_total = tax_totals
                .iter()
                .find(|t| t.attrs.iter().find(|a| a.local == "currencyID").map(|a| a.value.trim().to_owned()) == currency)
                .or(tax_totals.first())
                .copied();
            let t = &mut inv.totals;
            t.line_total = r.field(r.one(m, RAM, "LineTotalAmount"), Kind::Amount);
            t.allowance_total = r.field(r.one(m, RAM, "AllowanceTotalAmount"), Kind::Amount);
            t.charge_total = r.field(r.one(m, RAM, "ChargeTotalAmount"), Kind::Amount);
            t.tax_exclusive = r.field(r.one(m, RAM, "TaxBasisTotalAmount"), Kind::Amount);
            t.tax_total = r.field(tax_total, Kind::Amount);
            t.tax_inclusive = r.field(r.one(m, RAM, "GrandTotalAmount"), Kind::Amount);
            t.prepaid = r.field(r.one(m, RAM, "TotalPrepaidAmount"), Kind::Amount);
            t.rounding = r.field(r.one(m, RAM, "RoundingAmount"), Kind::Amount);
            t.payable = r.field(r.one(m, RAM, "DuePayableAmount"), Kind::Amount);
        }
    }
    let lines: Vec<&El> = r.all(tx, RAM, "IncludedSupplyChainTradeLineItem").collect();
    inv.lines_total = lines.len();
    inv.lines = r.bounded(lines.into_iter(), MAX_LINES, "lines").into_iter().map(|l| cii_line(r, l)).collect();
    inv
}

fn cii_line<'a>(r: &mut Reader<'a>, l: &'a El) -> Line {
    let doc = r.one(l, RAM, "AssociatedDocumentLineDocument");
    let product = r.one(l, RAM, "SpecifiedTradeProduct");
    let settlement = r.one(l, RAM, "SpecifiedLineTradeSettlement");
    let tax = settlement.and_then(|s| r.one(s, RAM, "ApplicableTradeTax"));
    let period = settlement.and_then(|s| r.one(s, RAM, "BillingSpecifiedPeriod"));
    let quantity_el = r.path(l, &[(RAM, "SpecifiedLineTradeDelivery"), (RAM, "BilledQuantity")]);
    let quantity = r.field(quantity_el, Kind::Quantity);
    let start = period.and_then(|p| r.one(p, RAM, "StartDateTime"));
    let end = period.and_then(|p| r.one(p, RAM, "EndDateTime"));
    Line {
        id: r.field(doc.and_then(|d| r.one(d, RAM, "LineID")), Kind::Text),
        name: r.field(product.and_then(|p| r.one(p, RAM, "Name")), Kind::Text),
        description: r.field(product.and_then(|p| r.one(p, RAM, "Description")), Kind::Text),
        note: r.field(doc.and_then(|d| r.path(d, &[(RAM, "IncludedNote"), (RAM, "Content")])), Kind::Text),
        unit: unit_field(quantity_el),
        quantity,
        net_price: r
            .field(r.path(l, &[(RAM, "SpecifiedLineTradeAgreement"), (RAM, "NetPriceProductTradePrice"), (RAM, "ChargeAmount")]), Kind::Amount),
        line_total: r.field(
            settlement.and_then(|s| r.path(s, &[(RAM, "SpecifiedTradeSettlementLineMonetarySummation"), (RAM, "LineTotalAmount")])),
            Kind::Amount,
        ),
        vat_category: r.field(tax.and_then(|t| r.one(t, RAM, "CategoryCode")), Kind::Text),
        vat_rate: r.field(tax.and_then(|t| r.one(t, RAM, "RateApplicablePercent")), Kind::Number),
        seller_item_id: r.field(product.and_then(|p| r.one(p, RAM, "SellerAssignedID")), Kind::Text),
        period_start: cii_date(r, start),
        period_end: cii_date(r, end),
    }
}

fn cii_address<'a>(r: &mut Reader<'a>, a: &'a El) -> Address {
    let lines = ["LineOne", "LineTwo", "LineThree"].into_iter().filter_map(|n| r.field(r.one(a, RAM, n), Kind::Text)).collect();
    Address {
        lines,
        city: r.field(r.one(a, RAM, "CityName"), Kind::Text),
        postcode: r.field(r.one(a, RAM, "PostcodeCode"), Kind::Text),
        subdivision: r.field(r.one(a, RAM, "CountrySubDivisionName"), Kind::Text),
        country: r.field(r.one(a, RAM, "CountryID"), Kind::Text),
    }
}

fn cii_party<'a>(r: &mut Reader<'a>, p: &'a El) -> Party {
    let registrations: Vec<&El> = r.all(p, RAM, "SpecifiedTaxRegistration").filter_map(|t| r.one(t, RAM, "ID")).collect();
    let scheme = |e: &El, s: &str| e.attrs.iter().any(|a| a.local == "schemeID" && a.value.trim() == s);
    let vat = registrations.iter().find(|e| scheme(e, "VA")).copied();
    let fc = registrations.iter().find(|e| scheme(e, "FC")).copied();
    let legal = r.one(p, RAM, "SpecifiedLegalOrganization");
    let contact = r.one(p, RAM, "DefinedTradeContact");
    let address = r.one(p, RAM, "PostalTradeAddress").map(|a| cii_address(r, a)).unwrap_or_default();
    let uri = r.path(p, &[(RAM, "URIUniversalCommunication"), (RAM, "URIID")]);
    let mut electronic_address = r.field(uri, Kind::Text);
    if let (Some(f), Some(e)) = (electronic_address.as_mut(), uri) {
        f.unit = e.attrs.iter().find(|a| a.local == "schemeID").map(|a| a.value.trim().to_owned());
    }
    let name = r.field(r.one(p, RAM, "Name"), Kind::Text);
    let trading = r.field(legal.and_then(|l| r.one(l, RAM, "TradingBusinessName")), Kind::Text);
    Party {
        name: name.clone().or_else(|| trading.clone()),
        trading_name: trading.filter(|t| name.as_ref().is_none_or(|n| n.text != t.text)),
        id: r.field(r.one(p, RAM, "ID"), Kind::Text),
        address,
        vat_id: r.field(vat, Kind::Text),
        tax_registration: r.field(fc, Kind::Text),
        legal_id: r.field(legal.and_then(|l| r.one(l, RAM, "ID")), Kind::Text),
        contact: Contact {
            name: r.field(contact.and_then(|c| r.one(c, RAM, "PersonName")), Kind::Text),
            phone: r.field(contact.and_then(|c| r.path(c, &[(RAM, "TelephoneUniversalCommunication"), (RAM, "CompleteNumber")])), Kind::Text),
            email: r.field(contact.and_then(|c| r.path(c, &[(RAM, "EmailURIUniversalCommunication"), (RAM, "URIID")])), Kind::Text),
        },
        electronic_address,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_keep_their_digits_and_refuse_what_is_no_xs_decimal() {
        assert_eq!(Decimal::parse("19.00"), Some(Decimal { mantissa: 1900, scale: 2 }));
        assert_eq!(Decimal::parse("-0.5"), Some(Decimal { mantissa: -5, scale: 1 }));
        assert_eq!(Decimal::parse("+.5"), Some(Decimal { mantissa: 5, scale: 1 }));
        assert_eq!(Decimal::parse("007"), Some(Decimal { mantissa: 7, scale: 0 }));
        assert_eq!(Decimal::parse("0.000"), Some(Decimal { mantissa: 0, scale: 3 }));
        for bad in ["", ".", "-", "1e3", "1,5", "NaN", "1.2.3", " ", &"9".repeat(40)] {
            assert_eq!(Decimal::parse(bad), None, "{bad:?}");
        }
        assert_eq!(Decimal::parse("1234.50").map(Decimal::digits), Some(("1234".into(), "50".into())));
        assert_eq!(Decimal::parse("-0.05").map(Decimal::digits), Some(("0".into(), "05".into())));
        assert_eq!(Decimal::parse("12").map(Decimal::digits), Some(("12".into(), String::new())));
    }

    #[test]
    fn a_field_points_at_its_text_and_a_long_one_is_cut() {
        let long = "é".repeat(MAX_FIELD_CHARS + 5);
        let doc = format!(
            "<Invoice xmlns=\"{UBL_INVOICE}\" xmlns:cbc=\"{UBL_CBC}\"><cbc:ID a=\"x>y\">  RE-1 </cbc:ID><cbc:Note>{long}</cbc:Note><cbc:IssueDate>2026-02-30</cbc:IssueDate></Invoice>"
        );
        let inv = read(&doc).expect("an invoice");
        let number = inv.number.expect("a number");
        assert_eq!((&doc[number.range.clone()], number.text.as_str()), ("RE-1", "RE-1"));
        assert_eq!(inv.notes[0].text.chars().count(), MAX_FIELD_CHARS);
        assert!(inv.cut.iter().any(|c| c.contains("cut at")), "{:?}", inv.cut);
        // A date that is no date keeps its text and has no calendar date.
        let issue = inv.issue_date.expect("a date");
        assert_eq!((issue.text.as_str(), issue.date), ("2026-02-30", None));
    }

    #[test]
    fn other_documents_have_no_model() {
        for text in ["", "<a/>", "not xml", "<Order xmlns=\"urn:oasis:names:specification:ubl:schema:xsd:Order-2\"/>"] {
            assert_eq!(read(text), None, "{text:?}");
        }
        assert!(read(&format!("<CreditNote xmlns=\"{UBL_CREDIT_NOTE}\"/>")).is_some_and(|i| i.document == "Credit note"));
    }

    proptest::proptest! {
        #[test]
        fn any_text_is_read_without_a_panic(text in ".{0,400}") {
            let _ = read(&text);
            let _ = read(&format!("<Invoice xmlns=\"{UBL_INVOICE}\" xmlns:cbc=\"{UBL_CBC}\">{text}</Invoice>"));
        }

        #[test]
        fn any_decimal_text_is_answered(text in "[-+0-9.]{0,50}") {
            if let Some(d) = Decimal::parse(&text) {
                let (whole, fraction) = d.digits();
                proptest::prop_assert!(whole.bytes().all(|b| b.is_ascii_digit()) && fraction.bytes().all(|b| b.is_ascii_digit()));
            }
        }
    }
}
