//! A tolerant reader: whatever the text, it gives back the values it could make out and the problems it found, with their places.

use std::ops::Range;

use xmlcraft_syntax::{Code, Diagnostic};

use crate::lexer::{Kind, Lexer, Token};
use crate::value::{Data, Member, Value};

/// How strictly a text is read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    /// RFC 8259: no comments, no trailing commas, names and strings in double quotes.
    #[default]
    Json,
    /// JSON with comments (`//`, `/* */`) and trailing commas, as editors read `.jsonc` files.
    Jsonc,
    /// JSON5: comments, trailing commas, single quotes, bare names, `+1`, `.5`, `0xFF`, `Infinity`, `NaN`.
    Json5,
}

/// Limits and the dialect.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub dialect: Dialect,
    /// How deep arrays and objects may nest (deeper is cut off with a problem).
    pub max_depth: usize,
    /// How many problems are listed (a further one says so).
    pub max_diagnostics: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self { dialect: Dialect::Json, max_depth: 512, max_diagnostics: 1_000 }
    }
}

/// What a reading made of a text.
#[derive(Clone, Debug, PartialEq)]
pub struct Parse {
    /// The value, whole or as far as it could be read; none when the text has no value.
    pub root: Option<Value>,
    pub diagnostics: Vec<Diagnostic>,
}

struct Reader<'a> {
    lexer: Lexer<'a>,
    peeked: Option<Token>,
    options: Options,
    diagnostics: Vec<Diagnostic>,
    full: bool,
    depth: usize,
    /// A comment was reported already (one problem says it).
    commented: bool,
}

/// Read a text that holds one value.
pub fn parse(text: &str, options: &Options) -> Parse {
    let mut reader = Reader {
        lexer: Lexer::new(text, options.dialect == Dialect::Json5),
        peeked: None,
        options: *options,
        diagnostics: Vec::new(),
        full: false,
        depth: 0,
        commented: false,
    };
    let root = reader.value();
    if root.is_none() {
        if let Some(token) = reader.peek().cloned() {
            // Something that is not a value is there: say what, once.
            reader.unexpected(&token);
        } else {
            reader.report(Code::JsonEmpty, 0..0, vec![]);
        }
    } else if let Some(token) = reader.peek().cloned() {
        reader.report(Code::JsonTrailingContent, token.span.clone(), vec![]);
    }
    Parse { root, diagnostics: reader.diagnostics }
}

/// Read JSON Lines: every line that is not blank is a value of its own. The range is the line (without its break).
pub fn parse_lines(text: &str, options: &Options) -> Vec<(Range<usize>, Parse)> {
    let mut out = Vec::new();
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let end = start + line.len();
        let content = line.trim_end_matches(['\n', '\r']);
        if !content.trim().is_empty() {
            let mut parse = parse(content, options);
            // Places are those of the whole text.
            for d in &mut parse.diagnostics {
                d.span = d.span.start + start..d.span.end + start;
            }
            if let Some(root) = &mut parse.root {
                shift(root, start);
            }
            out.push((start..start + content.len(), parse));
        }
        start = end;
    }
    out
}

fn shift(value: &mut Value, by: usize) {
    value.span = value.span.start + by..value.span.end + by;
    match &mut value.data {
        Data::Array(items) => items.iter_mut().for_each(|v| shift(v, by)),
        Data::Object(members) => {
            for m in members {
                m.key_span = m.key_span.start + by..m.key_span.end + by;
                shift(&mut m.value, by);
            }
        }
        _ => {}
    }
}

