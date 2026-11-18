//! A hand-written, scannerless parser for XQuery 3.1 (without the Full-Text and Update extensions).
//!
//! XQuery's lexical structure depends on where the parser is (keywords are names, `<` is an operator or a tag, whitespace and
//! comments are skipped between tokens but are content inside direct constructors), so it reads characters, not tokens. Every
//! recursion goes through a depth counter, and the tree's depth is checked once more after parsing (operator chains are built in
//! loops), so no query can exhaust the stack. A syntax error is `XPST0003` with the place and the tokens that would have fitted.

use std::collections::HashMap;

use ibig::IBig;
use ordered_float::OrderedFloat;
use rust_decimal::Decimal;
use xee_xpath_ast::ast as xast;
use xot::xmlname::NameStrInfo;

use crate::ast::*;

/// The most bytes of query text read.
pub const MAX_QUERY_BYTES: usize = 4 * 1024 * 1024;
/// How deeply expressions may nest while parsing (parentheses, constructors, clauses).
pub const MAX_NESTING: usize = 96;
/// How deep the finished tree may be (operator chains such as `1 + 1 + …` are built in loops but nest in the tree).
pub const MAX_TREE_DEPTH: usize = 384;

pub const FN_NS: &str = "http://www.w3.org/2005/xpath-functions";
pub const XS_NS: &str = "http://www.w3.org/2001/XMLSchema";
pub const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
pub const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";
pub const ERR_NS: &str = "http://www.w3.org/2005/xqt-errors";
pub const LOCAL_NS: &str = "http://www.w3.org/2005/xquery-local-functions";
pub const ANNOTATION_NS: &str = "http://www.w3.org/2012/xquery";

/// The namespaces every query knows (XQuery 3.1 §4.12, plus `err` for `try`/`catch`).
pub const PREDECLARED: &[(&str, &str)] = &[
    ("xml", XML_NS),
    ("xs", XS_NS),
    ("xsi", "http://www.w3.org/2001/XMLSchema-instance"),
    ("fn", FN_NS),
    ("local", LOCAL_NS),
    ("math", "http://www.w3.org/2005/xpath-functions/math"),
    ("map", "http://www.w3.org/2005/xpath-functions/map"),
    ("array", "http://www.w3.org/2005/xpath-functions/array"),
    ("err", ERR_NS),
];

/// A static error found while reading the query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// The W3C error code (`XPST0003` for syntax, `XPST0081` for an unknown prefix, …).
    pub code: &'static str,
    pub message: String,
    /// Where (a byte range of the query).
    pub span: Span,
    /// For a syntax error, what would have been accepted at the place of the error.
    pub expected: Vec<String>,
}

type R<T> = Result<T, ParseError>;

#[derive(Clone, Debug)]
struct Scope {
    prefixes: HashMap<String, String>,
    default_element: String,
    default_function: String,
}

/// How an unprefixed name is resolved.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NameKind {
    /// Elements and types: the default element namespace.
    Element,
    /// Functions: the default function namespace.
    Function,
    /// Variables and attributes: no namespace.
    None,
    /// Annotations: the XQuery annotation namespace.
    Annotation,
}

/// A name as written: `p:local`, `local`, or `Q{uri}local`.
#[derive(Clone, Debug)]
struct RawName {
    prefix: Option<String>,
    uri: Option<String>,
    local: String,
    span: Span,
}

/// Read a module. `namespaces` are prefixes known besides the predeclared ones (from the caller's options).
pub fn parse(src: &str, namespaces: &[(String, String)]) -> Result<Module, ParseError> {
    if src.len() > MAX_QUERY_BYTES {
        return Err(ParseError {
            code: "XPST0003",
            message: format!("the query is longer than {} bytes", MAX_QUERY_BYTES),
            span: 0..0,
            expected: Vec::new(),
        });
    }
    let mut prefixes: HashMap<String, String> = PREDECLARED.iter().map(|(p, u)| ((*p).to_owned(), (*u).to_owned())).collect();
    for (p, u) in namespaces {
        prefixes.insert(p.clone(), u.clone());
    }
    let mut parser = Parser {
        src,
        pos: 0,
        depth: 0,
        scopes: vec![Scope { prefixes, default_element: String::new(), default_function: FN_NS.to_owned() }],
        furthest: 0,
        expected: Vec::new(),
        declared_prefixes: Vec::new(),
    };
    let module = parser.module()?;
    if let Some(body) = &module.body {
        check_depth(body)?;
    }
    for decl in &module.prolog {
        match &decl.value {
            Decl::Variable { value: Some(e), .. } | Decl::ContextItem { value: Some(e), .. } => check_depth(e)?,
            Decl::Function(f) => {
                if let Some(b) = &f.body {
                    check_depth(b)?;
                }
            }
            _ => {}
        }
    }
    Ok(module)
}

/// The tree's depth, measured without recursion.
fn check_depth(root: &ExprS) -> R<()> {
    let mut stack: Vec<(&ExprS, usize)> = vec![(root, 1)];
    while let Some((e, d)) = stack.pop() {
        if d > MAX_TREE_DEPTH {
            return Err(ParseError {
                code: "XPST0003",
                message: format!("the expression is nested more than {MAX_TREE_DEPTH} levels deep"),
                span: e.span.clone(),
                expected: Vec::new(),
            });
        }
        for c in e.value.children() {
            stack.push((c, d.saturating_add(1)));
        }
    }
    Ok(())
}

fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || ('\u{2070}'..='\u{218F}').contains(&c) || ('\u{3001}'..='\u{D7FF}').contains(&c)
}

fn is_name_char(c: char) -> bool {
    is_name_start(c)
        || c.is_alphanumeric()
        || c == '-'
        || c == '.'
        || c == '\u{B7}'
        || ('\u{300}'..='\u{36F}').contains(&c)
        || ('\u{203F}'..='\u{2040}').contains(&c)
}

