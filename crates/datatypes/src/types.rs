//! Simple types: the built-in ones and the machinery to check a lexical representation against any of them.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use xmlcraft_pattern::Pattern;

use crate::binary::{parse_base64, parse_hex};
use crate::decimal::Decimal;
use crate::duration::Duration;
use crate::error::{Violation, ViolationKind};
use crate::facets::{Facets, Fixed};
use crate::name::split_qname;
use crate::number::{parse_double, parse_float};
use crate::temporal::{Kind, Temporal};
use crate::uri::is_any_uri;
use crate::value::{QNameValue, Value};
use crate::whitespace::Whitespace;

/// The namespace of the built-in types.
pub const XSD_NAMESPACE: &str = "http://www.w3.org/2001/XMLSchema";

/// A name with its namespace.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ExpandedName {
    pub namespace: Option<String>,
    pub local: String,
}

impl ExpandedName {
    pub fn new(namespace: Option<&str>, local: &str) -> Self {
        Self { namespace: namespace.map(str::to_owned), local: local.to_owned() }
    }

    /// A name in the namespace of the built-in types.
    pub fn xsd(local: &str) -> Self {
        Self::new(Some(XSD_NAMESPACE), local)
    }
}

impl std::fmt::Display for ExpandedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.namespace {
            Some(ns) => write!(f, "{{{ns}}}{}", self.local),
            None => f.write_str(&self.local),
        }
    }
}

/// The primitive types (and `anySimpleType`, whose values are any string).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Primitive {
    AnySimple,
    String,
    Boolean,
    Decimal,
    Float,
    Double,
    Duration,
    DateTime,
    Time,
    Date,
    GYearMonth,
    GYear,
    GMonthDay,
    GDay,
    GMonth,
    HexBinary,
    Base64Binary,
    AnyUri,
    QName,
    Notation,
}

impl Primitive {
    /// The name the primitive type has in the schema language.
    pub fn name(self) -> &'static str {
        match self {
            Self::AnySimple => "anySimpleType",
            Self::String => "string",
            Self::Boolean => "boolean",
            Self::Decimal => "decimal",
            Self::Float => "float",
            Self::Double => "double",
            Self::Duration => "duration",
            Self::DateTime => "dateTime",
            Self::Time => "time",
            Self::Date => "date",
            Self::GYearMonth => "gYearMonth",
            Self::GYear => "gYear",
            Self::GMonthDay => "gMonthDay",
            Self::GDay => "gDay",
            Self::GMonth => "gMonth",
            Self::HexBinary => "hexBinary",
            Self::Base64Binary => "base64Binary",
            Self::AnyUri => "anyURI",
            Self::QName => "QName",
            Self::Notation => "NOTATION",
        }
    }

    /// What the `whiteSpace` facet is for the type unless a restriction says otherwise.
    fn default_whitespace(self) -> Whitespace {
        match self {
            Self::AnySimple | Self::String => Whitespace::Preserve,
            _ => Whitespace::Collapse,
        }
    }

    fn all() -> [Primitive; 20] {
        [
            Self::AnySimple,
            Self::String,
            Self::Boolean,
            Self::Decimal,
            Self::Float,
            Self::Double,
            Self::Duration,
            Self::DateTime,
            Self::Time,
            Self::Date,
            Self::GYearMonth,
            Self::GYear,
            Self::GMonthDay,
            Self::GDay,
            Self::GMonth,
            Self::HexBinary,
            Self::Base64Binary,
            Self::AnyUri,
            Self::QName,
            Self::Notation,
        ]
    }
}

/// The namespaces in scope where a lexical representation is, for the types whose values have a qualified name.
pub trait NamespaceContext {
    /// The namespace a prefix stands for; `None` as the prefix asks for the default namespace. `None` is also the answer for a prefix
    /// that is not declared (a `QName` that uses it is not valid), and for no default namespace.
    fn resolve(&self, prefix: Option<&str>) -> Option<String>;

    /// Whether a prefix is declared at all (a default namespace that is not declared is fine).
    fn is_declared(&self, prefix: &str) -> bool {
        self.resolve(Some(prefix)).is_some()
    }
}

/// A context in which no prefix is declared.
pub struct NoNamespaces;

impl NamespaceContext for NoNamespaces {
    fn resolve(&self, _: Option<&str>) -> Option<String> {
        None
    }
}

/// How a type came to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Derivation {
    /// The ur-type, a primitive type: derived from `anySimpleType`.
    Primitive,
    /// Restriction of the base type by facets.
    Restriction,
    /// A list of the item type.
    List,
    /// A union of the member types.
    Union,
}

/// What kind of values a type has.
#[derive(Clone, Debug)]
pub enum Variety {
    Atomic(Primitive),
    List(Arc<SimpleType>),
    Union(Vec<Arc<SimpleType>>),
}

