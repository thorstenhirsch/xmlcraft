//! A push validator against an assembled XML Schema: it is told the elements, text and attributes of a document in order and checks
//! them against the schema (the validity constraints of XML Schema Part 1, "Schema Validity Assessment (Element)"): the element is
//! declared, its type (or the `xsi:type` it names) allows the attributes and the content it has, simple values are valid for their
//! types and the values that are fixed are as fixed, `xsi:nil` is used where the declaration allows it, and the IDs and ID references
//! of the document match. It knows nothing of trees or files; a driver feeds it.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use xmlcraft_datatypes::{NamespaceContext, SimpleType, collapse};
use xmlcraft_grammar::Name;
use xmlcraft_syntax::{Code, Diagnostic, is_xml_space};
use xmlcraft_xsd::model::{ProcessContents, QName};
use xmlcraft_xsd::schema::{
    AttributeDecl, AttributeUse, ConstraintId, ContentType, ElementDecl, ElementId, NamespaceSet, Schema, TypeId, TypeKind, ValueConstraint, Wildcard,
};
use xmlcraft_xsd::{ContentModel, ModelState, Symbol};

use crate::xsd_identity::{RowRef, Run};

/// The namespace of `xsi:type`, `xsi:nil` and the other attributes the schema language gives every element.
pub const XSI_NAMESPACE: &str = "http://www.w3.org/2001/XMLSchema-instance";

/// How many elements may be open at once.
pub const MAX_OPEN: usize = 10_000;

/// How many names a message lists before it says there are more.
const LISTED: usize = 6;

/// An attribute of a start tag, as it was written; the declarations of namespaces are not among them.
pub struct XsdAttribute<'a> {
    /// The expanded name (an attribute without a prefix is in no namespace).
    pub name: QName,
    /// The name as it was written, for messages.
    pub shown: &'a str,
    /// The value, with the references in it replaced.
    pub value: &'a str,
    /// The whole attribute, for the problems found in it.
    pub range: Range<usize>,
}

pub(crate) enum Content<'s> {
    /// Not checked: skipped by a wildcard, or inside something that was reported.
    Skip,
    /// No type: the elements in it are checked if they are declared (a wildcard that is lax lets in an element that is not).
    Lax,
    /// The type has no content (or the element is nilled).
    Empty,
    /// Text of a simple type.
    Simple(Arc<SimpleType>),
    Elements {
        model: Option<&'s ContentModel>,
        state: ModelState,
        mixed: bool,
    },
}

pub(crate) struct Frame<'s> {
    pub(crate) shown: String,
    /// The expanded name of the element, for the paths of identity constraints.
    pub(crate) name: QName,
    /// Where the name of the element is written.
    pub(crate) range: Range<usize>,
    pub(crate) content: Content<'s>,
    /// The identity constraints the declaration has: the element is their context.
    pub(crate) constraints: &'s [ConstraintId],
    /// The attribute uses and the wildcard of the type, to find the type of an attribute.
    pub(crate) uses: Option<(&'s [AttributeUse], Option<&'s Wildcard>)>,
    pub(crate) runs: Vec<Run>,
    /// The rows and fields this element is a value of.
    pub(crate) targets: Vec<(RowRef, usize)>,
    /// The keys of the key and unique constraints made in the element and below it.
    pub(crate) tables: HashMap<ConstraintId, HashSet<String>>,
    /// `xsi:nil="true"` on an element whose declaration allows it.
    pub(crate) nil: bool,
    /// The default or fixed value of the declaration.
    pub(crate) constraint: Option<&'s ValueConstraint>,
    pub(crate) text: String,
    /// A reference to an entity was in the text, which is not known here, so the text is not checked.
    pub(crate) unknown_text: bool,
    /// What was wrong with the content has been said; nothing more is said of it.
    pub(crate) reported: bool,
}

/// What a child element is matched to.
enum Selected {
    Declared(ElementId),
    Lax,
    Skip,
}