fn is_xml_char(c: char) -> bool {
    matches!(c, '\u{9}' | '\u{A}' | '\u{D}' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

/// Names that are never function names (XQuery 3.1 §A.3), so `name(` before them is a test or an expression.
const RESERVED_FUNCTION_NAMES: &[&str] = &[
    "array",
    "attribute",
    "comment",
    "document-node",
    "element",
    "empty-sequence",
    "function",
    "if",
    "item",
    "map",
    "namespace-node",
    "node",
    "processing-instruction",
    "schema-attribute",
    "schema-element",
    "switch",
    "text",
    "typeswitch",
];

const KIND_TESTS: &[&str] = &[
    "attribute",
    "comment",
    "document-node",
    "element",
    "namespace-node",
    "node",
    "processing-instruction",
    "schema-attribute",
    "schema-element",
    "text",
];

const AXES: &[(&str, xast::Axis)] = &[
    ("ancestor-or-self", xast::Axis::AncestorOrSelf),
    ("ancestor", xast::Axis::Ancestor),
    ("attribute", xast::Axis::Attribute),
    ("child", xast::Axis::Child),
    ("descendant-or-self", xast::Axis::DescendantOrSelf),
    ("descendant", xast::Axis::Descendant),
    ("following-sibling", xast::Axis::FollowingSibling),
    ("following", xast::Axis::Following),
    ("namespace", xast::Axis::Namespace),
    ("parent", xast::Axis::Parent),
    ("preceding-sibling", xast::Axis::PrecedingSibling),
    ("preceding", xast::Axis::Preceding),
    ("self", xast::Axis::Self_),
];

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    depth: usize,
    scopes: Vec<Scope>,
    /// The furthest place a token was looked for, and the tokens looked for there (for the "expected" hint).
    furthest: usize,
    expected: Vec<String>,
    declared_prefixes: Vec<String>,
}

impl<'a> Parser<'a> {
    // ---- characters and tokens ----

    fn rest(&self) -> &'a str {
        self.src.get(self.pos..).unwrap_or("")
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.rest().chars().nth(n)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos = self.pos.saturating_add(c.len_utf8());
        Some(c)
    }

    fn scope(&self) -> &Scope {
        // (There is always the outermost scope: `parse` creates it and pushes and pops are paired.)
        self.scopes.last().unwrap_or_else(|| unreachable_scope())
    }

    fn scope_mut(&mut self) -> &mut Scope {
        if self.scopes.is_empty() {
            self.scopes.push(Scope { prefixes: HashMap::new(), default_element: String::new(), default_function: FN_NS.to_owned() });
        }
        let last = self.scopes.len().saturating_sub(1);
        &mut self.scopes[last]
    }

    fn err(&self, code: &'static str, message: impl Into<String>, span: Span) -> ParseError {
        ParseError { code, message: message.into(), span, expected: Vec::new() }
    }

    /// A syntax error at the furthest place reached, saying what would have fitted there.
    fn syntax(&self, what: &str) -> ParseError {
        let at = self.furthest.max(self.pos);
        let found = self.src.get(at..).and_then(|s| s.chars().next());
        let end = at.saturating_add(found.map_or(0, char::len_utf8));
        let mut expected = if at == self.furthest { self.expected.clone() } else { Vec::new() };
        expected.sort();
        expected.dedup();
        let found = match found {
            Some(c) => format!("'{c}'"),
            None => "the end of the query".to_owned(),
        };
        let hint = if expected.is_empty() { String::new() } else { format!(" (expected {})", expected.join(", ")) };
        ParseError { code: "XPST0003", message: format!("{what}: unexpected {found}{hint}"), span: at..end, expected }
    }

    fn note(&mut self, token: &str) {
        if self.pos > self.furthest {
            self.furthest = self.pos;
            self.expected.clear();
        }
        if self.pos == self.furthest && self.expected.len() < 24 && !self.expected.iter().any(|e| e == token) {
            self.expected.push(token.to_owned());
        }
    }

    /// Skip whitespace and (nested) comments.
    fn ws(&mut self) -> R<()> {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.bump();
                }
                Some('(') if self.peek_at(1) == Some(':') => {
                    let start = self.pos;
                    self.pos = self.pos.saturating_add(2);
                    let mut level = 1usize;
                    loop {
                        if self.rest().starts_with("(:") {
                            self.pos = self.pos.saturating_add(2);
                            level = level.saturating_add(1);
                            if level > MAX_NESTING {
                                return Err(self.err("XPST0003", "comments are nested too deeply", start..self.pos));
                            }
                        } else if self.rest().starts_with(":)") {
                            self.pos = self.pos.saturating_add(2);
                            level = level.saturating_sub(1);
                            if level == 0 {
                                break;
                            }
                        } else if self.bump().is_none() {
                            return Err(self.err("XPST0003", "a comment is not closed with ':)'", start..self.src.len()));
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn at(&mut self, s: &str) -> R<bool> {
        self.ws()?;
        Ok(self.rest().starts_with(s))
    }

    /// Read the symbol `s` (after whitespace) if it is next.
    fn eat(&mut self, s: &str) -> R<bool> {
        self.ws()?;
        if self.rest().starts_with(s) {
            self.pos = self.pos.saturating_add(s.len());
            Ok(true)
        } else {
            self.note(&format!("'{s}'"));
            Ok(false)
        }
    }

    fn expect(&mut self, s: &str) -> R<()> {
        if self.eat(s)? { Ok(()) } else { Err(self.syntax("syntax error")) }
    }

    /// Whether the keyword `kw` is next (a name that is not followed by more name characters).
    fn at_kw(&mut self, kw: &str) -> R<bool> {
        self.ws()?;
        let rest = self.rest();
        if !rest.starts_with(kw) {
            self.note(&format!("'{kw}'"));
            return Ok(false);
        }
        let after = rest.get(kw.len()..).and_then(|s| s.chars().next());
        Ok(!after.is_some_and(is_name_char))
    }

    fn eat_kw(&mut self, kw: &str) -> R<bool> {
        if self.at_kw(kw)? {
            self.pos = self.pos.saturating_add(kw.len());
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn expect_kw(&mut self, kw: &str) -> R<()> {
        if self.eat_kw(kw)? { Ok(()) } else { Err(self.syntax("syntax error")) }
    }

    /// Whether keyword `kw` is next and is followed by the symbol `then` (without reading them).
    fn at_kw_then(&mut self, kw: &str, then: &str) -> R<bool> {
        let save = self.pos;
        let ok = self.eat_kw(kw)? && self.at(then)?;
        self.pos = save;
        Ok(ok)
    }

    fn at_kw_kw(&mut self, kw: &str, then: &str) -> R<bool> {
        let save = self.pos;
        let ok = self.eat_kw(kw)? && self.at_kw(then)?;
        self.pos = save;
        Ok(ok)
    }

    fn enter(&mut self) -> R<()> {
        self.depth = self.depth.saturating_add(1);
        if self.depth > MAX_NESTING {
            return Err(self.err(
                "XPST0003",
                format!("the query is nested more than {MAX_NESTING} levels deep"),
                self.pos..self.pos.saturating_add(1).min(self.src.len()),
            ));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    // ---- names ----

    fn ncname(&mut self) -> Option<String> {
        let rest = self.rest();
        let mut chars = rest.char_indices();
        match chars.next() {
            Some((_, c)) if is_name_start(c) => {}
            _ => return None,
        }
        let end = chars.find(|(_, c)| !is_name_char(*c)).map_or(rest.len(), |(i, _)| i);
        let name = rest.get(..end)?.to_owned();
        self.pos = self.pos.saturating_add(end);
        Some(name)
    }

    /// Read an EQName (after whitespace), unresolved.
    fn raw_name(&mut self) -> R<Option<RawName>> {
        self.ws()?;
        let start = self.pos;
        if self.rest().starts_with("Q{") {
            let Some(close) = self.rest().find('}') else { return Err(self.err("XPST0003", "a 'Q{' name is not closed", start..self.src.len())) };
            let uri = self.rest().get(2..close).unwrap_or("").to_owned();
            if uri.contains('{') {
                return Err(self.err("XPST0003", "a URI in a 'Q{…}' name contains '{'", start..start.saturating_add(close)));
            }
            self.pos = self.pos.saturating_add(close + 1);
            let Some(local) = self.ncname() else {
                self.pos = start;
                return Ok(None);
            };
            return Ok(Some(RawName { prefix: None, uri: Some(normalize_space(&resolve_refs_lenient(&uri))), local, span: start..self.pos }));
        }
        let Some(first) = self.ncname() else {
            self.note("a name");
            return Ok(None);
        };
        if self.rest().starts_with(':') && self.peek_at(1).is_some_and(is_name_start) {
            self.pos = self.pos.saturating_add(1);
            let Some(local) = self.ncname() else { return Ok(None) };
            return Ok(Some(RawName { prefix: Some(first), uri: None, local, span: start..self.pos }));
        }
        Ok(Some(RawName { prefix: None, uri: None, local: first, span: start..self.pos }))
    }

    fn resolve(&self, raw: &RawName, kind: NameKind) -> R<Name> {
        if let Some(uri) = &raw.uri {
            return Ok(Name::new(raw.local.clone(), uri.clone(), String::new()));
        }
        match &raw.prefix {
            Some(p) => {
                let Some(uri) = self.scope().prefixes.get(p) else {
                    return Err(self.err("XPST0081", format!("the prefix '{p}' is not declared"), raw.span.clone()));
                };
                Ok(Name::new(raw.local.clone(), uri.clone(), p.clone()))
            }
            None => {
                let ns = match kind {
                    NameKind::Element => self.scope().default_element.clone(),
                    NameKind::Function => self.scope().default_function.clone(),
                    NameKind::None => String::new(),
                    NameKind::Annotation => ANNOTATION_NS.to_owned(),
                };
                Ok(Name::new(raw.local.clone(), ns, String::new()))
            }
        }
    }

    fn name(&mut self, kind: NameKind) -> R<Name> {
        match self.raw_name()? {
            Some(raw) => self.resolve(&raw, kind),
            None => Err(self.syntax("expected a name")),
        }
    }

    fn var_name(&mut self) -> R<VarBinding> {
        self.ws()?;
        let start = self.pos;
        self.expect("$")?;
        let name = self.name(NameKind::None)?;
        Ok(VarBinding { name, type_: None, span: start..self.pos })
    }

    // ---- literals ----

    fn string_literal(&mut self) -> R<Option<String>> {
        self.ws()?;
        let Some(quote) = self.peek().filter(|c| *c == '"' || *c == '\'') else {
            self.note("a string");
            return Ok(None);
        };
        let start = self.pos;
        self.bump();
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err(self.err("XPST0003", "a string is not closed", start..self.src.len())),
                Some(c) if c == quote => {
                    if self.peek() == Some(quote) {
                        self.bump();
                        out.push(quote);
                    } else {
                        break;
                    }
                }
                Some('&') => {
                    let at = self.pos.saturating_sub(1);
                    let c = self.reference(at)?;
                    out.push_str(&c);
                }
                Some('\r') => {
                    if self.peek() == Some('\n') {
                        self.bump();
                    }
                    out.push('\n');
                }
                Some(c) => out.push(c),
            }
        }
        Ok(Some(out))
    }

    fn uri_literal(&mut self) -> R<String> {
        match self.string_literal()? {
            Some(s) => Ok(normalize_space(&s)),
            None => Err(self.syntax("expected a URI literal")),
        }
    }

    /// After `&`: a predefined entity or a character reference (XQuery 3.1 §3.1.1), as text.
    fn reference(&mut self, at: usize) -> R<String> {
        let rest = self.rest();
        let Some(semi) = rest.find(';').filter(|i| *i <= 12) else {
            return Err(self.err("XPST0003", "'&' starts no entity or character reference", at..self.pos));
        };
        let body = rest.get(..semi).unwrap_or("");
        let text = match body {
            "lt" => "<".to_owned(),
            "gt" => ">".to_owned(),
            "amp" => "&".to_owned(),
            "quot" => "\"".to_owned(),
            "apos" => "'".to_owned(),
            _ => {
                let code = if let Some(hex) = body.strip_prefix("#x") {
                    if hex.is_empty() || !hex.chars().all(|c| c.is_ascii_hexdigit()) { None } else { u32::from_str_radix(hex, 16).ok() }
                } else if let Some(dec) = body.strip_prefix('#') {
                    if dec.is_empty() || !dec.chars().all(|c| c.is_ascii_digit()) { None } else { dec.parse::<u32>().ok() }
                } else {
                    return Err(self.err("XPST0003", format!("'&{body};' is not a predefined entity"), at..self.pos.saturating_add(semi + 1)));
                };
                match code.and_then(char::from_u32).filter(|c| is_xml_char(*c)) {
                    Some(c) => c.to_string(),
                    None => {
                        return Err(self.err("XQST0090", format!("'&{body};' is not a character XML allows"), at..self.pos.saturating_add(semi + 1)));
                    }
                }
            }
        };
        self.pos = self.pos.saturating_add(semi + 1);
        Ok(text)
    }

    fn number(&mut self) -> R<Option<Literal>> {
        self.ws()?;
        let rest = self.rest();
        let bytes = rest.as_bytes();
        let digits = |from: usize| bytes.get(from..).map_or(0, |b| b.iter().take_while(|c| c.is_ascii_digit()).count());
        let int_len = digits(0);
        let mut end = int_len;
        let mut decimal = false;
        if bytes.get(end) == Some(&b'.') && !(int_len > 0 && bytes.get(end + 1) == Some(&b'.')) {
            let frac = digits(end + 1);
            if int_len == 0 && frac == 0 {
                return Ok(None);
            }
            decimal = true;
            end = end + 1 + frac;
        } else if int_len == 0 {
            return Ok(None);
        }
        let mut double = false;
        if matches!(bytes.get(end), Some(b'e' | b'E')) {
            let mut e = end + 1;
            if matches!(bytes.get(e), Some(b'+' | b'-')) {
                e += 1;
            }
            let exp = digits(e);
            if exp == 0 {
                return Err(self.err("XPST0003", "an exponent has no digits", self.pos..self.pos.saturating_add(e)));
            }
            end = e + exp;
            double = true;
        }
        let text = rest.get(..end).unwrap_or("");
        let start = self.pos;
        self.pos = self.pos.saturating_add(end);
        // A number directly followed by a name character (`1div`) is an error (XQuery 3.1 §A.2.2).
        if self.peek().is_some_and(is_name_start) {
            return Err(self.err("XPST0003", "a number is followed directly by a name", start..self.pos.saturating_add(1)));
        }
        let literal = if double {
            Literal::Double(OrderedFloat(text.parse::<f64>().unwrap_or(f64::NAN)))
        } else if decimal {
            match text.parse::<Decimal>() {
                Ok(d) => Literal::Decimal(d),
                Err(_) => return Err(self.err("FOAR0002", "the decimal is too large", start..self.pos)),
            }
        } else {
            match text.parse::<IBig>() {
                Ok(i) => Literal::Integer(i),
                Err(_) => return Err(self.err("XPST0003", "not an integer", start..self.pos)),
            }
        };
        Ok(Some(literal))
    }

    // ---- modules and the prolog ----

    fn module(&mut self) -> R<Module> {
        self.ws()?;
        let mut version = None;
        if self.at_kw_kw("xquery", "version")? || self.at_kw_kw("xquery", "encoding")? {
            let start = self.pos;
            self.expect_kw("xquery")?;
            if self.eat_kw("version")? {
                let v = self.uri_literal()?;
                if !matches!(v.as_str(), "1.0" | "3.0" | "3.1") {
                    return Err(self.err("XQST0031", format!("XQuery version {v} is not supported (3.1 is)"), start..self.pos));
                }
                version = Some(v);
            }
            if self.eat_kw("encoding")? {
                let e = self.uri_literal()?;
                let ok = !e.is_empty()
                    && e.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                    && e.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
                if !ok {
                    return Err(self.err("XQST0087", format!("'{e}' is not an encoding name"), start..self.pos));
                }
            }
            self.expect(";")?;
        }
        let mut library = None;
        if self.at_kw_kw("module", "namespace")? {
            let start = self.pos;
            self.expect_kw("module")?;
            self.expect_kw("namespace")?;
            self.ws()?;
            let Some(prefix) = self.ncname() else { return Err(self.syntax("expected a prefix")) };
            self.expect("=")?;
            let uri = self.uri_literal()?;
            if uri.is_empty() {
                return Err(self.err("XQST0088", "a module namespace cannot be empty", start..self.pos));
            }
            self.expect(";")?;
            self.scope_mut().prefixes.insert(prefix.clone(), uri.clone());
            library = Some((prefix, uri));
        }
        let prolog = self.prolog()?;
        self.ws()?;
        let body = if library.is_some() {
            None
        } else if self.pos >= self.src.len() {
            return Err(self.syntax("the query has no body"));
        } else {
            Some(self.expr()?)
        };
        self.ws()?;
        if self.pos < self.src.len() {
            self.note("','");
            return Err(self.syntax("syntax error"));
        }
        Ok(Module { version, library, prolog, body })
    }

    fn prolog(&mut self) -> R<Vec<Spanned<Decl>>> {
        let mut decls = Vec::new();
        loop {
            self.ws()?;
            let start = self.pos;
            let decl = if self.at_kw("declare")? {
                let save = self.pos;
                self.expect_kw("declare")?;
                self.ws()?;
                match self.declaration(start)? {
                    Some(d) => d,
                    None => {
                        // `declare` was a name in the body (`declare` the element, `declare(…)`).
                        self.pos = save;
                        break;
                    }
                }
            } else if self.at_kw_kw("import", "schema")? || self.at_kw_kw("import", "module")? {
                self.expect_kw("import")?;
                let schema = self.eat_kw("schema")?;
                if !schema {
                    self.expect_kw("module")?;
                }
                let mut prefix = None;
                if self.eat_kw("namespace")? {
                    self.ws()?;
                    let Some(p) = self.ncname() else { return Err(self.syntax("expected a prefix")) };
                    self.expect("=")?;
                    prefix = Some(p);
                } else if schema && self.eat_kw("default")? {
                    self.expect_kw("element")?;
                    self.expect_kw("namespace")?;
                }
                let uri = self.uri_literal()?;
                let mut at = Vec::new();
                if self.eat_kw("at")? {
                    at.push(self.uri_literal()?);
                    while self.eat(",")? {
                        at.push(self.uri_literal()?);
                    }
                }
                if let Some(p) = &prefix {
                    self.bind_prefix(p, &uri, start)?;
                }
                if schema { Decl::SchemaImport { prefix, uri, at } } else { Decl::ModuleImport { prefix, uri, at } }
            } else {
                break;
            };
            self.expect(";")?;
            decls.push(Spanned::new(decl, start..self.pos));
        }
        Ok(decls)
    }

    fn bind_prefix(&mut self, prefix: &str, uri: &str, start: usize) -> R<()> {
        if prefix == "xml" || prefix == "xmlns" {
            return Err(self.err("XQST0070", format!("the prefix '{prefix}' cannot be declared"), start..self.pos));
        }
        if uri == XML_NS || uri == XMLNS_NS {
            return Err(self.err("XQST0070", format!("the namespace '{uri}' cannot be bound to a prefix"), start..self.pos));
        }
        if self.declared_prefixes.iter().any(|p| p == prefix) {
            return Err(self.err("XQST0033", format!("the prefix '{prefix}' is declared twice"), start..self.pos));
        }
        self.declared_prefixes.push(prefix.to_owned());
        if uri.is_empty() {
            self.scope_mut().prefixes.remove(prefix);
        } else {
            self.scope_mut().prefixes.insert(prefix.to_owned(), uri.to_owned());
        }
        Ok(())
    }

    /// After `declare`: one declaration, or `None` when `declare` is not the start of one.
    fn declaration(&mut self, start: usize) -> R<Option<Decl>> {
        if self.eat_kw("default")? {
            if self.eat_kw("element")? {
                self.expect_kw("namespace")?;
                let uri = self.uri_literal()?;
                self.scope_mut().default_element = uri.clone();
                return Ok(Some(Decl::DefaultElementNamespace(uri)));
            }
            if self.eat_kw("function")? {
                self.expect_kw("namespace")?;
                let uri = self.uri_literal()?;
                self.scope_mut().default_function = uri.clone();
                return Ok(Some(Decl::DefaultFunctionNamespace(uri)));
            }
            if self.eat_kw("collation")? {
                return Ok(Some(Decl::DefaultCollation(self.uri_literal()?)));
            }
            if self.eat_kw("order")? {
                self.expect_kw("empty")?;
                let greatest = if self.eat_kw("greatest")? {
                    true
                } else {
                    self.expect_kw("least")?;
                    false
                };
                return Ok(Some(Decl::EmptyOrder(greatest)));
            }
            if self.eat_kw("decimal-format")? {
                return Ok(Some(self.decimal_format(None)?));
            }
            return Err(self.syntax("syntax error in a declaration"));
        }
        if self.eat_kw("namespace")? {
            self.ws()?;
            let Some(prefix) = self.ncname() else { return Err(self.syntax("expected a prefix")) };
            self.expect("=")?;
            let uri = self.uri_literal()?;
            self.bind_prefix(&prefix, &uri, start)?;
            return Ok(Some(Decl::Namespace { prefix, uri }));
        }
        if self.eat_kw("boundary-space")? {
            let preserve = self.preserve_or("strip")?;
            return Ok(Some(Decl::BoundarySpace(preserve)));
        }
        if self.eat_kw("base-uri")? {
            return Ok(Some(Decl::BaseUri(self.uri_literal()?)));
        }
        if self.eat_kw("construction")? {
            let preserve = self.preserve_or("strip")?;
            return Ok(Some(Decl::Construction(preserve)));
        }
        if self.eat_kw("ordering")? {
            let ordered = if self.eat_kw("ordered")? {
                true
            } else {
                self.expect_kw("unordered")?;
                false
            };
            return Ok(Some(Decl::Ordering(ordered)));
        }
        if self.eat_kw("copy-namespaces")? {
            let preserve = self.preserve_or("no-preserve")?;
            self.expect(",")?;
            let inherit = if self.eat_kw("inherit")? {
                true
            } else {
                self.expect_kw("no-inherit")?;
                false
            };
            return Ok(Some(Decl::CopyNamespaces { preserve, inherit }));
        }
        if self.eat_kw("decimal-format")? {
            let name = self.name(NameKind::None)?;
            return Ok(Some(self.decimal_format(Some(name))?));
        }
        if self.at_kw_kw("context", "item")? {
            self.expect_kw("context")?;
            self.expect_kw("item")?;
            let type_ = if self.eat_kw("as")? { Some(self.sequence_type_item_only()?) } else { None };
            let (external, value) = self.var_value()?;
            return Ok(Some(Decl::ContextItem { type_, external, value }));
        }
        if self.at_kw("option")? {
            self.expect_kw("option")?;
            let name = self.name(NameKind::None)?;
            let Some(value) = self.string_literal()? else { return Err(self.syntax("expected the option's value")) };
            return Ok(Some(Decl::Option { name, value }));
        }
        let annotations = self.annotations()?;
        if self.eat_kw("variable")? {
            let var = self.var_name()?;
            let type_ = if self.eat_kw("as")? { Some(self.sequence_type()?) } else { None };
            let (external, value) = self.var_value()?;
            return Ok(Some(Decl::Variable { name: var.name, type_, external, value, annotations }));
        }
        if self.eat_kw("function")? {
            let name_start = self.pos;
            let raw = match self.raw_name()? {
                Some(r) => r,
                None => return Err(self.syntax("expected the function's name")),
            };
            let name = self.resolve(&raw, NameKind::Function)?;
            if raw.prefix.is_none() && raw.uri.is_none() && name.namespace() == FN_NS && self.scope().default_function == FN_NS {
                return Err(self.err("XQST0060", "a declared function needs a namespace (write local:name)", name_start..self.pos));
            }
            let ns = name.namespace();
            if matches!(ns, FN_NS | XS_NS | XML_NS | "http://www.w3.org/2001/XMLSchema-instance")
                || ns == "http://www.w3.org/2005/xpath-functions/math"
                || ns == "http://www.w3.org/2005/xpath-functions/map"
                || ns == "http://www.w3.org/2005/xpath-functions/array"
            {
                return Err(self.err("XQST0045", format!("a function cannot be declared in the namespace '{ns}'"), name_start..self.pos));
            }
            if RESERVED_FUNCTION_NAMES.contains(&name.local_name()) && raw.prefix.is_none() && raw.uri.is_none() {
                return Err(self.err("XPST0003", format!("'{}' is reserved and cannot name a function", name.local_name()), name_start..self.pos));
            }
            let params = self.param_list()?;
            let return_type = if self.eat_kw("as")? { Some(self.sequence_type()?) } else { None };
            let body = if self.eat_kw("external")? { None } else { Some(self.enclosed_or_empty()?) };
            return Ok(Some(Decl::Function(FunctionDecl { name, params, return_type, body, annotations })));
        }
        if annotations.is_empty() {
            return Ok(None);
        }
        Err(self.syntax("expected 'variable' or 'function' after annotations"))
    }

    fn preserve_or(&mut self, other: &str) -> R<bool> {
        if self.eat_kw("preserve")? {
            Ok(true)
        } else {
            self.expect_kw(other)?;
            Ok(false)
        }
    }

    fn decimal_format(&mut self, name: Option<Name>) -> R<Decl> {
        const PROPS: &[&str] = &[
            "decimal-separator",
            "grouping-separator",
            "infinity",
            "minus-sign",
            "NaN",
            "percent",
            "per-mille",
            "zero-digit",
            "digit",
            "pattern-separator",
            "exponent-separator",
        ];
        let mut properties: Vec<(String, String)> = Vec::new();
        loop {
            self.ws()?;
            let save = self.pos;
            let Some(prop) = self.ncname() else { break };
            if !PROPS.contains(&prop.as_str()) {
                self.pos = save;
                break;
            }
            self.expect("=")?;
            let Some(value) = self.string_literal()? else { return Err(self.syntax("expected a string")) };
            if properties.iter().any(|(p, _)| *p == prop) {
                return Err(self.err("XQST0114", format!("the property '{prop}' is given twice"), save..self.pos));
            }
            properties.push((prop, value));
        }
        Ok(Decl::DecimalFormat { name, properties })
    }

    /// `:= value`, `external`, or `external := default`.
    fn var_value(&mut self) -> R<(bool, Option<ExprS>)> {
        if self.eat(":=")? {
            return Ok((false, Some(self.expr_single()?)));
        }
        self.expect_kw("external")?;
        if self.eat(":=")? {
            return Ok((true, Some(self.expr_single()?)));
        }
        Ok((true, None))
    }

    fn annotations(&mut self) -> R<Vec<Annotation>> {
        let mut out = Vec::new();
        while self.eat("%")? {
            let name = self.name(NameKind::Annotation)?;
            let mut values = Vec::new();
            if self.eat("(")? {
                loop {
                    if let Some(s) = self.string_literal()? {
                        values.push(Literal::String(s));
                    } else if let Some(n) = self.number()? {
                        values.push(n);
                    } else {
                        return Err(self.syntax("expected a literal"));
                    }
                    if !self.eat(",")? {
                        break;
                    }
                }
                self.expect(")")?;
            }
            out.push(Annotation { name, values });
        }
        Ok(out)
    }

    fn param_list(&mut self) -> R<Vec<Param>> {
        self.expect("(")?;
        let mut params = Vec::new();
        if !self.eat(")")? {
            loop {
                let var = self.var_name()?;
                let type_ = if self.eat_kw("as")? { Some(self.sequence_type()?) } else { None };
                if params.iter().any(|p: &Param| p.name == var.name) {
                    return Err(self.err("XQST0039", "two parameters have the same name", var.span));
                }
                params.push(Param { name: var.name, type_, span: var.span });
                if !self.eat(",")? {
                    break;
                }
            }
            self.expect(")")?;
        }
        Ok(params)
    }

    // ---- types ----

    fn namespaces(&self) -> xee_xpath_ast::Namespaces {
        let mut map = ahash::HashMap::default();
        for (p, u) in &self.scope().prefixes {
            map.insert(p.clone(), u.clone());
        }
        xee_xpath_ast::Namespaces::new(map, self.scope().default_element.clone(), self.scope().default_function.clone())
    }

    /// Read a sequence type: find where it ends, then let `xee`'s type parser read that text.
    fn sequence_type(&mut self) -> R<SequenceType> {
        self.ws()?;
        let start = self.pos;
        self.enter()?;
        let scanned = self.scan_sequence_type();
        self.leave();
        scanned?;
        let text = self.src.get(start..self.pos).unwrap_or("");
        self.xee_type(text, start)
    }

    /// A context item declaration takes an item type (no occurrence indicator).
    fn sequence_type_item_only(&mut self) -> R<SequenceType> {
        self.ws()?;
        let start = self.pos;
        self.enter()?;
        let scanned = self.scan_item_type();
        self.leave();
        scanned?;
        let text = self.src.get(start..self.pos).unwrap_or("");
        self.xee_type(text, start)
    }

    fn xee_type(&self, text: &str, start: usize) -> R<SequenceType> {
        let namespaces = self.namespaces();
        xee_xpath_ast::parse_sequence_type(text, &namespaces).map_err(|e| {
            let debug = format!("{e:?}");
            let code = if debug.contains("UnknownPrefix") || debug.contains("XPST0081") { "XPST0081" } else { "XPST0003" };
            let code = if debug.contains("XPST0051") || debug.contains("UnknownType") { "XPST0051" } else { code };
            self.err(code, format!("'{text}' is not a sequence type this engine reads"), start..start.saturating_add(text.len()))
        })
    }

    fn scan_sequence_type(&mut self) -> R<()> {
        if self.at_kw_then("empty-sequence", "(")? {
            self.expect_kw("empty-sequence")?;
            self.expect("(")?;
            self.expect(")")?;
            return Ok(());
        }
        self.scan_item_type()?;
        // Occurrence indicators directly after the item type (xgc: occurrence-indicators). Whitespace is not skipped, so that
        // `$x instance of xs:integer + 1` keeps its `+` an operator only when spaced… the spec reads it as an indicator either way.
        let save = self.pos;
        self.ws()?;
        if matches!(self.peek(), Some('?' | '*' | '+')) {
            self.bump();
        } else {
            self.pos = save;
        }
        Ok(())
    }

    fn scan_item_type(&mut self) -> R<()> {
        self.ws()?;
        if self.eat("(")? {
            self.enter()?;
            let r = self.scan_item_type();
            self.leave();
            r?;
            return self.expect(")");
        }
        // Annotated function tests: `%a function(…)`.
        while self.eat("%")? {
            self.name(NameKind::Annotation)?;
            if self.at("(")? {
                self.scan_balanced()?;
            }
        }
        let Some(raw) = self.raw_name()? else { return Err(self.syntax("expected a type")) };
        let local = raw.local.as_str();
        if raw.prefix.is_none() && raw.uri.is_none() && self.at("(")? {
            self.scan_balanced()?;
            if local == "function" && self.eat_kw("as")? {
                self.enter()?;
                let r = self.scan_sequence_type();
                self.leave();
                r?;
            }
        }
        Ok(())
    }

    /// Skip `( … )` with nested parentheses (the inside of a kind test or function test).
    fn scan_balanced(&mut self) -> R<()> {
        let start = self.pos;
        self.expect("(")?;
        let mut level = 1usize;
        while level > 0 {
            self.ws()?;
            match self.bump() {
                Some('(') => {
                    level = level.saturating_add(1);
                    if level > MAX_NESTING {
                        return Err(self.err("XPST0003", "a type is nested too deeply", start..self.pos));
                    }
                }
                Some(')') => level = level.saturating_sub(1),
                Some('"' | '\'') => {
                    self.pos = self.pos.saturating_sub(1);
                    self.string_literal()?;
                }
                Some(_) => {}
                None => return Err(self.err("XPST0003", "a type is not closed with ')'", start..self.src.len())),
            }
        }
        Ok(())
    }

    fn single_type(&mut self) -> R<SingleType> {
        let name = self.name(NameKind::Element)?;
        let save = self.pos;
        self.ws()?;
        let optional = if self.peek() == Some('?') {
            self.bump();
            true
        } else {
            self.pos = save;
            false
        };
        Ok(SingleType { name, optional })
    }

    // ---- expressions ----

    fn spanned(&self, value: Expr, start: usize) -> ExprS {
        Spanned::new(value, start..self.pos)
    }

    pub(crate) fn expr(&mut self) -> R<ExprS> {
        self.ws()?;
        let start = self.pos;
        let first = self.expr_single()?;
        if !self.at(",")? {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.eat(",")? {
            items.push(self.expr_single()?);
        }
        Ok(self.spanned(Expr::Sequence(items), start))
    }

    /// `{ Expr? }` as an expression (an empty one is the empty sequence).
    fn enclosed_or_empty(&mut self) -> R<ExprS> {
        self.ws()?;
        let start = self.pos;
        self.expect("{")?;
        if self.eat("}")? {
            return Ok(self.spanned(Expr::Sequence(Vec::new()), start));
        }
        let e = self.expr()?;
        self.expect("}")?;
        Ok(e)
    }

    fn enclosed_optional(&mut self) -> R<Option<Box<ExprS>>> {
        self.expect("{")?;
        if self.eat("}")? {
            return Ok(None);
        }
        let e = self.expr()?;
        self.expect("}")?;
        Ok(Some(Box::new(e)))
    }

    fn expr_single(&mut self) -> R<ExprS> {
        self.enter()?;
        let r = self.expr_single_inner();
        self.leave();
        r
    }

    fn expr_single_inner(&mut self) -> R<ExprS> {
        self.ws()?;
        if self.at_kw_then("for", "$")? || self.at_kw_then("let", "$")? || self.at_kw_kw("for", "tumbling")? || self.at_kw_kw("for", "sliding")? {
            return self.flwor();
        }
        if self.at_kw_then("some", "$")? || self.at_kw_then("every", "$")? {
            return self.quantified();
        }
        if self.at_kw_then("switch", "(")? {
            return self.switch();
        }
        if self.at_kw_then("typeswitch", "(")? {
            return self.typeswitch();
        }
        if self.at_kw_then("if", "(")? {
            return self.if_expr();
        }
        if self.at_kw_then("try", "{")? {
            return self.try_expr();
        }
        self.or_expr()
    }

    fn flwor(&mut self) -> R<ExprS> {
        let start = self.pos;
        let mut clauses: Vec<Spanned<Clause>> = Vec::new();
        loop {
            self.ws()?;
            let cstart = self.pos;
            if self.at_kw_kw("for", "tumbling")? || self.at_kw_kw("for", "sliding")? {
                self.expect_kw("for")?;
                let w = self.window()?;
                clauses.push(Spanned::new(Clause::Window(Box::new(w)), cstart..self.pos));
            } else if self.at_kw_then("for", "$")? {
                self.expect_kw("for")?;
                loop {
                    let bstart = self.pos;
                    let mut var = self.var_name()?;
                    if self.eat_kw("as")? {
                        var.type_ = Some(self.sequence_type()?);
                    }
                    let allowing_empty = if self.eat_kw("allowing")? {
                        self.expect_kw("empty")?;
                        true
                    } else {
                        false
                    };
                    let position = if self.eat_kw("at")? { Some(self.var_name()?) } else { None };
                    if position.as_ref().is_some_and(|p| p.name == var.name) {
                        return Err(self.err("XQST0089", "the positional variable has the name of the variable it counts", bstart..self.pos));
                    }
                    self.expect_kw("in")?;
                    let expr = self.expr_single()?;
                    clauses.push(Spanned::new(Clause::For { var, allowing_empty, position, expr }, bstart..self.pos));
                    if !self.eat(",")? {
                        break;
                    }
                }
            } else if self.at_kw_then("let", "$")? {
                self.expect_kw("let")?;
                loop {
                    let bstart = self.pos;
                    let mut var = self.var_name()?;
                    if self.eat_kw("as")? {
                        var.type_ = Some(self.sequence_type()?);
                    }
                    self.expect(":=")?;
                    let expr = self.expr_single()?;
                    clauses.push(Spanned::new(Clause::Let { var, expr }, bstart..self.pos));
                    if !self.eat(",")? {
                        break;
                    }
                }
            } else if clauses.is_empty() {
                return Err(self.syntax("expected 'for' or 'let'"));
            } else if self.eat_kw("where")? {
                let e = self.expr_single()?;
                clauses.push(Spanned::new(Clause::Where(e), cstart..self.pos));
            } else if self.at_kw_kw("group", "by")? {
                self.expect_kw("group")?;
                self.expect_kw("by")?;
                let mut specs = Vec::new();
                loop {
                    let mut var = self.var_name()?;
                    let mut expr = None;
                    if self.eat_kw("as")? {
                        var.type_ = Some(self.sequence_type()?);
                        self.expect(":=")?;
                        expr = Some(self.expr_single()?);
                    } else if self.eat(":=")? {
                        expr = Some(self.expr_single()?);
                    }
                    let collation = if self.eat_kw("collation")? { Some(self.uri_literal()?) } else { None };
                    specs.push(GroupSpec { var, expr, collation });
                    if !self.eat(",")? {
                        break;
                    }
                }
                clauses.push(Spanned::new(Clause::GroupBy(specs), cstart..self.pos));
            } else if self.at_kw_kw("order", "by")? || self.at_kw_kw("stable", "order")? {
                let stable = self.eat_kw("stable")?;
                self.expect_kw("order")?;
                self.expect_kw("by")?;
                let mut specs = Vec::new();
                loop {
                    let expr = self.expr_single()?;
                    let descending = if self.eat_kw("descending")? {
                        true
                    } else {
                        self.eat_kw("ascending")?;
                        false
                    };
                    let empty_greatest = if self.eat_kw("empty")? {
                        if self.eat_kw("greatest")? {
                            Some(true)
                        } else {
                            self.expect_kw("least")?;
                            Some(false)
                        }
                    } else {
                        None
                    };
                    let collation = if self.eat_kw("collation")? { Some(self.uri_literal()?) } else { None };
                    specs.push(OrderSpec { expr, descending, empty_greatest, collation });
                    if !self.eat(",")? {
                        break;
                    }
                }
                clauses.push(Spanned::new(Clause::OrderBy { stable, specs }, cstart..self.pos));
            } else if self.at_kw_then("count", "$")? {
                self.expect_kw("count")?;
                let var = self.var_name()?;
                clauses.push(Spanned::new(Clause::Count(var), cstart..self.pos));
            } else if self.eat_kw("return")? {
                let ret = self.expr_single()?;
                return Ok(self.spanned(Expr::Flwor { clauses, ret: Box::new(ret) }, start));
            } else {
                return Err(self.syntax("expected a FLWOR clause or 'return'"));
            }
        }
    }

    fn window(&mut self) -> R<Window> {
        let tumbling = if self.eat_kw("tumbling")? {
            true
        } else {
            self.expect_kw("sliding")?;
            false
        };
        self.expect_kw("window")?;
        let mut var = self.var_name()?;
        if self.eat_kw("as")? {
            var.type_ = Some(self.sequence_type()?);
        }
        self.expect_kw("in")?;
        let expr = self.expr_single()?;
        self.expect_kw("start")?;
        let start = self.window_condition()?;
        let mut end = None;
        if self.at_kw("only")? || self.at_kw("end")? {
            let only = self.eat_kw("only")?;
            self.expect_kw("end")?;
            end = Some((only, self.window_condition()?));
        } else if !tumbling {
            return Err(self.syntax("a sliding window needs an 'end' condition"));
        }
        Ok(Window { tumbling, var, expr, start, end })
    }

    fn window_condition(&mut self) -> R<WindowCondition> {
        let current = if self.at("$")? { Some(self.var_name()?) } else { None };
        let position = if self.eat_kw("at")? { Some(self.var_name()?) } else { None };
        let previous = if self.eat_kw("previous")? { Some(self.var_name()?) } else { None };
        let next = if self.eat_kw("next")? { Some(self.var_name()?) } else { None };
        self.expect_kw("when")?;
        let when = self.expr_single()?;
        Ok(WindowCondition { current, position, previous, next, when })
    }

    fn quantified(&mut self) -> R<ExprS> {
        let start = self.pos;
        let every = if self.eat_kw("every")? {
            true
        } else {
            self.expect_kw("some")?;
            false
        };
        let mut bindings = Vec::new();
        loop {
            let mut var = self.var_name()?;
            if self.eat_kw("as")? {
                var.type_ = Some(self.sequence_type()?);
            }
            self.expect_kw("in")?;
            let e = self.expr_single()?;
            bindings.push((var, e));
            if !self.eat(",")? {
                break;
            }
        }
        self.expect_kw("satisfies")?;
        let satisfies = self.expr_single()?;
        Ok(self.spanned(Expr::Quantified { every, bindings, satisfies: Box::new(satisfies) }, start))
    }

    fn switch(&mut self) -> R<ExprS> {
        let start = self.pos;
        self.expect_kw("switch")?;
        self.expect("(")?;
        let operand = self.expr()?;
        self.expect(")")?;
        let mut cases = Vec::new();
        while self.at_kw("case")? {
            let mut values = Vec::new();
            while self.eat_kw("case")? {
                values.push(self.expr_single()?);
            }
            self.expect_kw("return")?;
            let ret = self.expr_single()?;
            cases.push((values, ret));
        }
        if cases.is_empty() {
            return Err(self.syntax("a switch needs a 'case'"));
        }
        self.expect_kw("default")?;
        self.expect_kw("return")?;
        let default = self.expr_single()?;
        Ok(self.spanned(Expr::Switch { operand: Box::new(operand), cases, default: Box::new(default) }, start))
    }

    fn typeswitch(&mut self) -> R<ExprS> {
        let start = self.pos;
        self.expect_kw("typeswitch")?;
        self.expect("(")?;
        let operand = self.expr()?;
        self.expect(")")?;
        let mut cases = Vec::new();
        while self.eat_kw("case")? {
            let var = if self.at("$")? {
                let v = self.var_name()?;
                self.expect_kw("as")?;
                Some(v)
            } else {
                None
            };
            let mut types = vec![self.sequence_type()?];
            while self.eat("|")? {
                types.push(self.sequence_type()?);
            }
            self.expect_kw("return")?;
            let ret = self.expr_single()?;
            cases.push(TypeCase { var, types, ret });
        }
        if cases.is_empty() {
            return Err(self.syntax("a typeswitch needs a 'case'"));
        }
        self.expect_kw("default")?;
        let default_var = if self.at("$")? { Some(self.var_name()?) } else { None };
        self.expect_kw("return")?;
        let default = self.expr_single()?;
        Ok(self.spanned(Expr::Typeswitch { operand: Box::new(operand), cases, default_var, default: Box::new(default) }, start))
    }

    fn if_expr(&mut self) -> R<ExprS> {
        let start = self.pos;
        self.expect_kw("if")?;
        self.expect("(")?;
        let condition = self.expr()?;
        self.expect(")")?;
        self.expect_kw("then")?;
        let then = self.expr_single()?;
        self.expect_kw("else")?;
        let else_ = self.expr_single()?;
        Ok(self.spanned(Expr::If { condition: Box::new(condition), then: Box::new(then), else_: Box::new(else_) }, start))
    }

    fn try_expr(&mut self) -> R<ExprS> {
        let start = self.pos;
        self.expect_kw("try")?;
        let body = self.enclosed_or_empty()?;
        let mut catches = Vec::new();
        while self.at_kw("catch")? {
            let cstart = self.pos;
            self.expect_kw("catch")?;
            let mut tests = vec![self.name_test(NameKind::None)?];
            while self.eat("|")? {
                tests.push(self.name_test(NameKind::None)?);
            }
            let body = self.enclosed_or_empty()?;
            catches.push(Catch { tests, body, span: cstart..self.pos });
        }
        if catches.is_empty() {
            return Err(self.syntax("a try needs a 'catch'"));
        }
        Ok(self.spanned(Expr::Try { body: Box::new(body), catches }, start))
    }

    fn binary_loop(&mut self, next: fn(&mut Self) -> R<ExprS>, ops: &[(&str, BinOp, bool)]) -> R<ExprS> {
        self.ws()?;
        let start = self.pos;
        let mut left = next(self)?;
        'outer: loop {
            for (token, op, keyword) in ops {
                let hit = if *keyword { self.eat_kw(token)? } else { self.eat_symbol(token)? };
                if hit {
                    let right = next(self)?;
                    left = self.spanned(Expr::Binary { op: *op, left: Box::new(left), right: Box::new(right) }, start);
                    continue 'outer;
                }
            }
            return Ok(left);
        }
    }

    /// Read an operator symbol, not when it is the start of a longer one (`|` of `||`, `<` of `<=`, `!` of `!=`).
    fn eat_symbol(&mut self, s: &str) -> R<bool> {
        self.ws()?;
        let rest = self.rest();
        if !rest.starts_with(s) {
            self.note(&format!("'{s}'"));
            return Ok(false);
        }
        let after = rest.get(s.len()..).unwrap_or("");
        let longer = match s {
            "|" => after.starts_with('|'),
            "!" => after.starts_with('='),
            "<" => after.starts_with('<') || after.starts_with('='),
            ">" => after.starts_with('>') || after.starts_with('='),
            "=" => after.starts_with('>'),
            "/" => after.starts_with('/'),
            _ => false,
        };
        if longer {
            return Ok(false);
        }
        self.pos = self.pos.saturating_add(s.len());
        Ok(true)
    }

    fn or_expr(&mut self) -> R<ExprS> {
        self.binary_loop(Self::and_expr, &[("or", BinOp::Or, true)])
    }

    fn and_expr(&mut self) -> R<ExprS> {
        self.binary_loop(Self::comparison_expr, &[("and", BinOp::And, true)])
    }

    fn comparison_expr(&mut self) -> R<ExprS> {
        self.ws()?;
        let start = self.pos;
        let left = self.concat_expr()?;
        const OPS: &[(&str, BinOp, bool)] = &[
            ("=", BinOp::GenEq, false),
            ("!=", BinOp::GenNe, false),
            ("<=", BinOp::GenLe, false),
            ("<<", BinOp::Precedes, false),
            ("<", BinOp::GenLt, false),
            (">=", BinOp::GenGe, false),
            (">>", BinOp::Follows, false),
            (">", BinOp::GenGt, false),
            ("eq", BinOp::ValueEq, true),
            ("ne", BinOp::ValueNe, true),
            ("lt", BinOp::ValueLt, true),
            ("le", BinOp::ValueLe, true),
            ("gt", BinOp::ValueGt, true),
            ("ge", BinOp::ValueGe, true),
            ("is", BinOp::Is, true),
        ];
        for (token, op, keyword) in OPS {
            let hit = if *keyword { self.eat_kw(token)? } else { self.eat_symbol(token)? };
            if hit {
                let right = self.concat_expr()?;
                return Ok(self.spanned(Expr::Binary { op: *op, left: Box::new(left), right: Box::new(right) }, start));
            }
        }
        Ok(left)
    }

    fn concat_expr(&mut self) -> R<ExprS> {
        self.binary_loop(Self::range_expr, &[("||", BinOp::Concat, false)])
    }

    fn range_expr(&mut self) -> R<ExprS> {
        self.ws()?;
        let start = self.pos;
        let left = self.additive_expr()?;
        if self.eat_kw("to")? {
            let right = self.additive_expr()?;
            return Ok(self.spanned(Expr::Binary { op: BinOp::Range, left: Box::new(left), right: Box::new(right) }, start));
        }
        Ok(left)
    }

    fn additive_expr(&mut self) -> R<ExprS> {
        self.binary_loop(Self::multiplicative_expr, &[("+", BinOp::Add, false), ("-", BinOp::Sub, false)])
    }

    fn multiplicative_expr(&mut self) -> R<ExprS> {
        self.binary_loop(
            Self::union_expr,
            &[("*", BinOp::Mul, false), ("div", BinOp::Div, true), ("idiv", BinOp::IntDiv, true), ("mod", BinOp::Mod, true)],
        )
    }

    fn union_expr(&mut self) -> R<ExprS> {
        self.binary_loop(Self::intersect_expr, &[("union", BinOp::Union, true), ("|", BinOp::Union, false)])
    }

    fn intersect_expr(&mut self) -> R<ExprS> {
        self.binary_loop(Self::instanceof_expr, &[("intersect", BinOp::Intersect, true), ("except", BinOp::Except, true)])
    }

    fn instanceof_expr(&mut self) -> R<ExprS> {
        self.ws()?;
        let start = self.pos;
        let e = self.treat_expr()?;
        if self.at_kw_kw("instance", "of")? {
            self.expect_kw("instance")?;
            self.expect_kw("of")?;
            let type_ = self.sequence_type()?;
            return Ok(self.spanned(Expr::InstanceOf { expr: Box::new(e), type_ }, start));
        }
        Ok(e)
    }

    fn treat_expr(&mut self) -> R<ExprS> {
        self.ws()?;
        let start = self.pos;
        let e = self.castable_expr()?;
        if self.at_kw_kw("treat", "as")? {
            self.expect_kw("treat")?;
            self.expect_kw("as")?;
            let type_ = self.sequence_type()?;
            return Ok(self.spanned(Expr::Treat { expr: Box::new(e), type_ }, start));
        }
        Ok(e)
    }

    fn castable_expr(&mut self) -> R<ExprS> {
        self.ws()?;
        let start = self.pos;
        let e = self.cast_expr()?;
        if self.at_kw_kw("castable", "as")? {
            self.expect_kw("castable")?;
            self.expect_kw("as")?;
            let type_ = self.single_type()?;
            return Ok(self.spanned(Expr::Castable { expr: Box::new(e), type_ }, start));
        }
        Ok(e)
    }

    fn cast_expr(&mut self) -> R<ExprS> {
        self.ws()?;
        let start = self.pos;
        let e = self.arrow_expr()?;
        if self.at_kw_kw("cast", "as")? {
            self.expect_kw("cast")?;
            self.expect_kw("as")?;
            let type_ = self.single_type()?;
            return Ok(self.spanned(Expr::Cast { expr: Box::new(e), type_ }, start));
        }
        Ok(e)
    }

    fn arrow_expr(&mut self) -> R<ExprS> {
        self.ws()?;
        let start = self.pos;
        let mut e = self.unary_expr()?;
        while self.eat("=>")? {
            self.ws()?;
            let function = if self.at("$")? {
                let vstart = self.pos;
                let v = self.var_name()?;
                ArrowTarget::Expr(Spanned::new(Expr::VarRef(v.name), vstart..self.pos))
            } else if self.at("(")? {
                ArrowTarget::Expr(self.parenthesized()?)
            } else {
                ArrowTarget::Name(self.name(NameKind::Function)?)
            };
            let args = self.argument_list()?;
            e = self.spanned(Expr::Arrow { base: Box::new(e), function: Box::new(function), args }, start);
        }
        Ok(e)
    }

    fn unary_expr(&mut self) -> R<ExprS> {
        self.ws()?;
        let start = self.pos;
        let mut signs = Vec::new();
        loop {
            self.ws()?;
            match self.peek() {
                Some('-') => {
                    self.bump();
                    signs.push((true, self.pos.saturating_sub(1)));
                }
                Some('+') => {
                    self.bump();
                    signs.push((false, self.pos.saturating_sub(1)));
                }
                _ => break,
            }
            if signs.len() > MAX_TREE_DEPTH {
                return Err(self.err("XPST0003", "too many signs", start..self.pos));
            }
        }
        let mut e = self.value_expr()?;
        for (minus, at) in signs.into_iter().rev() {
            e = Spanned::new(Expr::Unary { minus, expr: Box::new(e) }, at..self.pos);
        }
        Ok(e)
    }

    fn value_expr(&mut self) -> R<ExprS> {
        self.ws()?;
        let start = self.pos;
        if self.at_kw_then("validate", "{")?
            || self.at_kw_kw("validate", "lax")?
            || self.at_kw_kw("validate", "strict")?
            || self.at_kw_kw("validate", "type")?
        {
            self.expect_kw("validate")?;
            if self.eat_kw("type")? {
                self.name(NameKind::Element)?;
            } else if !self.eat_kw("lax")? {
                self.eat_kw("strict")?;
            }
            let body = self.enclosed_or_empty()?;
            return Ok(self.spanned(Expr::Validate(Box::new(body)), start));
        }
        if self.rest().starts_with("(#") {
            // Extension expressions: pragmas this engine does not know are ignored, so the expression itself is evaluated.
            while self.at("(#")? {
                let pstart = self.pos;
                self.pos = self.pos.saturating_add(2);
                self.ws()?;
                self.name(NameKind::None)?;
                match self.rest().find("#)") {
                    Some(i) => self.pos = self.pos.saturating_add(i + 2),
                    None => return Err(self.err("XPST0003", "a pragma is not closed with '#)'", pstart..self.src.len())),
                }
            }
            self.ws()?;
            self.expect("{")?;
            if self.eat("}")? {
                return Err(self.err("XQST0079", "an extension expression needs an expression when its pragmas are not known", start..self.pos));
            }
            let e = self.expr()?;
            self.expect("}")?;
            return Ok(e);
        }
        self.simple_map_expr()
    }

    fn simple_map_expr(&mut self) -> R<ExprS> {
        self.ws()?;
        let start = self.pos;
        let mut left = self.path_expr()?;
        while self.eat_symbol("!")? {
            let right = self.path_expr()?;
            left = self.spanned(Expr::SimpleMap { left: Box::new(left), right: Box::new(right) }, start);
        }
        Ok(left)
    }

    fn descendant_or_self(&self, at: usize) -> ExprS {
        Spanned::new(
            Expr::AxisStep { axis: xast::Axis::DescendantOrSelf, test: xast::NodeTest::KindTest(xast::KindTest::Any), predicates: Vec::new() },
            at..at.saturating_add(2),
        )
    }

    /// Whether a relative path can start here (after a leading `/`).
    fn at_step_start(&mut self) -> R<bool> {
        self.ws()?;
        Ok(match self.peek() {
            Some(c) if is_name_start(c) => true,
            Some('*' | '@' | '.' | '$' | '(' | '"' | '\'' | '<' | '?' | '[' | '%' | '`') => true,
            Some(c) => c.is_ascii_digit(),
            None => false,
        })
    }

    fn path_expr(&mut self) -> R<ExprS> {
        self.ws()?;
        let start = self.pos;
        if self.rest().starts_with("//") {
            self.pos = self.pos.saturating_add(2);
            let root = Spanned::new(Expr::Root, start..start.saturating_add(2));
            let dos = self.descendant_or_self(start);
            let first = Spanned::new(Expr::Path { left: Box::new(root), right: Box::new(dos) }, start..self.pos);
            let step = self.step_expr()?;
            let left = self.spanned(Expr::Path { left: Box::new(first), right: Box::new(step) }, start);
            return self.relative_rest(left, start);
        }
        if self.rest().starts_with('/') {
            self.pos = self.pos.saturating_add(1);
            let root = Spanned::new(Expr::Root, start..start.saturating_add(1));
            if !self.at_step_start()? {
                return Ok(root);
            }
            let step = self.step_expr()?;
            let left = self.spanned(Expr::Path { left: Box::new(root), right: Box::new(step) }, start);
            return self.relative_rest(left, start);
        }
        let first = self.step_expr()?;
        self.relative_rest(first, start)
    }

    fn relative_rest(&mut self, mut left: ExprS, start: usize) -> R<ExprS> {
        loop {
            self.ws()?;
            if self.rest().starts_with("//") {
                let at = self.pos;
                self.pos = self.pos.saturating_add(2);
                let dos = self.descendant_or_self(at);
                left = self.spanned(Expr::Path { left: Box::new(left), right: Box::new(dos) }, start);
                let step = self.step_expr()?;
                left = self.spanned(Expr::Path { left: Box::new(left), right: Box::new(step) }, start);
            } else if self.rest().starts_with('/') {
                self.pos = self.pos.saturating_add(1);
                let step = self.step_expr()?;
                left = self.spanned(Expr::Path { left: Box::new(left), right: Box::new(step) }, start);
            } else {
                return Ok(left);
            }
        }
    }

    fn step_expr(&mut self) -> R<ExprS> {
        self.enter()?;
        let r = self.step_expr_inner();
        self.leave();
        r
    }

    fn step_expr_inner(&mut self) -> R<ExprS> {
        self.ws()?;
        let start = self.pos;
        // `axis::test`
        for (axis_name, axis) in AXES {
            if self.at_kw(axis_name)? {
                let save = self.pos;
                self.pos = self.pos.saturating_add(axis_name.len());
                if self.eat("::")? {
                    let kind = if matches!(axis, xast::Axis::Attribute) { NameKind::None } else { NameKind::Element };
                    let test = self.node_test(kind)?;
                    return self.axis_step(axis.clone(), test, start);
                }
                self.pos = save;
            }
        }
        if self.eat("@")? {
            let test = self.node_test(NameKind::None)?;
            return self.axis_step(xast::Axis::Attribute, test, start);
        }
        if self.rest().starts_with("..") {
            self.pos = self.pos.saturating_add(2);
            return self.axis_step(xast::Axis::Parent, xast::NodeTest::KindTest(xast::KindTest::Any), start);
        }
        if let Some(primary) = self.primary()? {
            return self.postfixes(primary, start);
        }
        // An abbreviated step: a name test or a kind test on the child axis (the attribute axis for `attribute(…)`).
        self.ws()?;
        let test = self.node_test(NameKind::Element)?;
        let axis = match &test {
            xast::NodeTest::KindTest(xast::KindTest::Attribute(_) | xast::KindTest::SchemaAttribute(_)) => xast::Axis::Attribute,
            xast::NodeTest::KindTest(xast::KindTest::NamespaceNode) => xast::Axis::Namespace,
            _ => xast::Axis::Child,
        };
        self.axis_step(axis, test, start)
    }

    fn axis_step(&mut self, axis: xast::Axis, test: xast::NodeTest, start: usize) -> R<ExprS> {
        let mut predicates = Vec::new();
        while self.eat("[")? {
            predicates.push(self.expr()?);
            self.expect("]")?;
        }
        Ok(self.spanned(Expr::AxisStep { axis, test, predicates }, start))
    }

    fn node_test(&mut self, kind: NameKind) -> R<xast::NodeTest> {
        self.ws()?;
        let start = self.pos;
        // a kind test
        for k in KIND_TESTS {
            if self.at_kw_then(k, "(")? {
                self.pos = self.pos.saturating_add(k.len());
                self.scan_balanced()?;
                let text = self.src.get(start..self.pos).unwrap_or("");
                let type_ = self.xee_type(text, start)?;
                return match type_ {
                    xast::SequenceType::Item(xast::Item { item_type: xast::ItemType::KindTest(k), .. }) => Ok(xast::NodeTest::KindTest(k)),
                    _ => Err(self.err("XPST0003", "not a kind test", start..self.pos)),
                };
            }
        }
        Ok(xast::NodeTest::NameTest(self.name_test(kind)?))
    }

    fn name_test(&mut self, kind: NameKind) -> R<xast::NameTest> {
        self.ws()?;
        let start = self.pos;
        if self.rest().starts_with("*:") && self.peek_at(2).is_some_and(is_name_start) {
            self.pos = self.pos.saturating_add(2);
            let Some(local) = self.ncname() else { return Err(self.syntax("expected a name")) };
            return Ok(xast::NameTest::LocalName(local));
        }
        if self.rest().starts_with('*') {
            self.pos = self.pos.saturating_add(1);
            return Ok(xast::NameTest::Star);
        }
        if self.rest().starts_with("Q{")
            && let Some(close) = self.rest().find('}')
            && self.rest().get(close + 1..).is_some_and(|s| s.starts_with('*'))
        {
            let uri = self.rest().get(2..close).unwrap_or("").to_owned();
            self.pos = self.pos.saturating_add(close + 2);
            return Ok(xast::NameTest::Namespace(normalize_space(&uri)));
        }
        // `prefix:*`
        let save = self.pos;
        if let Some(prefix) = self.ncname()
            && self.rest().starts_with(":*")
        {
            self.pos = self.pos.saturating_add(2);
            let Some(uri) = self.scope().prefixes.get(&prefix).cloned() else {
                return Err(self.err("XPST0081", format!("the prefix '{prefix}' is not declared"), start..self.pos));
            };
            return Ok(xast::NameTest::Namespace(uri));
        }
        self.pos = save;
        let Some(raw) = self.raw_name()? else { return Err(self.syntax("expected an expression")) };
        let name = self.resolve(&raw, kind)?;
        Ok(xast::NameTest::Name(xee_xpath_ast::span::Spanned::new(name, (raw.span.start..raw.span.end).into())))
    }

    fn postfixes(&mut self, mut e: ExprS, start: usize) -> R<ExprS> {
        loop {
            self.ws()?;
            if self.eat("[")? {
                let p = self.expr()?;
                self.expect("]")?;
                e = self.spanned(Expr::Filter { base: Box::new(e), predicate: Box::new(p) }, start);
            } else if self.at("(")? && !self.rest().starts_with("(:") && !self.rest().starts_with("(#") {
                let args = self.argument_list()?;
                e = self.spanned(Expr::DynamicCall { function: Box::new(e), args }, start);
            } else if self.at("?")? && !self.rest().starts_with("??") {
                self.pos = self.pos.saturating_add(1);
                let key = self.key_specifier()?;
                e = self.spanned(Expr::Lookup { base: Box::new(e), key }, start);
            } else {
                return Ok(e);
            }
        }
    }

    fn key_specifier(&mut self) -> R<Key> {
        self.ws()?;
        if self.eat("*")? {
            return Ok(Key::Star);
        }
        if self.at("(")? {
            return Ok(Key::Expr(Box::new(self.parenthesized()?)));
        }
        if self.peek().is_some_and(|c| c.is_ascii_digit()) {
            let start = self.pos;
            let digits: String = self.rest().chars().take_while(|c| c.is_ascii_digit()).collect();
            self.pos = self.pos.saturating_add(digits.len());
            return match digits.parse::<IBig>() {
                Ok(i) => Ok(Key::Integer(i)),
                Err(_) => Err(self.err("XPST0003", "not an integer", start..self.pos)),
            };
        }
        match self.ncname() {
            Some(n) => Ok(Key::Name(n)),
            None => Err(self.syntax("expected a key after '?'")),
        }
    }

    fn argument_list(&mut self) -> R<Vec<Arg>> {
        self.expect("(")?;
        let mut args = Vec::new();
        if self.eat(")")? {
            return Ok(args);
        }
        loop {
            self.ws()?;
            let at = self.pos;
            if self.rest().starts_with('?') {
                let save = self.pos;
                self.pos = self.pos.saturating_add(1);
                if self.at(",")? || self.at(")")? {
                    args.push(Arg::Placeholder(at..at.saturating_add(1)));
                } else {
                    self.pos = save;
                    args.push(Arg::Expr(self.expr_single()?));
                }
            } else {
                args.push(Arg::Expr(self.expr_single()?));
            }
            if args.len() > usize::from(u8::MAX) {
                return Err(self.err("XPST0003", "a function call has more than 255 arguments", at..self.pos));
            }
            if !self.eat(",")? {
                break;
            }
        }
        self.expect(")")?;
        Ok(args)
    }

    fn parenthesized(&mut self) -> R<ExprS> {
        self.ws()?;
        let start = self.pos;
        self.expect("(")?;
        if self.eat(")")? {
            return Ok(self.spanned(Expr::Sequence(Vec::new()), start));
        }
        let e = self.expr()?;
        self.expect(")")?;
        // (Keep the parentheses in the span: `(1, 2)[1]` reads better marked whole.)
        Ok(Spanned::new(e.value, start..self.pos))
    }

    /// A primary expression, or `None` (nothing read) when there is none here.
    fn primary(&mut self) -> R<Option<ExprS>> {
        self.ws()?;
        let start = self.pos;
        let Some(c) = self.peek() else {
            self.note("an expression");
            return Ok(None);
        };
        if c.is_ascii_digit() || (c == '.' && self.peek_at(1).is_some_and(|d| d.is_ascii_digit())) {
            let Some(n) = self.number()? else { return Ok(None) };
            return Ok(Some(self.spanned(Expr::Literal(n), start)));
        }
        if c == '"' || c == '\'' {
            let Some(s) = self.string_literal()? else { return Ok(None) };
            return Ok(Some(self.spanned(Expr::Literal(Literal::String(s)), start)));
        }
        if c == '$' {
            let v = self.var_name()?;
            return Ok(Some(self.spanned(Expr::VarRef(v.name), start)));
        }
        if c == '(' {
            return Ok(Some(self.parenthesized()?));
        }
        if c == '.' && self.peek_at(1) != Some('.') {
            self.bump();
            return Ok(Some(self.spanned(Expr::ContextItem, start)));
        }
        if c == '<' {
            return Ok(Some(self.direct_constructor()?));
        }
        if self.rest().starts_with("``[") {
            return Ok(Some(self.string_constructor()?));
        }
        if c == '[' {
            self.bump();
            let mut members = Vec::new();
            if !self.eat("]")? {
                loop {
                    members.push(self.expr_single()?);
                    if !self.eat(",")? {
                        break;
                    }
                }
                self.expect("]")?;
            }
            return Ok(Some(self.spanned(Expr::SquareArray(members), start)));
        }
        if c == '?' {
            self.bump();
            let key = self.key_specifier()?;
            return Ok(Some(self.spanned(Expr::UnaryLookup(key), start)));
        }
        if c == '%' {
            let annotations = self.annotations()?;
            self.expect_kw("function")?;
            return Ok(Some(self.inline_function(annotations, start)?));
        }
        if !(is_name_start(c) || self.rest().starts_with("Q{")) {
            self.note("an expression");
            return Ok(None);
        }
        // Keyword-led primaries.
        if self.at_kw_then("function", "(")? {
            self.expect_kw("function")?;
            return Ok(Some(self.inline_function(Vec::new(), start)?));
        }
        if self.at_kw_then("map", "{")? {
            self.expect_kw("map")?;
            self.expect("{")?;
            let mut entries = Vec::new();
            if !self.eat("}")? {
                loop {
                    let k = self.expr_single()?;
                    self.expect(":")?;
                    let v = self.expr_single()?;
                    entries.push((k, v));
                    if !self.eat(",")? {
                        break;
                    }
                }
                self.expect("}")?;
            }
            return Ok(Some(self.spanned(Expr::MapConstructor(entries), start)));
        }
        if self.at_kw_then("array", "{")? {
            self.expect_kw("array")?;
            let e = self.enclosed_optional()?;
            return Ok(Some(self.spanned(Expr::CurlyArray(e), start)));
        }
        if self.at_kw_then("ordered", "{")? || self.at_kw_then("unordered", "{")? {
            if !self.eat_kw("ordered")? {
                self.expect_kw("unordered")?;
            }
            let e = self.enclosed_or_empty()?;
            return Ok(Some(Spanned::new(e.value, start..self.pos)));
        }
        if let Some(e) = self.computed_constructor(start)? {
            return Ok(Some(e));
        }
        // A function call or a named function reference.
        let save = self.pos;
        let Some(raw) = self.raw_name()? else { return Ok(None) };
        let unprefixed = raw.prefix.is_none() && raw.uri.is_none();
        if self.at("#")? && !self.rest().starts_with("#)") {
            self.pos = self.pos.saturating_add(1);
            self.ws()?;
            let digits: String = self.rest().chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                return Err(self.syntax("expected the arity after '#'"));
            }
            self.pos = self.pos.saturating_add(digits.len());
            let arity = digits.parse::<u8>().map_err(|_| self.err("XPST0017", "the arity is more than 255", start..self.pos))?;
            let name = self.resolve(&raw, NameKind::Function)?;
            return Ok(Some(self.spanned(Expr::NamedFunctionRef { name, arity }, start)));
        }
        if self.at("(")? && !self.rest().starts_with("(:") && !self.rest().starts_with("(#") {
            if unprefixed && RESERVED_FUNCTION_NAMES.contains(&raw.local.as_str()) {
                self.pos = save;
                return Ok(None);
            }
            let name = self.resolve(&raw, NameKind::Function)?;
            let args = self.argument_list()?;
            return Ok(Some(self.spanned(Expr::FunctionCall { name, args }, start)));
        }
        self.pos = save;
        Ok(None)
    }

    fn inline_function(&mut self, annotations: Vec<Annotation>, start: usize) -> R<ExprS> {
        let params = self.param_list()?;
        let return_type = if self.eat_kw("as")? { Some(self.sequence_type()?) } else { None };
        let body = self.enclosed_or_empty()?;
        Ok(self.spanned(Expr::InlineFunction { params, return_type, body: Box::new(body), annotations }, start))
    }

    /// `document {…}`, `element n {…}`, `attribute {…} {…}`, `text {…}`, `comment {…}`, `processing-instruction n {…}`,
    /// `namespace p {…}`, or `None`.
    fn computed_constructor(&mut self, start: usize) -> R<Option<ExprS>> {
        if self.at_kw_then("document", "{")? {
            self.expect_kw("document")?;
            let e = self.enclosed_or_empty()?;
            return Ok(Some(self.spanned(Expr::CompDocument(Box::new(e)), start)));
        }
        if self.at_kw_then("text", "{")? {
            self.expect_kw("text")?;
            let e = self.enclosed_or_empty()?;
            return Ok(Some(self.spanned(Expr::CompText(Box::new(e)), start)));
        }
        if self.at_kw_then("comment", "{")? {
            self.expect_kw("comment")?;
            let e = self.enclosed_or_empty()?;
            return Ok(Some(self.spanned(Expr::CompComment(Box::new(e)), start)));
        }
        for (kw, which) in [("element", 0u8), ("attribute", 1), ("processing-instruction", 2), ("namespace", 3)] {
            if !self.at_kw(kw)? {
                continue;
            }
            let save = self.pos;
            self.pos = self.pos.saturating_add(kw.len());
            let name = if self.at("{")? {
                self.expect("{")?;
                let e = self.expr()?;
                self.expect("}")?;
                CompName::Expr(Box::new(e))
            } else {
                self.ws()?;
                let name_save = self.pos;
                let name = match which {
                    0 | 1 => match self.raw_name()? {
                        Some(raw) if self.at("{")? => {
                            let kind = if which == 0 { NameKind::Element } else { NameKind::None };
                            Some(CompName::Name(self.resolve(&raw, kind)?))
                        }
                        _ => None,
                    },
                    _ => match self.ncname() {
                        Some(n) if self.at("{")? => Some(CompName::NcName(n)),
                        _ => None,
                    },
                };
                match name {
                    Some(n) => n,
                    None => {
                        let _ = name_save;
                        self.pos = save;
                        continue;
                    }
                }
            };
            let content = self.enclosed_optional()?;
            let e = match which {
                0 => Expr::CompElement { name, content },
                1 => Expr::CompAttribute { name, content },
                2 => Expr::CompPi { target: name, content },
                _ => Expr::CompNamespace { prefix: name, uri: content },
            };
            return Ok(Some(self.spanned(e, start)));
        }
        Ok(None)
    }

    /// ``` ``[ … `{ expr }` … ]`` ```
    fn string_constructor(&mut self) -> R<ExprS> {
        let start = self.pos;
        self.pos = self.pos.saturating_add(3);
        let mut parts = Vec::new();
        let mut text = String::new();
        loop {
            if self.rest().starts_with("]``") {
                self.pos = self.pos.saturating_add(3);
                break;
            }
            if self.rest().starts_with("`{") {
                self.pos = self.pos.saturating_add(2);
                if !text.is_empty() {
                    parts.push(StringPart::Text(std::mem::take(&mut text)));
                }
                self.enter()?;
                let e = if self.at("}`")? { None } else { Some(self.expr()?) };
                self.leave();
                self.ws()?;
                if !self.rest().starts_with("}`") {
                    self.note("'}`'");
                    return Err(self.syntax("an interpolation is not closed"));
                }
                self.pos = self.pos.saturating_add(2);
                parts.push(StringPart::Expr(e));
                continue;
            }
            match self.bump() {
                Some('\r') => {
                    if self.peek() == Some('\n') {
                        self.bump();
                    }
                    text.push('\n');
                }
                Some(c) => text.push(c),
                None => return Err(self.err("XPST0003", "a string constructor is not closed with ']``'", start..self.src.len())),
            }
        }
        if !text.is_empty() {
            parts.push(StringPart::Text(text));
        }
        Ok(self.spanned(Expr::StringConstructor(parts), start))
    }

    // ---- direct constructors ----

    fn direct_constructor(&mut self) -> R<ExprS> {
        self.enter()?;
        let r = self.direct_constructor_inner();
        self.leave();
        r
    }

    fn direct_constructor_inner(&mut self) -> R<ExprS> {
        let start = self.pos;
        if self.rest().starts_with("<!--") {
            self.pos = self.pos.saturating_add(4);
            let Some(end) = self.rest().find("-->") else {
                return Err(self.err("XPST0003", "a comment is not closed with '-->'", start..self.src.len()));
            };
            let text = self.rest().get(..end).unwrap_or("").to_owned();
            if text.contains("--") || text.ends_with('-') {
                return Err(self.err("XPST0003", "a comment cannot contain '--' or end with '-'", start..self.pos.saturating_add(end + 3)));
            }
            self.pos = self.pos.saturating_add(end + 3);
            return Ok(self.spanned(Expr::DirComment(normalize_newlines(&text)), start));
        }
        if self.rest().starts_with("<?") {
            self.pos = self.pos.saturating_add(2);
            let Some(target) = self.ncname() else { return Err(self.syntax("expected a processing-instruction target")) };
            if target.eq_ignore_ascii_case("xml") {
                return Err(self.err("XPST0003", "a processing instruction cannot be named 'xml'", start..self.pos));
            }
            let Some(end) = self.rest().find("?>") else {
                return Err(self.err("XPST0003", "a processing instruction is not closed with '?>'", start..self.src.len()));
            };
            let raw = self.rest().get(..end).unwrap_or("");
            if !raw.is_empty() && !raw.starts_with(|c: char| c.is_whitespace()) {
                return Err(self.err("XPST0003", "a processing-instruction target must be followed by whitespace", start..self.pos));
            }
            let content = normalize_newlines(raw.trim_start());
            self.pos = self.pos.saturating_add(end + 2);
            return Ok(self.spanned(Expr::DirPi { target, content }, start));
        }
        self.pos = self.pos.saturating_add(1);
        let Some(tag) = self.raw_name_here()? else { return Err(self.syntax("expected an element name after '<'")) };
        // attributes, as written
        let mut raw_attrs: Vec<(RawName, Vec<RawAttrPart>, Span)> = Vec::new();
        loop {
            let had_space = self.skip_xml_space();
            if self.rest().starts_with("/>") || self.rest().starts_with('>') {
                break;
            }
            if !had_space {
                return Err(self.syntax("expected whitespace, '>' or '/>'"));
            }
            let astart = self.pos;
            let Some(name) = self.raw_name_here()? else { return Err(self.syntax("expected an attribute name")) };
            self.skip_xml_space();
            if !self.rest().starts_with('=') {
                self.note("'='");
                return Err(self.syntax("expected '='"));
            }
            self.pos = self.pos.saturating_add(1);
            self.skip_xml_space();
            let value = self.attribute_value()?;
            raw_attrs.push((name, value, astart..self.pos));
        }
        // namespace declarations first: they are in scope for the names of the element and its attributes
        let mut namespaces: Vec<(String, String)> = Vec::new();
        let mut scope = self.scope().clone();
        let mut attributes_raw = Vec::new();
        for (name, value, span) in raw_attrs {
            let is_default = name.prefix.is_none() && name.uri.is_none() && name.local == "xmlns";
            let is_prefixed = name.prefix.as_deref() == Some("xmlns");
            if !(is_default || is_prefixed) {
                attributes_raw.push((name, value, span));
                continue;
            }
            let mut uri = String::new();
            for part in value {
                match part {
                    RawAttrPart::Text(t) => uri.push_str(&t),
                    RawAttrPart::Expr(_) => return Err(self.err("XQST0022", "a namespace declaration must be a literal", span)),
                }
            }
            let prefix = if is_prefixed { name.local.clone() } else { String::new() };
            if namespaces.iter().any(|(p, _)| *p == prefix) {
                return Err(self.err("XQST0071", "a namespace is declared twice on one element", span));
            }
            if prefix == "xmlns" || (prefix == "xml" && uri != XML_NS) || (prefix != "xml" && uri == XML_NS) || uri == XMLNS_NS {
                return Err(self.err("XQST0070", format!("the prefix '{prefix}' cannot be bound to '{uri}'"), span));
            }
            if is_prefixed && uri.is_empty() {
                return Err(self.err("XQST0085", "a prefix cannot be undeclared (bound to '')", span));
            }
            if is_default {
                scope.default_element = uri.clone();
            } else {
                scope.prefixes.insert(prefix.clone(), uri.clone());
            }
            namespaces.push((prefix, uri));
        }
        self.scopes.push(scope);
        let result = self.direct_element_rest(start, tag, attributes_raw, namespaces);
        self.scopes.pop();
        result
    }

    fn direct_element_rest(
        &mut self,
        start: usize,
        tag: RawName,
        attributes_raw: Vec<(RawName, Vec<RawAttrPart>, Span)>,
        namespaces: Vec<(String, String)>,
    ) -> R<ExprS> {
        let name = self.resolve(&tag, NameKind::Element)?;
        let mut attributes: Vec<DirAttribute> = Vec::new();
        for (raw, parts, span) in attributes_raw {
            let aname = self.resolve(&raw, NameKind::None)?;
            if attributes.iter().any(|a| a.name.local_name() == aname.local_name() && a.name.namespace() == aname.namespace()) {
                return Err(self.err("XQST0040", format!("the attribute '{}' is given twice", aname.local_name()), span));
            }
            let mut value = Vec::new();
            for p in parts {
                match p {
                    RawAttrPart::Text(t) => value.push(AttrPart::Text(t)),
                    RawAttrPart::Expr(e) => value.push(AttrPart::Expr(e)),
                }
            }
            attributes.push(DirAttribute { name: aname, value, span });
        }
        if self.rest().starts_with("/>") {
            self.pos = self.pos.saturating_add(2);
            return Ok(self.spanned(Expr::DirElement { name, namespaces, attributes, content: Vec::new() }, start));
        }
        // `>`
        self.pos = self.pos.saturating_add(1);
        let content = self.element_content()?;
        // `</name S? >`
        let close_start = self.pos;
        self.pos = self.pos.saturating_add(2);
        let Some(close) = self.raw_name_here()? else { return Err(self.syntax("expected the name of the end tag")) };
        if close.prefix != tag.prefix || close.local != tag.local || close.uri != tag.uri {
            return Err(self.err("XQST0118", "the end tag does not match the start tag", close_start..self.pos));
        }
        self.skip_xml_space();
        if !self.rest().starts_with('>') {
            self.note("'>'");
            return Err(self.syntax("expected '>'"));
        }
        self.pos = self.pos.saturating_add(1);
        Ok(self.spanned(Expr::DirElement { name, namespaces, attributes, content }, start))
    }

    /// A name right here (no whitespace or comments skipped: inside a tag they are not allowed before the name).
    fn raw_name_here(&mut self) -> R<Option<RawName>> {
        let start = self.pos;
        let Some(first) = self.ncname() else {
            self.note("a name");
            return Ok(None);
        };
        if self.rest().starts_with(':') && self.peek_at(1).is_some_and(is_name_start) {
            self.pos = self.pos.saturating_add(1);
            let Some(local) = self.ncname() else { return Ok(None) };
            return Ok(Some(RawName { prefix: Some(first), uri: None, local, span: start..self.pos }));
        }
        Ok(Some(RawName { prefix: None, uri: None, local: first, span: start..self.pos }))
    }

    fn skip_xml_space(&mut self) -> bool {
        let mut any = false;
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.bump();
            any = true;
        }
        any
    }

    fn attribute_value(&mut self) -> R<Vec<RawAttrPart>> {
        let start = self.pos;
        let Some(quote) = self.peek().filter(|c| *c == '"' || *c == '\'') else {
            self.note("a quoted attribute value");
            return Err(self.syntax("expected an attribute value"));
        };
        self.bump();
        let mut parts = Vec::new();
        let mut text = String::new();
        loop {
            match self.bump() {
                None => return Err(self.err("XPST0003", "an attribute value is not closed", start..self.src.len())),
                Some(c) if c == quote => {
                    if self.peek() == Some(quote) {
                        self.bump();
                        text.push(quote);
                    } else {
                        break;
                    }
                }
                Some('{') => {
                    if self.peek() == Some('{') {
                        self.bump();
                        text.push('{');
                        continue;
                    }
                    if !text.is_empty() {
                        parts.push(RawAttrPart::Text(std::mem::take(&mut text)));
                    }
                    self.enter()?;
                    let e = if self.at("}")? { None } else { Some(self.expr()?) };
                    self.leave();
                    self.expect("}")?;
                    if let Some(e) = e {
                        parts.push(RawAttrPart::Expr(e));
                    }
                }
                Some('}') => {
                    if self.peek() == Some('}') {
                        self.bump();
                        text.push('}');
                    } else {
                        return Err(self.err("XPST0003", "a '}' in an attribute value must be written '}}'", self.pos.saturating_sub(1)..self.pos));
                    }
                }
                Some('<') => return Err(self.err("XPST0003", "'<' is not allowed in an attribute value", self.pos.saturating_sub(1)..self.pos)),
                Some('&') => {
                    let at = self.pos.saturating_sub(1);
                    let t = self.reference(at)?;
                    text.push_str(&t);
                }
                // Attribute-value normalization: literal whitespace characters become spaces (a line end, CR LF, one space).
                Some('\r') => {
                    if self.peek() == Some('\n') {
                        self.bump();
                    }
                    text.push(' ');
                }
                Some('\n' | '\t') => text.push(' '),
                Some(c) => text.push(c),
            }
        }
        if !text.is_empty() || parts.is_empty() {
            parts.push(RawAttrPart::Text(text));
        }
        Ok(parts)
    }

    fn element_content(&mut self) -> R<Vec<DirContent>> {
        let mut content = Vec::new();
        let mut text = String::new();
        // Whether the current run of characters has more than whitespace written as such (references and CDATA count as content).
        let mut significant = false;
        let flush = |content: &mut Vec<DirContent>, text: &mut String, significant: &mut bool| {
            if !text.is_empty() {
                content.push(DirContent::Text { text: std::mem::take(text), boundary: !*significant });
            }
            *significant = false;
        };
        loop {
            if content.len() > 100_000 {
                return Err(self.err("XPST0003", "an element has too much content", self.pos..self.pos));
            }
            let rest = self.rest();
            if rest.starts_with("</") {
                flush(&mut content, &mut text, &mut significant);
                return Ok(content);
            }
            if rest.starts_with("<![CDATA[") {
                let cstart = self.pos;
                self.pos = self.pos.saturating_add(9);
                let Some(end) = self.rest().find("]]>") else {
                    return Err(self.err("XPST0003", "a CDATA section is not closed with ']]>'", cstart..self.src.len()));
                };
                text.push_str(&normalize_newlines(self.rest().get(..end).unwrap_or("")));
                significant = true;
                self.pos = self.pos.saturating_add(end + 3);
                continue;
            }
            if rest.starts_with('<') {
                flush(&mut content, &mut text, &mut significant);
                let node = self.direct_constructor()?;
                content.push(DirContent::Node(Box::new(node)));
                continue;
            }
            match self.bump() {
                None => return Err(self.err("XPST0003", "an element is not closed", self.src.len()..self.src.len())),
                Some('{') => {
                    if self.peek() == Some('{') {
                        self.bump();
                        text.push('{');
                        significant = true;
                        continue;
                    }
                    flush(&mut content, &mut text, &mut significant);
                    self.enter()?;
                    let e = if self.at("}")? { None } else { Some(Box::new(self.expr()?)) };
                    self.leave();
                    self.expect("}")?;
                    content.push(DirContent::Enclosed(e));
                }
                Some('}') => {
                    if self.peek() == Some('}') {
                        self.bump();
                        text.push('}');
                        significant = true;
                    } else {
                        return Err(self.err("XPST0003", "a '}' in element content must be written '}}'", self.pos.saturating_sub(1)..self.pos));
                    }
                }
                Some('&') => {
                    let at = self.pos.saturating_sub(1);
                    let t = self.reference(at)?;
                    text.push_str(&t);
                    significant = true;
                }
                Some('\r') => {
                    if self.peek() == Some('\n') {
                        self.bump();
                    }
                    text.push('\n');
                }
                Some(c) => {
                    if !c.is_whitespace() {
                        significant = true;
                    }
                    text.push(c);
                }
            }
        }
    }
}

#[allow(clippy::large_enum_variant)]
enum RawAttrPart {
    Text(String),
    Expr(ExprS),
}

#[cold]
fn unreachable_scope() -> &'static Scope {
    static EMPTY: std::sync::OnceLock<Scope> = std::sync::OnceLock::new();
    EMPTY.get_or_init(|| Scope { prefixes: HashMap::new(), default_element: String::new(), default_function: FN_NS.to_owned() })
}

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Whitespace collapsed (URI literals are whitespace-normalized, XQuery 3.1 §4.12).
fn normalize_space(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn resolve_refs_lenient(s: &str) -> String {
    s.to_owned()
}