/// A simple type.
#[derive(Clone, Debug)]
pub struct SimpleType {
    pub(crate) name: Option<ExpandedName>,
    pub(crate) variety: Variety,
    pub(crate) derivation: Derivation,
    pub(crate) base: Option<Arc<SimpleType>>,
    pub(crate) facets: Facets,
}

impl SimpleType {
    pub fn name(&self) -> Option<&ExpandedName> {
        self.name.as_ref()
    }

    pub fn variety(&self) -> &Variety {
        &self.variety
    }

    pub fn derivation(&self) -> Derivation {
        self.derivation
    }

    /// The type this one is derived from (`None` for `anySimpleType`).
    pub fn base(&self) -> Option<&Arc<SimpleType>> {
        self.base.as_ref()
    }

    /// The facets this derivation step sets.
    pub fn facets(&self) -> &Facets {
        &self.facets
    }

    /// The primitive type of an atomic type's values.
    pub fn primitive(&self) -> Option<Primitive> {
        match &self.variety {
            Variety::Atomic(p) => Some(*p),
            _ => None,
        }
    }

    /// The item type of a list type.
    pub fn item_type(&self) -> Option<&Arc<SimpleType>> {
        match &self.variety {
            Variety::List(item) => Some(item),
            _ => None,
        }
    }

    /// The member types of a union type.
    pub fn member_types(&self) -> Option<&[Arc<SimpleType>]> {
        match &self.variety {
            Variety::Union(members) => Some(members),
            _ => None,
        }
    }

    /// The local name if this is a built-in type, or an ancestor of it that is, walking up the restrictions (`ID` for a type derived
    /// from `ID`).
    pub fn is_derived_from_builtin(&self, local: &str) -> bool {
        let mut at = Some(self);
        while let Some(t) = at {
            if t.name.as_ref().is_some_and(|n| n.namespace.as_deref() == Some(XSD_NAMESPACE) && n.local == local) {
                return true;
            }
            at = t.base.as_deref();
        }
        false
    }

    /// Whether `other` is this type or an ancestor of it.
    pub fn is_derived_from(&self, other: &SimpleType) -> bool {
        let mut at = Some(self);
        while let Some(t) = at {
            if std::ptr::eq(t, other) || (t.name.is_some() && t.name == other.name) {
                return true;
            }
            at = t.base.as_deref();
        }
        false
    }

    /// This step and the restrictions it is made of, most derived first, ending with the step that made the variety (a primitive type,
    /// or the list or union type).
    pub(crate) fn steps(&self) -> Vec<&SimpleType> {
        let mut out = vec![self];
        let mut at = self;
        while at.derivation == Derivation::Restriction {
            let Some(base) = at.base.as_deref() else { break };
            out.push(base);
            at = base;
        }
        out
    }

    /// The `whiteSpace` in effect: the last restriction that sets one, or the default of the variety.
    pub fn whitespace(&self) -> Whitespace {
        let steps = self.steps();
        steps.iter().find_map(|s| s.facets.white_space).unwrap_or(match &self.variety {
            Variety::Atomic(p) => p.default_whitespace(),
            Variety::List(_) => Whitespace::Collapse,
            Variety::Union(_) => Whitespace::Preserve,
        })
    }

    /// A name for messages.
    fn describe(&self) -> String {
        let mut at = Some(self);
        while let Some(t) = at {
            if let Some(name) = &t.name {
                return name.local.clone();
            }
            at = t.base.as_deref();
        }
        "value".to_owned()
    }

    /// Check a lexical representation and give the value it stands for. A violation says what is wrong with it.
    pub fn validate(&self, lexical: &str, ns: &dyn NamespaceContext) -> Result<Value, Violation> {
        match &self.variety {
            Variety::Atomic(primitive) => self.validate_atomic(*primitive, lexical, ns),
            Variety::List(item) => self.validate_list(item, lexical, ns),
            Variety::Union(members) => self.validate_union(members, lexical, ns),
        }
    }

    fn validate_atomic(&self, primitive: Primitive, lexical: &str, ns: &dyn NamespaceContext) -> Result<Value, Violation> {
        let normalised = self.whitespace().apply(lexical);
        let steps = self.steps();
        for step in steps.iter().rev() {
            step.facets.check_patterns(&normalised).map_err(|v| step.lexical_if_builtin(&normalised, v))?;
        }
        let value = parse_primitive(primitive, &normalised, ns)
            .ok_or_else(|| Violation::new(ViolationKind::Lexical, format!("'{normalised}' is not a valid {}", self.describe())))?;
        for step in steps.iter().rev() {
            step.facets.check_value(&value)?;
        }
        Ok(value)
    }