impl Reader<'_> {
    fn report(&mut self, code: Code, span: Range<usize>, args: Vec<String>) {
        if self.full {
            return;
        }
        if self.diagnostics.len() >= self.options.max_diagnostics {
            self.full = true;
            self.diagnostics.push(Diagnostic::new(Code::TooManyDiagnostics, span, vec![]));
            return;
        }
        self.diagnostics.push(Diagnostic::new(code, span, args));
    }

    fn lenient(&self) -> bool {
        self.options.dialect != Dialect::Json
    }

    fn json5(&self) -> bool {
        self.options.dialect == Dialect::Json5
    }

    /// The next token that is not a comment (comments are reported when the dialect has none), without taking it.
    fn peek(&mut self) -> Option<&Token> {
        while self.peeked.is_none() {
            let token = self.lexer.next_token()?;
            match token.kind {
                Kind::LineComment | Kind::BlockComment { .. } => {
                    if !self.lenient() && !self.commented {
                        self.commented = true;
                        self.report(Code::JsonCommentNotAllowed, token.span.clone(), vec![]);
                    }
                    if let Kind::BlockComment { closed: false } = token.kind {
                        self.report(Code::JsonUnterminatedComment, token.span.clone(), vec![]);
                    }
                }
                _ => self.peeked = Some(token),
            }
        }
        self.peeked.as_ref()
    }

    fn bump(&mut self) -> Option<Token> {
        self.peek();
        self.peeked.take()
    }

    fn describe(&self, token: &Token) -> String {
        match token.kind {
            Kind::LBrace => "'{'".to_owned(),
            Kind::RBrace => "'}'".to_owned(),
            Kind::LBracket => "'['".to_owned(),
            Kind::RBracket => "']'".to_owned(),
            Kind::Colon => "':'".to_owned(),
            Kind::Comma => "','".to_owned(),
            _ => format!("'{}'", self.lexer.text().get(token.span.clone()).unwrap_or_default().chars().take(20).collect::<String>()),
        }
    }

    fn unexpected(&mut self, token: &Token) {
        let what = self.describe(token);
        self.report(Code::JsonUnexpectedToken, token.span.clone(), vec![what]);
    }

    /// A value, or none when the next token cannot start one (it is left where it is).
    fn value(&mut self) -> Option<Value> {
        let token = self.peek()?.clone();
        match token.kind {
            Kind::LBrace => Some(self.object()),
            Kind::LBracket => Some(self.array()),
            Kind::Str { quote, closed } => {
                self.bump();
                Some(self.string_value(&token, quote, closed))
            }
            Kind::Word => {
                self.bump();
                Some(self.word_value(&token))
            }
            Kind::RBrace | Kind::RBracket | Kind::Colon | Kind::Comma | Kind::LineComment | Kind::BlockComment { .. } => None,
        }
    }

    fn string_value(&mut self, token: &Token, quote: char, closed: bool) -> Value {
        if !closed {
            self.report(Code::JsonUnterminatedString, token.span.clone(), vec![quote.to_string()]);
        }
        if quote == '\'' && !self.json5() {
            self.report(Code::JsonSingleQuotes, token.span.clone(), vec![]);
        }
        let text = self.lexer.text().get(token.span.clone()).unwrap_or_default();
        let inner_end = if closed { text.len().saturating_sub(quote.len_utf8()) } else { text.len() };
        let inner = text.get(quote.len_utf8()..inner_end.max(quote.len_utf8())).unwrap_or_default();
        let decoded = self.decode(inner, token.span.start + quote.len_utf8());
        Value { span: token.span.clone(), data: Data::String(decoded) }
    }

    /// The characters a string stands for, its escapes decoded; problems are reported at the place in the text.
    fn decode(&mut self, inner: &str, base: usize) -> String {
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.char_indices().peekable();
        let mut control_reported = false;
        while let Some((i, c)) = chars.next() {
            if c != '\\' {
                if (c as u32) < 0x20 && c != '\n' && c != '\r' && !control_reported {
                    control_reported = true;
                    self.report(Code::JsonControlCharacter, base + i..base + i + c.len_utf8(), vec![]);
                }
                out.push(c);
                continue;
            }
            let Some((_, e)) = chars.next() else {
                self.report(Code::JsonInvalidEscape, base + i..base + i + 1, vec!["\\".to_owned()]);
                break;
            };
            let at = base + i..base + i + 1 + e.len_utf8();
            match e {
                '"' | '\\' | '/' => out.push(e),
                '\'' if self.json5() => out.push(e),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let mut code = String::new();
                    for _ in 0..4 {
                        if let Some((_, h)) = chars.next_if(|(_, h)| h.is_ascii_hexdigit()) {
                            code.push(h);
                        }
                    }
                    match (code.len() == 4).then(|| u32::from_str_radix(&code, 16).ok()).flatten() {
                        Some(unit) if (0xD800..0xDC00).contains(&unit) => {
                            // A high surrogate wants a low one right after it.
                            let mut low = None;
                            let mut ahead = chars.clone();
                            if ahead.next().is_some_and(|(_, c)| c == '\\') && ahead.next().is_some_and(|(_, c)| c == 'u') {
                                let digits: String = ahead.by_ref().take(4).map(|(_, c)| c).collect();
                                low = u32::from_str_radix(&digits, 16).ok().filter(|l| (0xDC00..0xE000).contains(l) && digits.len() == 4);
                                if low.is_some() {
                                    chars = ahead;
                                }
                            }
                            match low.and_then(|l| char::from_u32(0x10000 + ((unit - 0xD800) << 10) + (l - 0xDC00))) {
                                Some(ch) => out.push(ch),
                                None => {
                                    out.push('\u{FFFD}');
                                    self.report(Code::JsonInvalidEscape, at, vec![format!("\\u{code}")]);
                                }
                            }
                        }
                        Some(unit) => match char::from_u32(unit) {
                            Some(ch) => out.push(ch),
                            // A low surrogate with no high one before it.
                            None => {
                                out.push('\u{FFFD}');
                                self.report(Code::JsonInvalidEscape, at, vec![format!("\\u{code}")]);
                            }
                        },
                        None => {
                            self.report(Code::JsonInvalidEscape, at, vec![format!("\\u{code}")]);
                        }
                    }
                }
                '\n' if self.json5() => {}
                '\r' if self.json5() => {
                    chars.next_if(|(_, n)| *n == '\n');
                }
                'v' | '0' | 'x' if self.options.dialect == Dialect::Json5 => match e {
                    'v' => out.push('\u{b}'),
                    '0' => out.push('\0'),
                    _ => {
                        let hex: String = std::iter::from_fn(|| chars.next_if(|(_, h)| h.is_ascii_hexdigit()).map(|(_, h)| h)).take(2).collect();
                        match u8::from_str_radix(&hex, 16) {
                            Ok(byte) if hex.len() == 2 => out.push(char::from(byte)),
                            _ => self.report(Code::JsonInvalidEscape, at, vec!["\\x".to_owned()]),
                        }
                    }
                },
                other if self.options.dialect == Dialect::Json5 => out.push(other),
                other => self.report(Code::JsonInvalidEscape, at, vec![format!("\\{other}")]),
            }
        }
        out
    }

    fn word_value(&mut self, token: &Token) -> Value {
        let word = self.lexer.text().get(token.span.clone()).unwrap_or_default().to_owned();
        let data = match word.as_str() {
            "true" => Data::Bool(true),
            "false" => Data::Bool(false),
            "null" => Data::Null,
            "Infinity" | "-Infinity" | "+Infinity" | "NaN" | "-NaN" | "+NaN" if self.options.dialect == Dialect::Json5 => Data::Number(word),
            _ => {
                if !number_ok(&word, self.options.dialect) {
                    self.report(Code::JsonInvalidNumber, token.span.clone(), vec![word.chars().take(30).collect()]);
                }
                Data::Number(word)
            }
        };
        Value { span: token.span.clone(), data }
    }

    fn object(&mut self) -> Value {
        let open = self.bump().map(|t| t.span).unwrap_or(0..0);
        let mut members: Vec<Member> = Vec::new();
        if self.depth >= self.options.max_depth {
            let end = self.skip_nested(&open);
            self.report(Code::JsonTooDeep, open.clone(), vec![self.options.max_depth.to_string()]);
            return Value { span: open.start..end, data: Data::Object(members) };
        }
        self.depth += 1;
        let mut end = open.end;
        let mut closed = false;
        let mut after_comma: Option<Range<usize>> = None;
        while let Some(token) = self.peek().cloned() {
            match token.kind {
                Kind::RBrace => {
                    self.bump();
                    if let Some(comma) = after_comma.take()
                        && self.options.dialect == Dialect::Json
                    {
                        self.report(Code::JsonTrailingComma, comma, vec![]);
                    }
                    end = token.span.end;
                    closed = true;
                    break;
                }
                Kind::RBracket => {
                    // A bracket of the wrong kind closes nothing here: say so and go on past it.
                    self.bump();
                    self.unexpected(&token);
                    end = token.span.end;
                }
                Kind::Comma => {
                    self.bump();
                    self.report(Code::JsonExpectedKey, token.span.clone(), vec![]);
                    end = token.span.end;
                }
                _ => {
                    let member_start = token.span.start;
                    let Some(member) = self.member(&token) else {
                        // Something that is no name: it was reported and taken.
                        end = self.peeked.as_ref().map_or(end.max(member_start), |t| t.span.start);
                        continue;
                    };
                    if members.iter().any(|m| m.key == member.key) {
                        self.report(Code::JsonDuplicateKey, member.key_span.clone(), vec![member.key.clone()]);
                    }
                    end = member.value.span.end.max(member.key_span.end);
                    members.push(member);
                    after_comma = None;
                    match self.peek().cloned() {
                        Some(next) if next.kind == Kind::Comma => {
                            self.bump();
                            after_comma = Some(next.span.clone());
                            end = next.span.end;
                        }
                        Some(next) if next.kind == Kind::RBrace => {}
                        Some(next) => {
                            // Two members with no comma between them.
                            if matches!(next.kind, Kind::Str { .. } | Kind::Word) {
                                self.report(Code::JsonExpectedComma, end..end.max(next.span.start), vec![]);
                            }
                        }
                        None => {}
                    }
                }
            }
        }
        self.depth -= 1;
        if !closed {
            self.report(Code::JsonUnclosedObject, open.clone(), vec![]);
        }
        Value { span: open.start..end, data: Data::Object(members) }
    }

    /// One `name: value`; none when the token cannot be a name (it is taken and reported).
    fn member(&mut self, token: &Token) -> Option<Member> {
        let (key, key_span) = match token.kind {
            Kind::Str { quote, closed } => {
                self.bump();
                let v = self.string_value(token, quote, closed);
                let Data::String(s) = v.data else { return None };
                (s, token.span.clone())
            }
            Kind::Word => {
                self.bump();
                let word = self.lexer.text().get(token.span.clone()).unwrap_or_default().to_owned();
                if !self.json5() || !is_identifier_name(&word) {
                    self.report(Code::JsonUnquotedKey, token.span.clone(), vec![]);
                }
                (word, token.span.clone())
            }
            _ => {
                self.bump();
                self.report(Code::JsonExpectedKey, token.span.clone(), vec![]);
                return None;
            }
        };
        let start = key_span.end;
        let colon_seen = match self.peek().cloned() {
            Some(t) if t.kind == Kind::Colon => {
                self.bump();
                true
            }
            _ => {
                self.report(Code::JsonExpectedColon, start..start, vec![]);
                false
            }
        };
        let value = match self.value() {
            Some(v) => v,
            None => {
                let len = self.lexer.text().len();
                let at = self.peek().map_or(len, |t| t.span.start);
                if colon_seen || self.peek().is_some() {
                    self.report(Code::JsonExpectedValue, at..at, vec![]);
                }
                Value { span: key_span.end..key_span.end, data: Data::Null }
            }
        };
        Some(Member { key, key_span, value })
    }

    fn array(&mut self) -> Value {
        let open = self.bump().map(|t| t.span).unwrap_or(0..0);
        let mut items: Vec<Value> = Vec::new();
        if self.depth >= self.options.max_depth {
            let end = self.skip_nested(&open);
            self.report(Code::JsonTooDeep, open.clone(), vec![self.options.max_depth.to_string()]);
            return Value { span: open.start..end, data: Data::Array(items) };
        }
        self.depth += 1;
        let mut end = open.end;
        let mut closed = false;
        let mut after_comma: Option<Range<usize>> = None;
        while let Some(token) = self.peek().cloned() {
            match token.kind {
                Kind::RBracket => {
                    self.bump();
                    if let Some(comma) = after_comma.take()
                        && self.options.dialect == Dialect::Json
                    {
                        self.report(Code::JsonTrailingComma, comma, vec![]);
                    }
                    end = token.span.end;
                    closed = true;
                    break;
                }
                Kind::RBrace | Kind::Colon => {
                    self.bump();
                    self.unexpected(&token);
                    end = token.span.end;
                }
                Kind::Comma => {
                    self.bump();
                    self.report(Code::JsonExpectedValue, token.span.clone(), vec![]);
                    end = token.span.end;
                }
                _ => {
                    let Some(item) = self.value() else { break };
                    end = item.span.end;
                    items.push(item);
                    after_comma = None;
                    match self.peek().cloned() {
                        Some(next) if next.kind == Kind::Comma => {
                            self.bump();
                            after_comma = Some(next.span.clone());
                            end = next.span.end;
                        }
                        Some(next) if next.kind == Kind::RBracket => {}
                        Some(next) => {
                            if matches!(next.kind, Kind::Str { .. } | Kind::Word | Kind::LBrace | Kind::LBracket) {
                                self.report(Code::JsonExpectedComma, end..end.max(next.span.start), vec![]);
                            }
                        }
                        None => {}
                    }
                }
            }
        }
        self.depth -= 1;
        if !closed {
            self.report(Code::JsonUnclosedArray, open.clone(), vec![]);
        }
        Value { span: open.start..end, data: Data::Array(items) }
    }

    /// Past the brackets that match the one just opened, without building anything (a text nested too deeply is not read): the offset
    /// where it ends, or the end of the text.
    fn skip_nested(&mut self, open: &Range<usize>) -> usize {
        let mut level = 1usize;
        let mut end = open.end;
        while level > 0 {
            let Some(token) = self.bump() else { break };
            end = token.span.end;
            match token.kind {
                Kind::LBrace | Kind::LBracket => level += 1,
                Kind::RBrace | Kind::RBracket => level -= 1,
                _ => {}
            }
        }
        end
    }
}

