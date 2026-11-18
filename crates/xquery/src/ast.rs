//! The syntax tree of an XQuery 3.1 module.
//!
//! Every expression and clause carries its place in the query text (a byte range), for error messages, the debugger and the profiler.
//! Names are resolved while parsing (prefixes against the prolog and the namespace declarations of enclosing direct constructors), so
//! the tree holds expanded names; types and node tests are `xee`'s own, so they go to the engine as they are.

// (The tree is built once per query and walked; boxing the large variants would save nothing that matters.)
#![allow(clippy::large_enum_variant)]

use std::ops::Range;

use ibig::IBig;
use ordered_float::OrderedFloat;
use rust_decimal::Decimal;
pub use xee_xpath_ast::ast::{Axis, KindTest, Name, NameTest, NodeTest, SequenceType};

/// A byte range in the query text.
pub type Span = Range<usize>;

/// A value with its place in the query text.
#[derive(Clone, Debug, PartialEq)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub fn new(value: T, span: Span) -> Self {
        Spanned { value, span }
    }
}

pub type ExprS = Spanned<Expr>;

/// A main module (a prolog and a query body) or a library module (a prolog only).
#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    /// `xquery version "3.1" encoding "…";`
    pub version: Option<String>,
    /// `module namespace p = "uri";` for a library module.
    pub library: Option<(String, String)>,
    pub prolog: Vec<Spanned<Decl>>,
    /// The query body (`None` for a library module).
    pub body: Option<ExprS>,
}

/// A declaration of the prolog.
#[derive(Clone, Debug, PartialEq)]
pub enum Decl {
    Namespace {
        prefix: String,
        uri: String,
    },
    DefaultElementNamespace(String),
    DefaultFunctionNamespace(String),
    /// `declare boundary-space preserve` (true) or `strip`.
    BoundarySpace(bool),
    DefaultCollation(String),
    BaseUri(String),
    /// `declare construction preserve` (true) or `strip`.
    Construction(bool),
    /// `declare ordering ordered` (true) or `unordered`.
    Ordering(bool),
    /// `declare default order empty greatest` (true) or `least`.
    EmptyOrder(bool),
    CopyNamespaces {
        preserve: bool,
        inherit: bool,
    },
    DecimalFormat {
        name: Option<Name>,
        properties: Vec<(String, String)>,
    },
    SchemaImport {
        prefix: Option<String>,
        uri: String,
        at: Vec<String>,
    },
    ModuleImport {
        prefix: Option<String>,
        uri: String,
        at: Vec<String>,
    },
    Variable {
        name: Name,
        type_: Option<SequenceType>,
        external: bool,
        value: Option<ExprS>,
        annotations: Vec<Annotation>,
    },
    ContextItem {
        type_: Option<SequenceType>,
        external: bool,
        value: Option<ExprS>,
    },
    Function(FunctionDecl),
    Option {
        name: Name,
        value: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionDecl {
    pub name: Name,
    pub params: Vec<Param>,
    pub return_type: Option<SequenceType>,
    /// `None` for an external function.
    pub body: Option<ExprS>,
    pub annotations: Vec<Annotation>,
}

/// `%name("literal", …)`: read, and ignored (except `%private` / `%public`, which only matter across modules).
#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub name: Name,
    pub values: Vec<Literal>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: Name,
    pub type_: Option<SequenceType>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    Integer(IBig),
    Decimal(Decimal),
    Double(OrderedFloat<f64>),
    String(String),
}

/// A binary operator. `Comma` is the sequence of `Expr::Sequence`, not one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    ValueEq,
    ValueNe,
    ValueLt,
    ValueLe,
    ValueGt,
    ValueGe,
    GenEq,
    GenNe,
    GenLt,
    GenLe,
    GenGt,
    GenGe,
    Is,
    Precedes,
    Follows,
    Concat,
    Range,
    Add,
    Sub,
    Mul,
    Div,
    IntDiv,
    Mod,
    Union,
    Intersect,
    Except,
}

/// An argument of a function call: an expression or the `?` of a partial application.
#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Expr(ExprS),
    Placeholder(Span),
}

/// What follows `?` in a lookup.
#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Name(String),
    Integer(IBig),
    Expr(Box<ExprS>),
    Star,
}