/// A validator for the elements of one document.
pub struct XsdValidator<'s> {
    pub(crate) schema: &'s Schema,
    pub(crate) stack: Vec<Frame<'s>>,
    /// The rows of identity constraints whose node is still open, innermost last.
    pub(crate) open_rows: Vec<RowRef>,
    ids: HashMap<String, Range<usize>>,
    idrefs: Vec<(String, Range<usize>)>,
    diagnostics: Vec<Diagnostic>,
    limit_reported: bool,
}

impl<'s> XsdValidator<'s> {
    pub fn new(schema: &'s Schema) -> Self {
        Self {
            schema,
            stack: Vec::new(),
            open_rows: Vec::new(),
            ids: HashMap::new(),
            idrefs: Vec::new(),
            diagnostics: Vec::new(),
            limit_reported: false,
        }
    }

    pub(crate) fn report(&mut self, code: Code, range: Range<usize>, args: &[&str]) {
        self.diagnostics.push(Diagnostic::new(code, range, args.iter().map(|a| (*a).to_owned()).collect()));
    }

    fn push(&mut self, name: &QName, shown: &str, range: &Range<usize>, content: Content<'s>) {
        self.stack.push(Frame {
            shown: shown.to_owned(),
            name: name.clone(),
            range: range.clone(),
            content,
            constraints: &[],
            uses: None,
            runs: Vec::new(),
            targets: Vec::new(),
            tables: HashMap::new(),
            nil: false,
            constraint: None,
            text: String::new(),
            unknown_text: false,
            reported: false,
        });
    }