/// A bare name JSON5 allows as a key: letters, `$` and `_` first, then also digits (the ECMAScript IdentifierName, without `\u` escapes).
fn is_identifier_name(word: &str) -> bool {
    let mut chars = word.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '$' || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '$' || c == '_' || c == '\u{200C}' || c == '\u{200D}')
}

/// Whether `word` is a number in the dialect: `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`; JSON5 also allows a leading `+`,
/// `.5`, `5.`, `0x1F` and (handled by the caller) `Infinity` and `NaN`.
pub(crate) fn number_ok(word: &str, dialect: Dialect) -> bool {
    let json5 = dialect == Dialect::Json5;
    let body = match word.strip_prefix('-') {
        Some(b) => b,
        None if json5 => word.strip_prefix('+').unwrap_or(word),
        None => word,
    };
    if json5 && let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        return !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(at) => (body.get(..at).unwrap_or_default(), body.get(at + 1..)),
        None => (body, None),
    };
    if let Some(exp) = exponent {
        let digits = exp.strip_prefix(['+', '-']).unwrap_or(exp);
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    let (int, frac) = match mantissa.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (mantissa, None),
    };
    let digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    if let Some(f) = frac
        && !(digits(f) && (!f.is_empty() || json5))
    {
        return false;
    }
    if int.is_empty() {
        return json5 && frac.is_some_and(|f| !f.is_empty());
    }
    digits(int) && (int == "0" || !int.starts_with('0'))
}