/// The single type of `cast as` and `castable as`.
#[derive(Clone, Debug, PartialEq)]
pub struct SingleType {
    pub name: Name,
    pub optional: bool,
}

/// `$x as T` in a FLWOR clause or quantified expression.
#[derive(Clone, Debug, PartialEq)]
pub struct VarBinding {
    pub name: Name,
    pub type_: Option<SequenceType>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Clause {
    For { var: VarBinding, allowing_empty: bool, position: Option<VarBinding>, expr: ExprS },
    Let { var: VarBinding, expr: ExprS },
    Window(Box<Window>),
    Where(ExprS),
    GroupBy(Vec<GroupSpec>),
    OrderBy { stable: bool, specs: Vec<OrderSpec> },
    Count(VarBinding),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    /// `tumbling window` (true) or `sliding window`.
    pub tumbling: bool,
    pub var: VarBinding,
    pub expr: ExprS,
    pub start: WindowCondition,
    /// `end` (`only end` when the flag is set); a sliding window always has one.
    pub end: Option<(bool, WindowCondition)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowCondition {
    pub current: Option<VarBinding>,
    pub position: Option<VarBinding>,
    pub previous: Option<VarBinding>,
    pub next: Option<VarBinding>,
    pub when: ExprS,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GroupSpec {
    pub var: VarBinding,
    /// `group by $k := expr`; `None` groups by a variable already bound.
    pub expr: Option<ExprS>,
    pub collation: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OrderSpec {
    pub expr: ExprS,
    pub descending: bool,
    /// `empty greatest` (true), `empty least` (false), or the prolog's default.
    pub empty_greatest: Option<bool>,
    pub collation: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeCase {
    pub var: Option<VarBinding>,
    pub types: Vec<SequenceType>,
    pub ret: ExprS,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Catch {
    pub tests: Vec<NameTest>,
    pub body: ExprS,
    pub span: Span,
}

/// A part of the value of a direct attribute: characters, or an enclosed expression.
#[derive(Clone, Debug, PartialEq)]
pub enum AttrPart {
    Text(String),
    Expr(ExprS),
}

#[derive(Clone, Debug, PartialEq)]
pub struct DirAttribute {
    pub name: Name,
    pub value: Vec<AttrPart>,
    pub span: Span,
}

/// The content of a direct element constructor.
#[derive(Clone, Debug, PartialEq)]
pub enum DirContent {
    /// Characters (references resolved). `boundary` is whitespace that touches only tags or enclosed expressions (boundary
    /// whitespace, dropped unless `declare boundary-space preserve`); CDATA and references are never boundary whitespace.
    Text { text: String, boundary: bool },
    /// `{ expr }`, or `{}` (`None`).
    Enclosed(Option<Box<ExprS>>),
    /// A nested direct element, comment or processing instruction.
    Node(Box<ExprS>),
}

/// The name of a computed constructor: written, or computed by an expression.
#[derive(Clone, Debug, PartialEq)]
pub enum CompName {
    Name(Name),
    /// An NCName (processing-instruction target, namespace prefix).
    NcName(String),
    Expr(Box<ExprS>),
}

/// The pieces of a string constructor (``` ``[ … `{ expr }` … ]`` ```).
#[derive(Clone, Debug, PartialEq)]
pub enum StringPart {
    Text(String),
    Expr(Option<ExprS>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// `a, b, c`; `()` is the empty one.
    Sequence(Vec<ExprS>),
    Literal(Literal),
    VarRef(Name),
    ContextItem,
    /// `/` (the root of the context node's tree).
    Root,
    FunctionCall {
        name: Name,
        args: Vec<Arg>,
    },
    NamedFunctionRef {
        name: Name,
        arity: u8,
    },
    InlineFunction {
        params: Vec<Param>,
        return_type: Option<SequenceType>,
        body: Box<ExprS>,
        annotations: Vec<Annotation>,
    },
    DynamicCall {
        function: Box<ExprS>,
        args: Vec<Arg>,
    },
    Filter {
        base: Box<ExprS>,
        predicate: Box<ExprS>,
    },
    Lookup {
        base: Box<ExprS>,
        key: Key,
    },
    UnaryLookup(Key),
    /// `a/b`: `right` is evaluated with each item of `left` as the context.
    Path {
        left: Box<ExprS>,
        right: Box<ExprS>,
    },
    AxisStep {
        axis: Axis,
        test: NodeTest,
        predicates: Vec<ExprS>,
    },
    Binary {
        op: BinOp,
        left: Box<ExprS>,
        right: Box<ExprS>,
    },
    Unary {
        minus: bool,
        expr: Box<ExprS>,
    },
    SimpleMap {
        left: Box<ExprS>,
        right: Box<ExprS>,
    },
    /// `base => f(args)`: `function` is a name or an expression (a variable or a parenthesized expression).
    Arrow {
        base: Box<ExprS>,
        function: Box<ArrowTarget>,
        args: Vec<Arg>,
    },
    InstanceOf {
        expr: Box<ExprS>,
        type_: SequenceType,
    },
    Treat {
        expr: Box<ExprS>,
        type_: SequenceType,
    },
    Castable {
        expr: Box<ExprS>,
        type_: SingleType,
    },
    Cast {
        expr: Box<ExprS>,
        type_: SingleType,
    },
    If {
        condition: Box<ExprS>,
        then: Box<ExprS>,
        else_: Box<ExprS>,
    },
    Switch {
        operand: Box<ExprS>,
        cases: Vec<(Vec<ExprS>, ExprS)>,
        default: Box<ExprS>,
    },
    Typeswitch {
        operand: Box<ExprS>,
        cases: Vec<TypeCase>,
        default_var: Option<VarBinding>,
        default: Box<ExprS>,
    },
    Try {
        body: Box<ExprS>,
        catches: Vec<Catch>,
    },
    Quantified {
        every: bool,
        bindings: Vec<(VarBinding, ExprS)>,
        satisfies: Box<ExprS>,
    },
    Flwor {
        clauses: Vec<Spanned<Clause>>,
        ret: Box<ExprS>,
    },
    MapConstructor(Vec<(ExprS, ExprS)>),
    /// `[a, b]`: each expression a member.
    SquareArray(Vec<ExprS>),
    /// `array { a, b }`: each item a member.
    CurlyArray(Option<Box<ExprS>>),
    StringConstructor(Vec<StringPart>),
    /// `validate { … }`: read so that the error says what it is, refused when compiled (XQST0075).
    Validate(Box<ExprS>),
    DirElement {
        name: Name,
        namespaces: Vec<(String, String)>,
        attributes: Vec<DirAttribute>,
        content: Vec<DirContent>,
    },
    DirComment(String),
    DirPi {
        target: String,
        content: String,
    },
    CompDocument(Box<ExprS>),
    CompElement {
        name: CompName,
        content: Option<Box<ExprS>>,
    },
    CompAttribute {
        name: CompName,
        content: Option<Box<ExprS>>,
    },
    CompNamespace {
        prefix: CompName,
        uri: Option<Box<ExprS>>,
    },
    CompText(Box<ExprS>),
    CompComment(Box<ExprS>),
    CompPi {
        target: CompName,
        content: Option<Box<ExprS>>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ArrowTarget {
    Name(Name),
    Expr(ExprS),
}

impl Expr {
    /// The direct sub-expressions (for walks over the tree that must not recurse, such as the depth check).
    pub fn children(&self) -> Vec<&ExprS> {
        let mut out: Vec<&ExprS> = Vec::new();
        fn push_args<'a>(out: &mut Vec<&'a ExprS>, args: &'a [Arg]) {
            for a in args {
                if let Arg::Expr(e) = a {
                    out.push(e);
                }
            }
        }
        fn push_key<'a>(out: &mut Vec<&'a ExprS>, key: &'a Key) {
            if let Key::Expr(e) = key {
                out.push(e);
            }
        }
        match self {
            Expr::Sequence(v) | Expr::SquareArray(v) => out.extend(v.iter()),
            Expr::Literal(_) | Expr::VarRef(_) | Expr::ContextItem | Expr::Root | Expr::NamedFunctionRef { .. } => {}
            Expr::DirComment(_) | Expr::DirPi { .. } => {}
            Expr::FunctionCall { args, .. } => push_args(&mut out, args),
            Expr::InlineFunction { body, .. } => out.push(body),
            Expr::DynamicCall { function, args } => {
                out.push(function);
                push_args(&mut out, args);
            }
            Expr::Filter { base, predicate } => {
                out.push(base);
                out.push(predicate);
            }
            Expr::Lookup { base, key } => {
                out.push(base);
                push_key(&mut out, key);
            }
            Expr::UnaryLookup(key) => push_key(&mut out, key),
            Expr::Path { left, right } | Expr::Binary { left, right, .. } | Expr::SimpleMap { left, right } => {
                out.push(left);
                out.push(right);
            }
            Expr::AxisStep { predicates, .. } => out.extend(predicates.iter()),
            Expr::Unary { expr, .. }
            | Expr::InstanceOf { expr, .. }
            | Expr::Treat { expr, .. }
            | Expr::Castable { expr, .. }
            | Expr::Cast { expr, .. }
            | Expr::Validate(expr)
            | Expr::CompDocument(expr)
            | Expr::CompText(expr)
            | Expr::CompComment(expr) => out.push(expr),
            Expr::Arrow { base, function, args } => {
                out.push(base);
                if let ArrowTarget::Expr(e) = function.as_ref() {
                    out.push(e);
                }
                push_args(&mut out, args);
            }
            Expr::If { condition, then, else_ } => {
                out.push(condition);
                out.push(then);
                out.push(else_);
            }
            Expr::Switch { operand, cases, default } => {
                out.push(operand);
                for (values, ret) in cases {
                    out.extend(values.iter());
                    out.push(ret);
                }
                out.push(default);
            }
            Expr::Typeswitch { operand, cases, default, .. } => {
                out.push(operand);
                out.extend(cases.iter().map(|c| &c.ret));
                out.push(default);
            }
            Expr::Try { body, catches } => {
                out.push(body);
                out.extend(catches.iter().map(|c| &c.body));
            }
            Expr::Quantified { bindings, satisfies, .. } => {
                out.extend(bindings.iter().map(|(_, e)| e));
                out.push(satisfies);
            }
            Expr::Flwor { clauses, ret } => {
                for clause in clauses {
                    match &clause.value {
                        Clause::For { expr, .. } | Clause::Let { expr, .. } | Clause::Where(expr) => out.push(expr),
                        Clause::Window(w) => {
                            out.push(&w.expr);
                            out.push(&w.start.when);
                            if let Some((_, end)) = &w.end {
                                out.push(&end.when);
                            }
                        }
                        Clause::GroupBy(specs) => out.extend(specs.iter().filter_map(|s| s.expr.as_ref())),
                        Clause::OrderBy { specs, .. } => out.extend(specs.iter().map(|s| &s.expr)),
                        Clause::Count(_) => {}
                    }
                }
                out.push(ret);
            }
            Expr::MapConstructor(entries) => {
                for (k, v) in entries {
                    out.push(k);
                    out.push(v);
                }
            }
            Expr::CurlyArray(e) => out.extend(e.as_deref()),
            Expr::StringConstructor(parts) => {
                for p in parts {
                    if let StringPart::Expr(Some(e)) = p {
                        out.push(e);
                    }
                }
            }
            Expr::DirElement { attributes, content, .. } => {
                for a in attributes {
                    for p in &a.value {
                        if let AttrPart::Expr(e) = p {
                            out.push(e);
                        }
                    }
                }
                for c in content {
                    match c {
                        DirContent::Enclosed(Some(e)) | DirContent::Node(e) => out.push(e),
                        _ => {}
                    }
                }
            }
            Expr::CompElement { name, content } | Expr::CompAttribute { name, content } | Expr::CompPi { target: name, content } => {
                if let CompName::Expr(e) = name {
                    out.push(e);
                }
                out.extend(content.as_deref());
            }
            Expr::CompNamespace { prefix, uri } => {
                if let CompName::Expr(e) = prefix {
                    out.push(e);
                }
                out.extend(uri.as_deref());
            }
        }
        out
    }
}

/// A name as a query writes it: `prefix:local`, or `local` (an unprefixed name in a namespace is written `local` too).
pub fn written(name: &Name) -> String {
    use xot::xmlname::NameStrInfo;
    if name.prefix().is_empty() { name.local_name().to_owned() } else { format!("{}:{}", name.prefix(), name.local_name()) }
}