    /// A pattern of a built-in type is what defines its lexical space (`integer` is `[\-+]?[0-9]+`): breaking it is not a lexical
    /// representation of that type, rather than a pattern facet a schema author wrote.
    fn lexical_if_builtin(&self, lexical: &str, violation: Violation) -> Violation {
        match &self.name {
            Some(name) if name.namespace.as_deref() == Some(XSD_NAMESPACE) => {
                Violation::new(ViolationKind::Lexical, format!("'{lexical}' is not a valid {}", name.local))
            }
            _ => violation,
        }
    }

    fn validate_list(&self, item: &Arc<SimpleType>, lexical: &str, ns: &dyn NamespaceContext) -> Result<Value, Violation> {
        let normalised = Whitespace::Collapse.apply(lexical);
        let steps = self.steps();
        for step in steps.iter().rev() {
            step.facets.check_patterns(&normalised).map_err(|v| step.lexical_if_builtin(&normalised, v))?;
        }
        let mut values = Vec::new();
        for (i, part) in normalised.split(' ').filter(|p| !p.is_empty()).enumerate() {
            values.push(item.validate(part, ns).map_err(|v| Violation::new(v.kind, format!("item {}: {}", i + 1, v.message)))?);
        }
        let value = Value::List(values);
        for step in steps.iter().rev() {
            step.facets.check_value(&value)?;
        }
        Ok(value)
    }

    fn validate_union(&self, members: &[Arc<SimpleType>], lexical: &str, ns: &dyn NamespaceContext) -> Result<Value, Violation> {
        let steps = self.steps();
        for step in steps.iter().rev() {
            step.facets.check_patterns(lexical)?;
        }
        let mut reasons = Vec::new();
        let mut found = None;
        for member in members {
            match member.validate(lexical, ns) {
                Ok(value) => {
                    found = Some(value);
                    break;
                }
                Err(v) => reasons.push(format!("{}: {}", member.describe(), v.message)),
            }
        }
        let Some(value) = found else {
            return Err(Violation::new(ViolationKind::Union, format!("'{lexical}' is valid for none of the member types ({})", reasons.join("; "))));
        };
        for step in steps.iter().rev() {
            step.facets.check_value(&value)?;
        }
        Ok(value)
    }

    /// A built-in type by its local name (`string`, `positiveInteger`, `anySimpleType`, …).
    pub fn builtin(local: &str) -> Option<Arc<SimpleType>> {
        builtins().get(local).cloned()
    }

    /// The local names of the built-in types.
    pub fn builtin_names() -> Vec<&'static str> {
        let mut names: Vec<&'static str> = builtins().keys().copied().collect();
        names.sort_unstable();
        names
    }
}

fn parse_primitive(primitive: Primitive, text: &str, ns: &dyn NamespaceContext) -> Option<Value> {
    Some(match primitive {
        Primitive::AnySimple | Primitive::String => Value::String(text.to_owned()),
        Primitive::Boolean => Value::Boolean(match text {
            "true" | "1" => true,
            "false" | "0" => false,
            _ => return None,
        }),
        Primitive::Decimal => Value::Decimal(Decimal::parse(text)?),
        Primitive::Float => Value::Float(parse_float(text)?),
        Primitive::Double => Value::Double(parse_double(text)?),
        Primitive::Duration => Value::Duration(Duration::parse(text)?),
        Primitive::DateTime => Value::Temporal(Temporal::parse(Kind::DateTime, text)?),
        Primitive::Time => Value::Temporal(Temporal::parse(Kind::Time, text)?),
        Primitive::Date => Value::Temporal(Temporal::parse(Kind::Date, text)?),
        Primitive::GYearMonth => Value::Temporal(Temporal::parse(Kind::GYearMonth, text)?),
        Primitive::GYear => Value::Temporal(Temporal::parse(Kind::GYear, text)?),
        Primitive::GMonthDay => Value::Temporal(Temporal::parse(Kind::GMonthDay, text)?),
        Primitive::GDay => Value::Temporal(Temporal::parse(Kind::GDay, text)?),
        Primitive::GMonth => Value::Temporal(Temporal::parse(Kind::GMonth, text)?),
        Primitive::HexBinary => Value::HexBinary(parse_hex(text)?),
        Primitive::Base64Binary => Value::Base64Binary(parse_base64(text)?),
        Primitive::AnyUri => is_any_uri(text).then(|| Value::AnyUri(text.to_owned()))?,
        Primitive::QName | Primitive::Notation => {
            let (prefix, local) = split_qname(text)?;
            let namespace = match prefix {
                Some(p) => Some(ns.resolve(Some(p))?),
                None => ns.resolve(None),
            };
            let name = QNameValue { namespace, local: local.to_owned() };
            if primitive == Primitive::QName { Value::QName(name) } else { Value::Notation(name) }
        }
    })
}