    /// The start of an element. `range` is where its name is written, `ns` the namespace declarations in scope at the element.
    pub fn start_element(&mut self, name: &QName, shown: &str, range: Range<usize>, attributes: &[XsdAttribute<'_>], ns: &dyn NamespaceContext) {
        self.open(name, shown, &range, attributes, ns);
        self.identity_start(&range, attributes, ns);
    }

    fn open(&mut self, name: &QName, shown: &str, range: &Range<usize>, attributes: &[XsdAttribute<'_>], ns: &dyn NamespaceContext) {
        let range = range.clone();
        if self.stack.len() >= MAX_OPEN {
            if !self.limit_reported {
                self.limit_reported = true;
                self.report(Code::CvcLimit, range.clone(), &[&format!("more than {MAX_OPEN} elements open at once")]);
            }
            self.push(name, shown, &range, Content::Skip);
            return;
        }
        let id = match self.select(name, shown, &range) {
            Selected::Declared(id) => id,
            Selected::Lax => return self.push(name, shown, &range, Content::Lax),
            Selected::Skip => return self.push(name, shown, &range, Content::Skip),
        };
        let schema = self.schema;
        let Some(decl) = schema.element(id) else { return self.push(name, shown, &range, Content::Skip) };
        if decl.abstract_ {
            self.report(Code::CvcAbstractElement, range.clone(), &[shown]);
        }
        let xsi = |local: &str| attributes.iter().find(|a| a.name.namespace.as_deref() == Some(XSI_NAMESPACE) && a.name.local == local);
        let type_id = match xsi("type") {
            Some(a) => match self.xsi_type(decl, a, shown, ns) {
                Some(t) => t,
                // The type it names cannot be used; nothing is known of what the element should be, so nothing more is said of it.
                None => return self.push(name, shown, &range, Content::Skip),
            },
            None => decl.type_id,
        };
        let nil = match xsi("nil") {
            Some(a) => self.xsi_nil(decl, a, shown),
            None => false,
        };
        let Some(entry) = schema.type_entry(type_id) else { return self.push(name, shown, &range, Content::Skip) };
        let content = match &entry.kind {
            TypeKind::Simple(t) => {
                self.attributes(shown, &range, &[], None, attributes, ns);
                if nil { Content::Empty } else { Content::Simple(t.clone()) }
            }
            TypeKind::Complex(c) => {
                if c.abstract_ {
                    let type_name = entry.name.as_ref().map_or_else(|| "(anonymous type)".to_owned(), ToString::to_string);
                    self.report(Code::CvcAbstractType, range.clone(), &[shown, &type_name]);
                }
                self.attributes(shown, &range, &c.attributes, c.attribute_wildcard.as_ref(), attributes, ns);
                if nil {
                    Content::Empty
                } else {
                    match &c.content {
                        ContentType::Empty => Content::Empty,
                        ContentType::Simple(t) => Content::Simple(t.clone()),
                        ContentType::ElementOnly(_) | ContentType::Mixed(_) => {
                            let model = schema.content_model(type_id);
                            let state = model.map(ContentModel::start);
                            let mixed = matches!(c.content, ContentType::Mixed(_));
                            match (model, state) {
                                (Some(model), Some(state)) => Content::Elements { model: Some(model), state, mixed },
                                // The model could not be compiled (that was reported with the schema): the content is not checked.
                                _ => Content::Skip,
                            }
                        }
                    }
                }
            }
        };
        self.push(name, shown, &range, content);
        if let Some(frame) = self.stack.last_mut() {
            frame.nil = nil;
            frame.constraint = decl.value_constraint.as_ref();
            frame.constraints = &decl.identity_constraints;
            frame.uses = match &entry.kind {
                TypeKind::Complex(c) => Some((&c.attributes, c.attribute_wildcard.as_ref())),
                TypeKind::Simple(_) => None,
            };
        }
    }

    /// Match a child element to a declaration, as its parent's content model says.
    fn select(&mut self, name: &QName, shown: &str, range: &Range<usize>) -> Selected {
        let schema = self.schema;
        let Some(parent) = self.stack.last_mut() else {
            // The root element has to be declared.
            return match schema.element_by_name(name) {
                Some(id) => Selected::Declared(id),
                None => {
                    self.report(Code::CvcNotDeclared, range.clone(), &[shown]);
                    Selected::Skip
                }
            };
        };
        let parent_shown = parent.shown.clone();
        match &mut parent.content {
            Content::Skip => Selected::Skip,
            Content::Lax => schema.element_by_name(name).map_or(Selected::Lax, Selected::Declared),
            Content::Empty => {
                let first = !std::mem::replace(&mut parent.reported, true);
                let nil = parent.nil;
                if first {
                    if nil {
                        self.report(Code::CvcNilContent, range.clone(), &[&parent_shown, &format!("it contains the element '{shown}'")]);
                    } else {
                        self.report(Code::CvcChildNotAllowed, range.clone(), &[shown, &format!("'{parent_shown}' must be empty")]);
                    }
                }
                Selected::Skip
            }
            Content::Simple(_) => {
                if !std::mem::replace(&mut parent.reported, true) {
                    self.report(
                        Code::CvcChildNotAllowed,
                        range.clone(),
                        &[shown, &format!("'{parent_shown}' has a simple type, so it contains text only")],
                    );
                }
                Selected::Skip
            }
            Content::Elements { model, state, .. } => {
                let Some(model) = *model else { return Selected::Skip };
                if parent.reported {
                    return Selected::Skip;
                }
                let step = model.step(state, &Name { namespace: name.namespace.clone(), local: name.local.clone() });
                match step {
                    Some((next, symbol)) => {
                        *state = next;
                        match symbol {
                            Symbol::Element(id) => Selected::Declared(*id),
                            Symbol::Wildcard(w) => match (w.process_contents, schema.element_by_name(name)) {
                                (ProcessContents::Skip, _) => Selected::Skip,
                                (_, Some(id)) => Selected::Declared(id),
                                (ProcessContents::Lax, None) => Selected::Lax,
                                (ProcessContents::Strict, None) => {
                                    self.report(Code::CvcWildcardNotDeclared, range.clone(), &[shown]);
                                    Selected::Skip
                                }
                            },
                        }
                    }
                    None => {
                        parent.reported = true;
                        let at = state.clone();
                        let expected = self.describe_expected(model, &at);
                        self.report(Code::CvcChildNotAllowed, range.clone(), &[shown, &expected]);
                        Selected::Skip
                    }
                }
            }
        }
    }

    fn describe(&self, symbol: &Symbol) -> String {
        match symbol {
            Symbol::Element(id) => self.schema.element(*id).map_or_else(String::new, |e| e.name.to_string()),
            Symbol::Wildcard(w) => match &w.constraint {
                NamespaceSet::Any => "any element".to_owned(),
                NamespaceSet::Not(_) => "any element from another namespace".to_owned(),
                NamespaceSet::List(list) => {
                    let spaces: Vec<String> = list.iter().map(|n| n.clone().unwrap_or_else(|| "no namespace".to_owned())).collect();
                    format!("any element from {}", spaces.join(" or "))
                }
            },
        }
    }

    /// `xsi:type`: the type it names, if the element may have it.
    fn xsi_type(&mut self, decl: &ElementDecl, attribute: &XsdAttribute<'_>, shown: &str, ns: &dyn NamespaceContext) -> Option<TypeId> {
        let value = collapse(attribute.value);
        let (prefix, local) = match value.split_once(':') {
            Some((prefix, local)) => (Some(prefix), local),
            None => (None, value.as_str()),
        };
        let namespace = ns.resolve(prefix);
        if let (Some(prefix), None) = (prefix, &namespace) {
            self.report(Code::CvcXsiType, attribute.range.clone(), &[shown, &format!("the prefix '{prefix}' is not declared")]);
            return None;
        }
        let name = QName { namespace, local: local.to_owned() };
        let Some(type_id) = self.schema.type_by_name(&name) else {
            self.report(Code::CvcXsiType, attribute.range.clone(), &[shown, &format!("there is no type '{name}'")]);
            return None;
        };
        if !self.schema.derivation_allowed(type_id, decl.type_id, decl.block) {
            self.report(
                Code::CvcXsiType,
                attribute.range.clone(),
                &[shown, &format!("'{name}' is not derived from the type of the element, or the derivation is blocked")],
            );
            return None;
        }
        Some(type_id)
    }

    /// `xsi:nil`: whether the element is nilled. The declaration has to allow it, and fix no value.
    fn xsi_nil(&mut self, decl: &ElementDecl, attribute: &XsdAttribute<'_>, shown: &str) -> bool {
        let nil = match collapse(attribute.value).as_str() {
            "true" | "1" => true,
            "false" | "0" => false,
            _ => {
                self.report(Code::CvcBadNil, attribute.range.clone(), &[shown, &format!("'{}' is not true or false", attribute.value)]);
                return false;
            }
        };
        if !nil {
            return false;
        }
        if !decl.nillable {
            self.report(Code::CvcNotNillable, attribute.range.clone(), &[shown]);
            return false;
        }
        if let Some(c) = decl.value_constraint.as_ref().filter(|c| c.fixed) {
            self.report(Code::CvcNilFixed, attribute.range.clone(), &[shown, &c.lexical]);
        }
        true
    }

    /// The attributes of an element against the uses its type has and the wildcard it allows.
    fn attributes(
        &mut self,
        shown: &str,
        name_range: &Range<usize>,
        uses: &'s [AttributeUse],
        wildcard: Option<&'s Wildcard>,
        attributes: &[XsdAttribute<'_>],
        ns: &dyn NamespaceContext,
    ) {
        let schema = self.schema;
        let mut present = vec![false; uses.len()];
        for attribute in attributes {
            if attribute.name.namespace.as_deref() == Some(XSI_NAMESPACE) {
                continue;
            }
            let position = uses.iter().position(|u| schema.attribute(u.attribute).is_some_and(|d| d.name == attribute.name));
            if let Some(index) = position {
                if let Some(flag) = present.get_mut(index) {
                    *flag = true;
                }
                if let Some(u) = uses.get(index)
                    && let Some(decl) = schema.attribute(u.attribute)
                {
                    self.attribute_value(attribute, decl, u.value_constraint.as_ref(), ns);
                }
                continue;
            }
            match wildcard.filter(|w| w.constraint.allows(attribute.name.namespace.as_deref())) {
                Some(w) => match (w.process_contents, schema.attribute_by_name(&attribute.name).and_then(|id| schema.attribute(id))) {
                    (ProcessContents::Skip, _) => {}
                    (_, Some(decl)) => self.attribute_value(attribute, decl, None, ns),
                    (ProcessContents::Lax, None) => {}
                    (ProcessContents::Strict, None) => self.report(Code::CvcAttributeNotDeclared, attribute.range.clone(), &[shown, attribute.shown]),
                },
                None => self.report(Code::CvcAttributeNotAllowed, attribute.range.clone(), &[shown, attribute.shown]),
            }
        }
        for (u, here) in uses.iter().zip(&present) {
            if u.required
                && !here
                && let Some(decl) = schema.attribute(u.attribute)
            {
                self.report(Code::CvcAttributeRequired, name_range.clone(), &[shown, &decl.name.to_string()]);
            }
        }
    }

    fn attribute_value(
        &mut self,
        attribute: &XsdAttribute<'_>,
        decl: &AttributeDecl,
        use_constraint: Option<&ValueConstraint>,
        ns: &dyn NamespaceContext,
    ) {
        let what = format!("the attribute '{}'", attribute.shown);
        match decl.type_.validate(attribute.value, ns) {
            Err(violation) => {
                self.report(Code::CvcInvalidValue, attribute.range.clone(), &[&what, &format!("{}: {}", violation.kind.code(), violation.message)]);
            }
            Ok(value) => {
                if let Some(fixed) = use_constraint.or(decl.value_constraint.as_ref()).filter(|c| c.fixed)
                    && !value.equals(&fixed.value)
                {
                    self.report(Code::CvcFixedValue, attribute.range.clone(), &[&what, &fixed.lexical]);
                }
                self.track_ids(&decl.type_, attribute.value, &attribute.range);
            }
        }
    }

    /// Remember the IDs and the references to them.
    fn track_ids(&mut self, ty: &SimpleType, lexical: &str, range: &Range<usize>) {
        if ty.is_derived_from_builtin("ID") {
            let id = collapse(lexical);
            match self.ids.entry(id) {
                Entry::Occupied(taken) => {
                    let id = taken.key().clone();
                    self.report(Code::CvcDuplicateId, range.clone(), &[&id]);
                }
                Entry::Vacant(free) => {
                    free.insert(range.clone());
                }
            }
        } else if ty.is_derived_from_builtin("IDREF") {
            self.idrefs.push((collapse(lexical), range.clone()));
        } else if ty.is_derived_from_builtin("IDREFS") {
            for token in lexical.split(is_xml_space).filter(|t| !t.is_empty()) {
                self.idrefs.push((token.to_owned(), range.clone()));
            }
        }
    }

    /// Text in the open element.
    pub fn characters(&mut self, text: &str, range: Range<usize>) {
        let Some(frame) = self.stack.last_mut() else { return };
        match &frame.content {
            Content::Skip | Content::Lax => {}
            Content::Simple(_) => frame.text.push_str(text),
            Content::Empty => {
                if !std::mem::replace(&mut frame.reported, true) {
                    let shown = frame.shown.clone();
                    if frame.nil {
                        self.report(Code::CvcNilContent, range, &[&shown, "it contains text"]);
                    } else {
                        self.report(Code::CvcTextNotAllowed, range, &[&shown, "its content must be empty"]);
                    }
                }
            }
            Content::Elements { mixed: false, .. } => {
                if !text.chars().all(is_xml_space) && !std::mem::replace(&mut frame.reported, true) {
                    let shown = frame.shown.clone();
                    self.report(Code::CvcTextNotAllowed, range, &[&shown, "it has element content only"]);
                }
            }
            Content::Elements { mixed: true, .. } => {}
        }
    }

    /// A reference to an entity in the open element: what it stands for is not known here.
    pub fn unknown_text(&mut self) {
        if let Some(frame) = self.stack.last_mut() {
            frame.unknown_text = true;
        }
    }

    /// The end of the open element; `close` is where its end tag is, and `ns` the declarations in scope at it.
    pub fn end_element(&mut self, close: Range<usize>, ns: &dyn NamespaceContext) {
        self.identity_node_end(ns);
        let Some(frame) = self.stack.pop() else { return };
        self.content_end(&frame, &close, ns);
        self.identity_context_end(frame);
    }

    /// What the content of the element that ends has to be.
    fn content_end(&mut self, frame: &Frame<'s>, close: &Range<usize>, ns: &dyn NamespaceContext) {
        match &frame.content {
            Content::Skip | Content::Lax | Content::Empty => {}
            Content::Simple(ty) => {
                if frame.unknown_text || frame.reported {
                    return;
                }
                self.simple_content(&frame.shown, &frame.text, ty, frame.constraint, &frame.range, ns);
            }
            Content::Elements { model: Some(model), state, .. } => {
                if !frame.reported && !model.is_accepting(state) {
                    let expected = self.describe_expected(model, state);
                    self.report(Code::CvcContentIncomplete, close.clone(), &[&frame.shown, &expected]);
                }
            }
            Content::Elements { model: None, .. } => {}
        }
    }

    fn describe_expected(&self, model: &ContentModel, state: &ModelState) -> String {
        let next = model.expected(state);
        let mandatory: Vec<&_> = next.iter().filter(|n| n.mandatory).collect();
        let list: Vec<&_> = if mandatory.is_empty() { next.iter().collect() } else { mandatory };
        let mut names: Vec<String> = Vec::new();
        for n in list {
            let described = self.describe(n.symbol);
            if !names.contains(&described) {
                names.push(described);
            }
        }
        match names.len() {
            0 => "no more elements are allowed".to_owned(),
            1 => format!("expected '{}'", names.join("")),
            n => {
                let shown: Vec<String> = names.iter().take(LISTED).map(|n| format!("'{n}'")).collect();
                let more = if n > LISTED { format!(" or {} more", n - LISTED) } else { String::new() };
                format!("expected one of {}{more}", shown.join(", "))
            }
        }
    }

    /// The text of an element with a simple type (or simple content); problems are reported at `at`, the name of the element.
    fn simple_content(
        &mut self,
        shown: &str,
        text: &str,
        ty: &SimpleType,
        constraint: Option<&ValueConstraint>,
        at: &Range<usize>,
        ns: &dyn NamespaceContext,
    ) {
        let what = format!("the element '{shown}'");
        // An element with no text takes the default or the fixed value of its declaration.
        if text.is_empty() && constraint.is_some() {
            return;
        }
        match ty.validate(text, ns) {
            Err(violation) => {
                self.report(Code::CvcInvalidValue, at.clone(), &[&what, &format!("{}: {}", violation.kind.code(), violation.message)]);
            }
            Ok(value) => {
                if let Some(fixed) = constraint.filter(|c| c.fixed)
                    && !value.equals(&fixed.value)
                {
                    self.report(Code::CvcFixedValue, at.clone(), &[&what, &fixed.lexical]);
                }
                self.track_ids(ty, text, at);
            }
        }
    }

    /// The end of the document: the references to IDs that nobody has.
    pub fn finish(mut self) -> Vec<Diagnostic> {
        let missing: Vec<(String, Range<usize>)> = self.idrefs.iter().filter(|(id, _)| !self.ids.contains_key(id)).cloned().collect();
        for (id, range) in missing {
            self.report(Code::CvcIdrefNotFound, range, &[&id]);
        }
        self.diagnostics
    }
}
