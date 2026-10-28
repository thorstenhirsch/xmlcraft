//! Values: what a lexical representation means, in the value space of a primitive type.

use std::cmp::Ordering;

use crate::binary::{to_base64, to_hex};
use crate::decimal::Decimal;
use crate::duration::Duration;
use crate::number::{double_canonical, float_canonical};
use crate::temporal::Temporal;

/// A qualified name: the namespace and the local part. (The prefix it was written with is not part of the value.)
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct QNameValue {
    pub namespace: Option<String>,
    pub local: String,
}

/// A value of a primitive type, or a list of values.
#[derive(Clone, Debug)]
pub enum Value {
    String(String),
    AnyUri(String),
    Boolean(bool),
    Decimal(Decimal),
    Float(f32),
    Double(f64),
    Duration(Duration),
    Temporal(Temporal),
    HexBinary(Vec<u8>),
    Base64Binary(Vec<u8>),
    QName(QNameValue),
    Notation(QNameValue),
    List(Vec<Value>),
}

impl Value {
    /// Whether the two are the same value. Values of different primitive types are never the same; `NaN` is the same as `NaN`, and `0`
    /// as `-0`; two dates that cannot be ordered are not the same.
    pub fn equals(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::String(a), Self::String(b)) | (Self::AnyUri(a), Self::AnyUri(b)) => a == b,
            (Self::Boolean(a), Self::Boolean(b)) => a == b,
            (Self::Decimal(a), Self::Decimal(b)) => a == b,
            (Self::Float(a), Self::Float(b)) => a == b || (a.is_nan() && b.is_nan()),
            (Self::Double(a), Self::Double(b)) => a == b || (a.is_nan() && b.is_nan()),
            (Self::Duration(a), Self::Duration(b)) => a.equals(b),
            (Self::Temporal(a), Self::Temporal(b)) => a.equals(b),
            (Self::HexBinary(a), Self::HexBinary(b)) | (Self::Base64Binary(a), Self::Base64Binary(b)) => a == b,
            (Self::QName(a), Self::QName(b)) | (Self::Notation(a), Self::Notation(b)) => a == b,
            (Self::List(a), Self::List(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.equals(y)),
            _ => false,
        }
    }

    /// The order of two values of the same primitive type, for the types that have one (the numbers, the durations, the dates and
    /// times); `None` for the others, for a pair that cannot be ordered, and for `NaN`.
    pub fn compare(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Self::Decimal(a), Self::Decimal(b)) => Some(a.cmp(b)),
            (Self::Float(a), Self::Float(b)) => a.partial_cmp(b),
            (Self::Double(a), Self::Double(b)) => a.partial_cmp(b),
            (Self::Duration(a), Self::Duration(b)) => a.compare(b),
            (Self::Temporal(a), Self::Temporal(b)) => a.compare(b),
            _ => None,
        }
    }

    /// Whether the type of the value has an order at all.
    pub fn is_ordered(&self) -> bool {
        matches!(self, Self::Decimal(_) | Self::Float(_) | Self::Double(_) | Self::Duration(_) | Self::Temporal(_))
    }

    /// The length the `length`, `minLength` and `maxLength` facets constrain: characters of a string, octets of binary data, items of
    /// a list; `None` for the types they do not apply to.
    pub fn length(&self) -> Option<usize> {
        match self {
            Self::String(s) | Self::AnyUri(s) => Some(s.chars().count()),
            Self::HexBinary(b) | Self::Base64Binary(b) => Some(b.len()),
            Self::List(items) => Some(items.len()),
            _ => None,
        }
    }

    /// The canonical lexical representation (a list is its items' canonical forms with spaces between).
    pub fn canonical(&self) -> String {
        match self {
            Self::String(s) | Self::AnyUri(s) => s.clone(),
            Self::Boolean(b) => b.to_string(),
            Self::Decimal(d) => d.to_string(),
            Self::Float(f) => float_canonical(*f),
            Self::Double(d) => double_canonical(*d),
            Self::Duration(d) => d.canonical(),
            Self::Temporal(t) => t.canonical(),
            Self::HexBinary(b) => to_hex(b),
            Self::Base64Binary(b) => to_base64(b),
            Self::QName(q) | Self::Notation(q) => match &q.namespace {
                Some(ns) => format!("{{{ns}}}{}", q.local),
                None => q.local.clone(),
            },
            Self::List(items) => items.iter().map(Self::canonical).collect::<Vec<_>>().join(" "),
        }
    }
}