/// The built-in types, made once.
fn builtins() -> &'static HashMap<&'static str, Arc<SimpleType>> {
    static BUILTINS: OnceLock<HashMap<&'static str, Arc<SimpleType>>> = OnceLock::new();
    BUILTINS.get_or_init(build_builtins)
}

fn patterns(sources: &[&str]) -> Vec<Pattern> {
    // The patterns are the ones the specification gives for the built-in types; a test checks that every one of them is read.
    sources.iter().filter_map(|s| Pattern::new(s).ok()).collect()
}

fn decimal_bound(text: &str) -> Option<Value> {
    Decimal::parse(text).map(Value::Decimal)
}

fn build_builtins() -> HashMap<&'static str, Arc<SimpleType>> {
    let mut all: HashMap<&'static str, Arc<SimpleType>> = HashMap::new();
    let any = Arc::new(SimpleType {
        name: Some(ExpandedName::xsd("anySimpleType")),
        variety: Variety::Atomic(Primitive::AnySimple),
        derivation: Derivation::Primitive,
        base: None,
        facets: Facets::default(),
    });
    all.insert("anySimpleType", any.clone());
    for primitive in Primitive::all().into_iter().skip(1) {
        let t = SimpleType {
            name: Some(ExpandedName::xsd(primitive.name())),
            variety: Variety::Atomic(primitive),
            derivation: Derivation::Primitive,
            base: Some(any.clone()),
            facets: Facets::default(),
        };
        all.insert(primitive.name(), Arc::new(t));
    }
    // `base` names a type made before; the facets are what the specification defines for the derived type.
    let mut derive = |name: &'static str, base: &str, facets: Facets| {
        let Some(base) = all.get(base).cloned() else { return };
        let variety = base.variety.clone();
        let t = SimpleType { name: Some(ExpandedName::xsd(name)), variety, derivation: Derivation::Restriction, base: Some(base), facets };
        all.insert(name, Arc::new(t));
    };
    let ws = |w: Whitespace| Facets { white_space: Some(w), ..Facets::default() };
    let pattern = |sources: &[&str]| Facets { patterns: patterns(sources), ..Facets::default() };
    derive("normalizedString", "string", ws(Whitespace::Replace));
    derive("token", "normalizedString", ws(Whitespace::Collapse));
    derive("language", "token", pattern(&["[a-zA-Z]{1,8}(-[a-zA-Z0-9]{1,8})*"]));
    derive("NMTOKEN", "token", pattern(&["\\c+"]));
    derive("Name", "token", pattern(&["\\i\\c*"]));
    derive("NCName", "Name", pattern(&["[\\i-[:]][\\c-[:]]*"]));
    derive("ID", "NCName", Facets::default());
    derive("IDREF", "NCName", Facets::default());
    derive("ENTITY", "NCName", Facets::default());
    derive(
        "integer",
        "decimal",
        Facets {
            fraction_digits: Some(0),
            patterns: patterns(&["[\\-+]?[0-9]+"]),
            fixed: Fixed { fraction_digits: true, ..Fixed::default() },
            ..Facets::default()
        },
    );
    let range = |min: Option<&str>, max: Option<&str>| Facets {
        min_inclusive: min.and_then(decimal_bound),
        max_inclusive: max.and_then(decimal_bound),
        ..Facets::default()
    };
    derive("nonPositiveInteger", "integer", range(None, Some("0")));
    derive("negativeInteger", "nonPositiveInteger", range(None, Some("-1")));
    derive("long", "integer", range(Some("-9223372036854775808"), Some("9223372036854775807")));
    derive("int", "long", range(Some("-2147483648"), Some("2147483647")));
    derive("short", "int", range(Some("-32768"), Some("32767")));
    derive("byte", "short", range(Some("-128"), Some("127")));
    derive("nonNegativeInteger", "integer", range(Some("0"), None));
    derive("unsignedLong", "nonNegativeInteger", range(None, Some("18446744073709551615")));
    derive("unsignedInt", "unsignedLong", range(None, Some("4294967295")));
    derive("unsignedShort", "unsignedInt", range(None, Some("65535")));
    derive("unsignedByte", "unsignedShort", range(None, Some("255")));
    derive("positiveInteger", "nonNegativeInteger", range(Some("1"), None));
    // The list types: one item type, at least one item.
    for (name, item) in [("NMTOKENS", "NMTOKEN"), ("IDREFS", "IDREF"), ("ENTITIES", "ENTITY")] {
        let Some(item) = all.get(item).cloned() else { continue };
        let t = SimpleType {
            name: Some(ExpandedName::xsd(name)),
            variety: Variety::List(item),
            derivation: Derivation::List,
            base: all.get("anySimpleType").cloned(),
            facets: Facets { min_length: Some(1), ..Facets::default() },
        };
        all.insert(name, Arc::new(t));
    }
    all
}
