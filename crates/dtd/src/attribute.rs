//! Attribute values the way a processor reads them (XML 1.0 §3.3.3): references replaced, white space turned into spaces, and for the
//! types other than CDATA the spaces trimmed and collapsed; and whether a value is a legal one for the type its attribute declares.

use xmlcraft_grammar::{AttributeType, EntityKind, Grammar};
use xmlcraft_syntax::{is_name_char, is_name_start_char};

use crate::parse::char_reference;

/// The longest a value may be after its references are replaced.
pub const MAX_VALUE_BYTES: usize = 1 << 20;
/// How deep entities may refer to entities in a value.
pub const MAX_VALUE_DEPTH: usize = 40;

/// Why a value could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValueProblem {
    /// A reference to an entity that is not declared.
    Undeclared(String),
    /// A reference to an entity that is external or unparsed, which cannot be in a value.
    NotInternal(String),
    /// A character reference to a character XML does not allow, or a `&` that starts no reference.
    BadReference(String),
    /// An entity that refers to itself.
    Recursive(String),
    /// The value is longer than [`MAX_VALUE_BYTES`], or entities nest deeper than [`MAX_VALUE_DEPTH`].
    TooLong,
}

impl std::fmt::Display for ValueProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValueProblem::Undeclared(name) => write!(f, "the entity '{name}' is not declared"),
            ValueProblem::NotInternal(name) => write!(f, "the entity '{name}' is not an internal entity"),
            ValueProblem::BadReference(text) => write!(f, "'{text}' is not a reference"),
            ValueProblem::Recursive(name) => write!(f, "the entity '{name}' refers to itself"),
            ValueProblem::TooLong => write!(f, "the value is too long once its entities are replaced"),
        }
    }
}

/// Reads attribute values against the entities a grammar declares.
pub struct ValueReader<'a> {
    grammar: &'a Grammar,
}

impl<'a> ValueReader<'a> {
    pub fn new(grammar: &'a Grammar) -> Self {
        Self { grammar }
    }

    /// The value of an attribute of type `ty` written as `literal` (the text between the quotes): the references replaced, each white
    /// space character a space, and for a type other than CDATA the spaces at the ends dropped and the runs of spaces one space.
    pub fn read(&self, literal: &str, ty: &AttributeType) -> Result<String, ValueProblem> {
        let mut out = String::new();
        let mut open: Vec<&str> = Vec::new();
        self.expand(literal, &mut out, &mut open)?;
        Ok(if matches!(ty, AttributeType::Cdata) { out } else { collapse(&out) })
    }

    fn expand<'t>(&'t self, text: &'t str, out: &mut String, open: &mut Vec<&'t str>) -> Result<(), ValueProblem> {
        let mut rest = text;
        while let Some(c) = rest.chars().next() {
            match c {
                '&' => {
                    let after = rest.get(1..).unwrap_or_default();
                    let Some(end) = after.find(';') else { return Err(ValueProblem::BadReference("&".to_owned())) };
                    let body = after.get(..end).unwrap_or_default();
                    rest = after.get(end + 1..).unwrap_or_default();
                    if let Some(number) = body.strip_prefix('#') {
                        let ch = char_reference(number).ok_or_else(|| ValueProblem::BadReference(format!("&{body};")))?;
                        push(out, ch)?;
                    } else {
                        match body {
                            "lt" => push(out, '<')?,
                            "gt" => push(out, '>')?,
                            "amp" => push(out, '&')?,
                            "quot" => push(out, '"')?,
                            "apos" => push(out, '\'')?,
                            name => self.entity(name, out, open)?,
                        }
                    }
                }
                '\r' => {
                    rest = rest.get(1..).unwrap_or_default();
                    // A line end is one space, whether it is CR, LF or CR LF.
                    rest = rest.strip_prefix('\n').unwrap_or(rest);
                    push(out, ' ')?;
                }
                '\n' | '\t' | ' ' => {
                    rest = rest.get(1..).unwrap_or_default();
                    push(out, ' ')?;
                }
                other => {
                    rest = rest.get(other.len_utf8()..).unwrap_or_default();
                    push(out, other)?;
                }
            }
        }
        Ok(())
    }

    fn entity<'t>(&'t self, name: &'t str, out: &mut String, open: &mut Vec<&'t str>) -> Result<(), ValueProblem> {
        let Some(decl) = self.grammar.entity(name) else { return Err(ValueProblem::Undeclared(name.to_owned())) };
        let EntityKind::Internal(text) = &decl.kind else { return Err(ValueProblem::NotInternal(name.to_owned())) };
        if open.contains(&name) {
            return Err(ValueProblem::Recursive(name.to_owned()));
        }
        if open.len() >= MAX_VALUE_DEPTH {
            return Err(ValueProblem::TooLong);
        }
        open.push(name);
        let result = self.expand(text, out, open);
        open.pop();
        result
    }
}

fn push(out: &mut String, c: char) -> Result<(), ValueProblem> {
    if out.len() + c.len_utf8() > MAX_VALUE_BYTES {
        return Err(ValueProblem::TooLong);
    }
    out.push(c);
    Ok(())
}

/// Trim the spaces at the ends and make each run of spaces one.
fn collapse(value: &str) -> String {
    value.split(' ').filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ")
}

fn is_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(is_name_start_char) && chars.all(is_name_char)
}

fn is_nmtoken(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_name_char)
}

/// Whether `value` (read already, see [`ValueReader::read`]) is legal for `ty`: a name for ID, IDREF and ENTITY, names for IDREFS and
/// ENTITIES, a name token or tokens, one of the values listed. Whether an IDREF refers to an ID, or an ENTITY to an unparsed entity,
/// is for the document to say. The reason, when it is not.
pub fn check_value(ty: &AttributeType, value: &str) -> Result<(), String> {
    let each = |valid: fn(&str) -> bool, what: &str| -> Result<(), String> {
        if value.is_empty() {
            return Err(format!("it must have at least one {what}"));
        }
        match value.split(' ').find(|part| !valid(part)) {
            Some(bad) => Err(format!("'{bad}' is not a {what}")),
            None => Ok(()),
        }
    };
    match ty {
        AttributeType::Cdata => Ok(()),
        AttributeType::Id | AttributeType::Idref | AttributeType::Entity => {
            if is_name(value) {
                Ok(())
            } else {
                Err(format!("'{value}' is not a name"))
            }
        }
        AttributeType::Idrefs | AttributeType::Entities => each(is_name, "name"),
        AttributeType::Nmtoken => {
            if is_nmtoken(value) {
                Ok(())
            } else {
                Err(format!("'{value}' is not a name token"))
            }
        }
        AttributeType::Nmtokens => each(is_nmtoken, "name token"),
        AttributeType::Notation(list) | AttributeType::Enumeration(list) => {
            if list.iter().any(|token| token == value) {
                Ok(())
            } else {
                Err(format!("'{value}' is not one of the values listed"))
            }
        }
    }
}
