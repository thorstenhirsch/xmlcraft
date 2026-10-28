//! Reading a schema document: the `xs:schema` element and everything in it, into the model of [`crate::model`], with the mistakes of
//! the document reported (the "representation constraints" of XML Schema Part 1, chapter 3: which elements and attributes may be
//! where, what their values are, which attributes may not be given together).
//!
//! The reader is total: whatever the document says, it gives a model and diagnostics. What could not be read is left out of the model.

use std::collections::HashSet;
use std::ops::Range;

use xmlcraft_datatypes::{collapse, is_any_uri, is_ncname};
use xmlcraft_syntax::{Code, Diagnostic, parse};

use crate::dom::{self, Attr, Dom, El};
use crate::model::*;
use crate::values;

/// The namespace of the schema language.
pub const XSD_NAMESPACE: &str = "http://www.w3.org/2001/XMLSchema";

/// What reading a document gave.
#[derive(Debug)]
pub struct Read {
    /// The document, if its root is an `xs:schema`.
    pub document: Option<SchemaDocument>,
    /// Well-formedness problems of the document, and the problems of the schema language found while reading it, by position.
    pub diagnostics: Vec<Diagnostic>,
}

/// Read the schema document `text`, which was found at `location`.
pub fn read(text: &str, location: &str) -> Read {
    let parsed = parse(text);
    let mut diagnostics: Vec<Diagnostic> = parsed.diagnostics().to_vec();
    let dom = dom::build(&parsed.syntax());
    if let Some(why) = dom.truncated {
        diagnostics.push(Diagnostic::new(Code::XsdLimit, 0..0, vec![why.to_owned()]));
    }
    let mut reader = Reader { dom: &dom, diags: Vec::new(), ids: HashSet::new() };
    let document = reader.document();
    diagnostics.extend(reader.diags);
    diagnostics.sort_by_key(|d| (d.span.start, d.span.end));
    Read {
        document: document.map(|mut d| {
            d.location = location.to_owned();
            d
        }),
        diagnostics,
    }
}

/// What the children of an element may be: one slot after the other, each with the names it takes and how many.
struct Slot {
    names: &'static [&'static str],
    min: usize,
    max: usize,
    /// For the message when the slot is not filled.
    needs: &'static str,
}

const ANNOTATION: Slot = Slot { names: &["annotation"], min: 0, max: 1, needs: "" };
const MANY: usize = usize::MAX;

struct Reader<'a> {
    dom: &'a Dom,
    diags: Vec<Diagnostic>,
    ids: HashSet<String>,
}

fn is_xs(el: &El) -> bool {
    el.namespace.as_deref() == Some(XSD_NAMESPACE)
}

impl<'a> Reader<'a> {
    fn el(&self, i: usize) -> Option<&'a El> {
        self.dom.elements.get(i)
    }

    fn report(&mut self, code: Code, span: Range<usize>, args: &[&str]) {
        self.diags.push(Diagnostic::new(code, span, args.iter().map(|a| (*a).to_owned()).collect()));
    }

    // ---- attributes ----

    fn attr(&self, i: usize, name: &str) -> Option<&'a Attr> {
        self.el(i)?.attrs.iter().find(|a| a.namespace.is_none() && a.local == name)
    }

    /// Report the attributes that are not in `allowed` (`id` is allowed on every element, attributes of other namespaces anywhere),
    /// and check the `id`.
    fn attrs(&mut self, i: usize, allowed: &[&str]) {
        let Some(el) = self.el(i) else { return };
        for a in &el.attrs {
            match &a.namespace {
                None if a.local == "id" => {
                    let id = collapse(&a.value);
                    if !is_ncname(&id) {
                        self.report(Code::XsdBadValue, a.value_range.clone(), &["id", &format!("'{}' is not an NCName", a.value)]);
                    } else if !self.ids.insert(id.clone()) {
                        self.report(Code::XsdDuplicateId, a.value_range.clone(), &[&id]);
                    }
                }
                None if allowed.contains(&a.local.as_str()) => {}
                None => self.report(Code::XsdUnknownAttribute, a.name_range.clone(), &[&a.local, &el.local]),
                Some(ns) if ns == XSD_NAMESPACE => self.report(Code::XsdUnknownAttribute, a.name_range.clone(), &[&a.local, &el.local]),
                Some(_) => {}
            }
        }
    }

    fn require(&mut self, i: usize, name: &str) -> Option<&'a Attr> {
        let found = self.attr(i, name);
        if found.is_none()
            && let Some(el) = self.el(i)
        {
            self.report(Code::XsdMissingAttribute, el.name_range.clone(), &[&el.local, name]);
        }
        found
    }

    /// The value of the attribute `name`, parsed; a value that does not parse is reported.
    fn value<T>(&mut self, i: usize, name: &str, parse: impl FnOnce(&str) -> Result<T, String>) -> Option<T> {
        let a = self.attr(i, name)?;
        match parse(&a.value) {
            Ok(v) => Some(v),
            Err(why) => {
                self.report(Code::XsdBadValue, a.value_range.clone(), &[name, &why]);
                None
            }
        }
    }

    fn name_attr(&mut self, i: usize, required: bool) -> (Option<String>, Range<usize>) {
        let a = if required { self.require(i, "name") } else { self.attr(i, "name") };
        let Some(a) = a else { return (None, self.el(i).map_or(0..0, |e| e.name_range.clone())) };
        let name = collapse(&a.value);
        if !is_ncname(&name) {
            self.report(Code::XsdBadValue, a.value_range.clone(), &["name", &format!("'{}' is not an NCName", a.value)]);
            return (None, a.value_range.clone());
        }
        (Some(name), a.value_range.clone())
    }

    fn qname_attr(&mut self, i: usize, name: &str) -> Option<QName> {
        let dom = self.dom;
        self.value(i, name, |v| values::qname(dom, i, v))
    }

    fn uri_attr(&mut self, i: usize, name: &str) -> Option<String> {
        self.value(i, name, |v| {
            let v = collapse(v);
            if is_any_uri(&v) { Ok(v) } else { Err(format!("'{v}' is not a URI reference")) }
        })
    }

    fn constraint(&self, i: usize, name: &str) -> Option<ValueConstraint> {
        let a = self.attr(i, name)?;
        Some(ValueConstraint { value: a.value.clone(), namespaces: self.dom.in_scope(i), span: a.value_range.clone() })
    }

    // ---- children ----

    fn xs_children(&self, i: usize) -> Vec<usize> {
        self.el(i).map(|e| e.children.clone()).unwrap_or_default()
    }

    /// Assign the children of `parent` to the `slots` in order; report the ones that fit nowhere and the slots left short.
    fn sequence(&mut self, parent: usize, slots: &[Slot]) -> Vec<Vec<usize>> {
        let mut out: Vec<Vec<usize>> = slots.iter().map(|_| Vec::new()).collect();
        let Some(pel) = self.el(parent) else { return out };
        if !is_text_container(pel) && !pel.text.trim().is_empty() {
            self.report(Code::XsdUnexpectedText, pel.name_range.clone(), &[&pel.local]);
        }
        let mut s = 0usize;
        for c in self.xs_children(parent) {
            let Some(child) = self.el(c) else { continue };
            if !is_xs(child) {
                self.report(Code::XsdUnexpectedElement, child.name_range.clone(), &[&child.qname, &pel.local]);
                continue;
            }
            loop {
                let Some(slot) = slots.get(s) else {
                    self.report(Code::XsdUnexpectedElement, child.name_range.clone(), &[&child.local, &pel.local]);
                    break;
                };
                let taken = out.get(s).map_or(0, Vec::len);
                if slot.names.contains(&child.local.as_str()) {
                    if taken < slot.max {
                        if let Some(list) = out.get_mut(s) {
                            list.push(c);
                        }
                    } else {
                        self.report(Code::XsdDuplicateChild, child.name_range.clone(), &[&pel.local, &child.local]);
                    }
                    break;
                }
                if taken >= slot.min {
                    s += 1;
                    continue;
                }
                self.report(Code::XsdUnexpectedElement, child.name_range.clone(), &[&child.local, &pel.local]);
                break;
            }
        }
        for (slot, taken) in slots.iter().zip(&out) {
            if taken.len() < slot.min {
                self.report(Code::XsdMissingContent, pel.name_range.clone(), &[&pel.local, slot.needs]);
            }
        }
        out
    }

    fn annotation_of(&mut self, found: &[usize]) -> Option<Annotation> {
        found.first().map(|&i| self.annotation(i))
    }

    fn annotation(&mut self, i: usize) -> Annotation {
        self.attrs(i, &[]);
        let mut out = Annotation { span: self.el(i).map_or(0..0, |e| e.range.clone()), ..Annotation::default() };
        for c in self.xs_children(i) {
            let Some(child) = self.el(c) else { continue };
            if !is_xs(child) {
                self.report(Code::XsdUnexpectedElement, child.name_range.clone(), &[&child.qname, "annotation"]);
                continue;
            }
            match child.local.as_str() {
                "documentation" => {
                    self.attrs(c, &["source"]);
                    // `xml:lang` is an attribute of another namespace and always allowed.
                    let lang = child
                        .attrs
                        .iter()
                        .find(|a| a.local == "lang" && a.namespace.as_deref() == Some(xmlcraft_syntax::XML_NAMESPACE))
                        .map(|a| a.value.clone());
                    out.documentation.push(Documentation { source: self.attr(c, "source").map(|a| a.value.clone()), lang, text: self.deep_text(c) });
                }
                "appinfo" => {
                    self.attrs(c, &["source"]);
                    out.appinfo += 1;
                }
                other => self.report(Code::XsdUnexpectedElement, child.name_range.clone(), &[other, "annotation"]),
            }
        }
        out
    }

    /// The text of an element and its descendants.
    fn deep_text(&self, i: usize) -> String {
        let mut out = String::new();
        let mut stack = vec![i];
        while let Some(at) = stack.pop() {
            let Some(el) = self.el(at) else { continue };
            out.push_str(&el.text);
            stack.extend(el.children.iter().rev());
        }
        out
    }

    // ---- the document ----

    fn document(&mut self) -> Option<SchemaDocument> {
        let root = self.dom.root()?;
        let el = self.el(root)?;
        if !(is_xs(el) && el.local == "schema") {
            self.report(Code::XsdNotASchema, el.name_range.clone(), &[&el.qname]);
            return None;
        }
        self.attrs(root, &["targetNamespace", "version", "elementFormDefault", "attributeFormDefault", "blockDefault", "finalDefault"]);
        let target_namespace = self.attr(root, "targetNamespace").map(|a| collapse(&a.value));
        if let Some(a) = self.attr(root, "targetNamespace")
            && (target_namespace.as_deref() == Some("") || !is_any_uri(&collapse(&a.value)))
        {
            self.report(Code::XsdBadValue, a.value_range.clone(), &["targetNamespace", "it must be a non-empty URI reference"]);
        }
        let version = self.attr(root, "version").map(|a| a.value.clone());
        let id = self.attr(root, "id").map(|a| collapse(&a.value));
        let element_form_default = self.value(root, "elementFormDefault", values::form).unwrap_or(Form::Unqualified);
        let attribute_form_default = self.value(root, "attributeFormDefault", values::form).unwrap_or(Form::Unqualified);
        let block_default =
            self.value(root, "blockDefault", |v| values::derivation_set(v, &["extension", "restriction", "substitution"])).unwrap_or_default();
        let final_default =
            self.value(root, "finalDefault", |v| values::derivation_set(v, &["extension", "restriction", "list", "union"])).unwrap_or_default();
        let mut doc = SchemaDocument {
            location: String::new(),
            span: el.range.clone(),
            target_namespace,
            element_form_default,
            attribute_form_default,
            block_default,
            final_default,
            version,
            id,
            annotations: Vec::new(),
            items: Vec::new(),
        };
        // Includes, imports, redefinitions and annotations come first; then the declarations, each with annotations after it.
        let mut declarations_started = false;
        for c in self.xs_children(root) {
            let Some(child) = self.el(c) else { continue };
            if !is_xs(child) {
                self.report(Code::XsdUnexpectedElement, child.name_range.clone(), &[&child.qname, "schema"]);
                continue;
            }
            match child.local.as_str() {
                "annotation" => {
                    let a = self.annotation(c);
                    doc.annotations.push(a);
                }
                "include" | "import" | "redefine" if declarations_started => {
                    self.report(Code::XsdUnexpectedElement, child.name_range.clone(), &[&child.local, "schema"]);
                }
                "include" => doc.items.extend(self.include(c).map(Item::Include)),
                "import" => doc.items.push(Item::Import(self.import(c))),
                "redefine" => doc.items.extend(self.redefine(c).map(Item::Redefine)),
                "simpleType" => {
                    declarations_started = true;
                    doc.items.push(Item::SimpleType(self.simple_type(c, true)));
                }
                "complexType" => {
                    declarations_started = true;
                    doc.items.push(Item::ComplexType(self.complex_type(c, true)));
                }
                "group" => {
                    declarations_started = true;
                    doc.items.push(Item::Group(self.group_def(c)));
                }
                "attributeGroup" => {
                    declarations_started = true;
                    doc.items.push(Item::AttributeGroup(self.attribute_group_def(c)));
                }
                "element" => {
                    declarations_started = true;
                    doc.items.push(Item::Element(self.element(c, true)));
                }
                "attribute" => {
                    declarations_started = true;
                    doc.items.push(Item::Attribute(self.attribute(c, true)));
                }
                "notation" => {
                    declarations_started = true;
                    doc.items.push(Item::Notation(self.notation(c)));
                }
                other => self.report(Code::XsdUnexpectedElement, child.name_range.clone(), &[other, "schema"]),
            }
        }
        Some(doc)
    }

    fn include(&mut self, i: usize) -> Option<Include> {
        self.attrs(i, &["schemaLocation"]);
        let found = self.sequence(i, &[ANNOTATION]);
        self.annotation_of(&found[0]);
        let location = self.uri_attr(i, "schemaLocation");
        let a = self.require(i, "schemaLocation");
        let _ = a;
        Some(Include { schema_location: location?, span: self.el(i)?.range.clone() })
    }

    fn import(&mut self, i: usize) -> Import {
        self.attrs(i, &["namespace", "schemaLocation"]);
        let found = self.sequence(i, &[ANNOTATION]);
        self.annotation_of(&found[0]);
        let namespace = self.uri_attr(i, "namespace");
        let schema_location = self.uri_attr(i, "schemaLocation");
        Import { namespace, schema_location, span: self.el(i).map_or(0..0, |e| e.range.clone()) }
    }

    fn redefine(&mut self, i: usize) -> Option<Redefine> {
        self.attrs(i, &["schemaLocation"]);
        let location = self.uri_attr(i, "schemaLocation");
        self.require(i, "schemaLocation");
        let mut items = Vec::new();
        for c in self.xs_children(i) {
            let Some(child) = self.el(c) else { continue };
            if !is_xs(child) {
                self.report(Code::XsdUnexpectedElement, child.name_range.clone(), &[&child.qname, "redefine"]);
                continue;
            }
            match child.local.as_str() {
                "annotation" => {
                    self.annotation(c);
                }
                "simpleType" => items.push(Item::SimpleType(self.simple_type(c, true))),
                "complexType" => items.push(Item::ComplexType(self.complex_type(c, true))),
                "group" => items.push(Item::Group(self.group_def(c))),
                "attributeGroup" => items.push(Item::AttributeGroup(self.attribute_group_def(c))),
                other => self.report(Code::XsdUnexpectedElement, child.name_range.clone(), &[other, "redefine"]),
            }
        }
        Some(Redefine { schema_location: location?, items, span: self.el(i)?.range.clone() })
    }

    fn notation(&mut self, i: usize) -> NotationDef {
        self.attrs(i, &["name", "public", "system"]);
        let found = self.sequence(i, &[ANNOTATION]);
        let annotation = self.annotation_of(&found[0]);
        let (name, name_span) = self.name_attr(i, true);
        let public = self.attr(i, "public").map(|a| collapse(&a.value));
        let system = self.uri_attr(i, "system");
        if public.is_none()
            && system.is_none()
            && let Some(el) = self.el(i)
        {
            self.report(Code::XsdMissingAttribute, el.name_range.clone(), &["notation", "public or system"]);
        }
        NotationDef { name, public, system, annotation, span: self.el(i).map_or(0..0, |e| e.range.clone()), name_span }
    }
}

/// Elements that hold text: `documentation` and `appinfo` hold anything, and the elements of other namespaces are not ours.
fn is_text_container(el: &El) -> bool {
    !is_xs(el) || matches!(el.local.as_str(), "documentation" | "appinfo")
}

const FACETS: &[&str] = &[
    "minExclusive",
    "minInclusive",
    "maxExclusive",
    "maxInclusive",
    "totalDigits",
    "fractionDigits",
    "length",
    "minLength",
    "maxLength",
    "enumeration",
    "whiteSpace",
    "pattern",
];

const SIMPLE_TYPE_SLOT: Slot = Slot { names: &["simpleType"], min: 0, max: 1, needs: "" };
const FACET_SLOT: Slot = Slot { names: FACETS, min: 0, max: MANY, needs: "" };
const ATTRIBUTE_SLOT: Slot = Slot { names: &["attribute", "attributeGroup"], min: 0, max: MANY, needs: "" };
const ANY_ATTRIBUTE_SLOT: Slot = Slot { names: &["anyAttribute"], min: 0, max: 1, needs: "" };
const PARTICLE_SLOT: Slot = Slot { names: &["group", "all", "choice", "sequence"], min: 0, max: 1, needs: "" };

impl Reader<'_> {
    // ---- simple types ----

    fn simple_type(&mut self, i: usize, top: bool) -> SimpleTypeDef {
        self.attrs(i, if top { &["name", "final"] } else { &[] });
        let found = self
            .sequence(i, &[ANNOTATION, Slot { names: &["restriction", "list", "union"], min: 1, max: 1, needs: "a restriction, a list or a union" }]);
        let annotation = self.annotation_of(&found[0]);
        let (name, name_span) = if top { self.name_attr(i, true) } else { (None, self.el(i).map_or(0..0, |e| e.name_range.clone())) };
        let final_ = self.value(i, "final", |v| values::derivation_set(v, &["list", "union", "restriction"]));
        let kind = match found[1].first().and_then(|&c| self.el(c).map(|e| (c, e.local.as_str()))) {
            Some((c, "restriction")) => self.simple_restriction(c),
            Some((c, "list")) => self.simple_list(c),
            Some((c, "union")) => self.simple_union(c),
            _ => SimpleTypeKind::Missing,
        };
        SimpleTypeDef { name, final_, kind, annotation, span: self.el(i).map_or(0..0, |e| e.range.clone()), name_span }
    }

    fn simple_restriction(&mut self, i: usize) -> SimpleTypeKind {
        self.attrs(i, &["base"]);
        let found = self.sequence(i, &[ANNOTATION, SIMPLE_TYPE_SLOT, FACET_SLOT]);
        self.annotation_of(&found[0]);
        let base = self.qname_attr(i, "base");
        let inline_base = found[1].first().map(|&c| Box::new(self.simple_type(c, false)));
        match (self.attr(i, "base").is_some(), inline_base.is_some()) {
            (true, true) => {
                if let Some(el) = self.el(i) {
                    self.report(Code::XsdConflict, el.name_range.clone(), &["base", "a simpleType child"]);
                }
            }
            (false, false) => {
                if let Some(el) = self.el(i) {
                    self.report(Code::XsdMissingContent, el.name_range.clone(), &["restriction", "a base attribute or a simpleType child"]);
                }
            }
            _ => {}
        }
        let facets = found[2].iter().filter_map(|&c| self.facet(c)).collect();
        SimpleTypeKind::Restriction { base, inline_base, facets }
    }

    fn facet(&mut self, i: usize) -> Option<FacetDef> {
        let el = self.el(i)?;
        let kind = FacetKind::from_local(&el.local)?;
        self.attrs(i, if matches!(kind, FacetKind::Enumeration | FacetKind::Pattern) { &["value"] } else { &["value", "fixed"] });
        let found = self.sequence(i, &[ANNOTATION]);
        self.annotation_of(&found[0]);
        let value = self.require(i, "value")?;
        let fixed = self.value(i, "fixed", values::boolean).unwrap_or(false);
        Some(FacetDef {
            kind,
            value: value.value.clone(),
            fixed,
            namespaces: self.dom.in_scope(i),
            span: el.range.clone(),
            value_span: value.value_range.clone(),
        })
    }

    fn simple_list(&mut self, i: usize) -> SimpleTypeKind {
        self.attrs(i, &["itemType"]);
        let found = self.sequence(i, &[ANNOTATION, SIMPLE_TYPE_SLOT]);
        self.annotation_of(&found[0]);
        let item_type = self.qname_attr(i, "itemType");
        let inline_item = found[1].first().map(|&c| Box::new(self.simple_type(c, false)));
        match (self.attr(i, "itemType").is_some(), inline_item.is_some()) {
            (true, true) => {
                if let Some(el) = self.el(i) {
                    self.report(Code::XsdConflict, el.name_range.clone(), &["itemType", "a simpleType child"]);
                }
            }
            (false, false) => {
                if let Some(el) = self.el(i) {
                    self.report(Code::XsdMissingContent, el.name_range.clone(), &["list", "an itemType attribute or a simpleType child"]);
                }
            }
            _ => {}
        }
        SimpleTypeKind::List { item_type, inline_item }
    }

    fn simple_union(&mut self, i: usize) -> SimpleTypeKind {
        self.attrs(i, &["memberTypes"]);
        let found = self.sequence(i, &[ANNOTATION, Slot { names: &["simpleType"], min: 0, max: MANY, needs: "" }]);
        self.annotation_of(&found[0]);
        let dom = self.dom;
        let member_types = self.value(i, "memberTypes", |v| values::qname_list(dom, i, v)).unwrap_or_default();
        let inline_members: Vec<SimpleTypeDef> = found[1].iter().map(|&c| self.simple_type(c, false)).collect();
        if self.attr(i, "memberTypes").is_none()
            && inline_members.is_empty()
            && let Some(el) = self.el(i)
        {
            self.report(Code::XsdMissingContent, el.name_range.clone(), &["union", "a memberTypes attribute or simpleType children"]);
        }
        SimpleTypeKind::Union { member_types, inline_members }
    }

    // ---- complex types ----

    fn complex_type(&mut self, i: usize, top: bool) -> ComplexTypeDef {
        self.attrs(i, if top { &["name", "abstract", "mixed", "final", "block"] } else { &["mixed"] });
        let found = self.sequence(
            i,
            &[
                ANNOTATION,
                Slot { names: &["simpleContent", "complexContent"], min: 0, max: 1, needs: "" },
                PARTICLE_SLOT,
                ATTRIBUTE_SLOT,
                ANY_ATTRIBUTE_SLOT,
            ],
        );
        let annotation = self.annotation_of(&found[0]);
        let (name, name_span) = if top { self.name_attr(i, true) } else { (None, self.el(i).map_or(0..0, |e| e.name_range.clone())) };
        let abstract_ = self.value(i, "abstract", values::boolean);
        let mixed = self.value(i, "mixed", values::boolean);
        let block = self.value(i, "block", |v| values::derivation_set(v, &["extension", "restriction"]));
        let final_ = self.value(i, "final", |v| values::derivation_set(v, &["extension", "restriction"]));
        let content = match found[1].first().and_then(|&c| self.el(c).map(|e| (c, e.local.as_str()))) {
            Some((c, kind)) => {
                // Nothing may follow `simpleContent` or `complexContent` but its own children.
                for extra in found[2].iter().chain(&found[3]).chain(&found[4]) {
                    if let (Some(e), Some(parent)) = (self.el(*extra), self.el(i)) {
                        self.report(Code::XsdUnexpectedElement, e.name_range.clone(), &[&e.local, &parent.local]);
                    }
                }
                if kind == "simpleContent" { self.simple_content(c) } else { self.complex_content(c) }
            }
            None => {
                let particle = found[2].first().and_then(|&c| self.particle(c, false));
                ComplexContentDef::Implicit { particle, attributes: self.attribute_uses(&found[3], &found[4]) }
            }
        };
        ComplexTypeDef { name, abstract_, mixed, block, final_, content, annotation, span: self.el(i).map_or(0..0, |e| e.range.clone()), name_span }
    }

    fn method_of(&self, c: usize) -> Option<DerivationMethod> {
        match self.el(c)?.local.as_str() {
            "restriction" => Some(DerivationMethod::Restriction),
            "extension" => Some(DerivationMethod::Extension),
            _ => None,
        }
    }

    fn simple_content(&mut self, i: usize) -> ComplexContentDef {
        self.attrs(i, &[]);
        let found =
            self.sequence(i, &[ANNOTATION, Slot { names: &["restriction", "extension"], min: 1, max: 1, needs: "a restriction or an extension" }]);
        self.annotation_of(&found[0]);
        let Some(&c) = found[1].first() else {
            return ComplexContentDef::Simple {
                method: DerivationMethod::Extension,
                base: None,
                inline_base: None,
                facets: Vec::new(),
                attributes: AttributeUses::default(),
            };
        };
        let method = self.method_of(c).unwrap_or(DerivationMethod::Extension);
        self.attrs(c, &["base"]);
        let found = if method == DerivationMethod::Restriction {
            self.sequence(c, &[ANNOTATION, SIMPLE_TYPE_SLOT, FACET_SLOT, ATTRIBUTE_SLOT, ANY_ATTRIBUTE_SLOT])
        } else {
            let f = self.sequence(c, &[ANNOTATION, ATTRIBUTE_SLOT, ANY_ATTRIBUTE_SLOT]);
            vec![f[0].clone(), Vec::new(), Vec::new(), f[1].clone(), f[2].clone()]
        };
        self.annotation_of(&found[0]);
        self.require(c, "base");
        let base = self.qname_attr(c, "base");
        let inline_base = found[1].first().map(|&s| Box::new(self.simple_type(s, false)));
        let facets = found[2].iter().filter_map(|&f| self.facet(f)).collect();
        let attributes = self.attribute_uses(&found[3], &found[4]);
        ComplexContentDef::Simple { method, base, inline_base, facets, attributes }
    }

    fn complex_content(&mut self, i: usize) -> ComplexContentDef {
        self.attrs(i, &["mixed"]);
        let mixed = self.value(i, "mixed", values::boolean);
        let found =
            self.sequence(i, &[ANNOTATION, Slot { names: &["restriction", "extension"], min: 1, max: 1, needs: "a restriction or an extension" }]);
        self.annotation_of(&found[0]);
        let Some(&c) = found[1].first() else {
            return ComplexContentDef::Complex {
                method: DerivationMethod::Extension,
                base: None,
                mixed,
                particle: None,
                attributes: AttributeUses::default(),
            };
        };
        let method = self.method_of(c).unwrap_or(DerivationMethod::Extension);
        self.attrs(c, &["base"]);
        let f = self.sequence(c, &[ANNOTATION, PARTICLE_SLOT, ATTRIBUTE_SLOT, ANY_ATTRIBUTE_SLOT]);
        self.annotation_of(&f[0]);
        self.require(c, "base");
        let base = self.qname_attr(c, "base");
        let particle = f[1].first().and_then(|&p| self.particle(p, false));
        ComplexContentDef::Complex { method, base, mixed, particle, attributes: self.attribute_uses(&f[2], &f[3]) }
    }

    fn attribute_uses(&mut self, uses: &[usize], wildcard: &[usize]) -> AttributeUses {
        let mut out = AttributeUses::default();
        for &c in uses {
            match self.el(c).map(|e| e.local.as_str()) {
                Some("attribute") => out.attributes.push(self.attribute(c, false)),
                Some("attributeGroup") => {
                    self.attrs(c, &["ref"]);
                    let found = self.sequence(c, &[ANNOTATION]);
                    self.annotation_of(&found[0]);
                    self.require(c, "ref");
                    if let Some(reference) = self.qname_attr(c, "ref") {
                        out.groups.push(AttributeGroupRef { reference, span: self.el(c).map_or(0..0, |e| e.range.clone()) });
                    }
                }
                _ => {}
            }
        }
        out.wildcard = wildcard.first().map(|&c| self.wildcard(c, false));
        out
    }

    // ---- particles ----

    fn occurs(&mut self, i: usize) -> (u32, MaxOccurs) {
        let min = self.value(i, "minOccurs", values::non_negative).unwrap_or(1);
        let max = self.value(i, "maxOccurs", values::max_occurs).unwrap_or(MaxOccurs::Bounded(1));
        if let MaxOccurs::Bounded(m) = max
            && m < min
            && let Some(a) = self.attr(i, "minOccurs")
        {
            self.report(Code::XsdOccurs, a.value_range.clone(), &[&min.to_string(), &m.to_string()]);
        }
        (min, max)
    }

    /// A particle: a local element, a group reference, a model group or a wildcard. `in_group` is a model group written directly in a
    /// group definition, which has no occurrence of its own.
    fn particle(&mut self, i: usize, in_group: bool) -> Option<Particle> {
        let el = self.el(i)?;
        let span = el.range.clone();
        let local = el.local.clone();
        let (min_occurs, max_occurs, term) = match local.as_str() {
            "element" => {
                let (min, max) = self.occurs(i);
                (min, max, Term::Element(Box::new(self.element(i, false))))
            }
            "group" => {
                self.attrs(i, &["ref", "minOccurs", "maxOccurs"]);
                let found = self.sequence(i, &[ANNOTATION]);
                self.annotation_of(&found[0]);
                self.require(i, "ref");
                let (min, max) = self.occurs(i);
                (min, max, Term::GroupRef(self.qname_attr(i, "ref")?))
            }
            "all" | "choice" | "sequence" => {
                self.attrs(i, if in_group { &[] } else { &["minOccurs", "maxOccurs"] });
                let (min, max) = if in_group { (1, MaxOccurs::Bounded(1)) } else { self.occurs(i) };
                let names: &'static [&'static str] = if local == "all" { &["element"] } else { &["element", "group", "choice", "sequence", "any"] };
                let found = self.sequence(i, &[ANNOTATION, Slot { names, min: 0, max: MANY, needs: "" }]);
                self.annotation_of(&found[0]);
                let children: Vec<Particle> = found[1].iter().filter_map(|&c| self.particle(c, false)).collect();
                let term = match local.as_str() {
                    "all" => Term::All(children),
                    "choice" => Term::Choice(children),
                    _ => Term::Sequence(children),
                };
                (min, max, term)
            }
            "any" => {
                let (min, max) = self.occurs(i);
                (min, max, Term::Any(self.wildcard(i, true)))
            }
            _ => return None,
        };
        Some(Particle { min_occurs, max_occurs, term, span })
    }

    fn wildcard(&mut self, i: usize, particle: bool) -> Wildcard {
        self.attrs(i, if particle { &["namespace", "processContents", "minOccurs", "maxOccurs"] } else { &["namespace", "processContents"] });
        let found = self.sequence(i, &[ANNOTATION]);
        self.annotation_of(&found[0]);
        let namespace = self.value(i, "namespace", values::namespace_constraint).unwrap_or(NamespaceConstraint::Any);
        let process_contents = self.value(i, "processContents", values::process_contents).unwrap_or(ProcessContents::Strict);
        Wildcard { namespace, process_contents, span: self.el(i).map_or(0..0, |e| e.range.clone()) }
    }

    fn group_def(&mut self, i: usize) -> GroupDef {
        self.attrs(i, &["name"]);
        let found =
            self.sequence(i, &[ANNOTATION, Slot { names: &["all", "choice", "sequence"], min: 1, max: 1, needs: "an all, a choice or a sequence" }]);
        let annotation = self.annotation_of(&found[0]);
        let (name, name_span) = self.name_attr(i, true);
        let content = found[1].first().and_then(|&c| self.particle(c, true));
        GroupDef { name, content, annotation, span: self.el(i).map_or(0..0, |e| e.range.clone()), name_span }
    }

    fn attribute_group_def(&mut self, i: usize) -> AttributeGroupDef {
        self.attrs(i, &["name"]);
        let found = self.sequence(i, &[ANNOTATION, ATTRIBUTE_SLOT, ANY_ATTRIBUTE_SLOT]);
        let annotation = self.annotation_of(&found[0]);
        let (name, name_span) = self.name_attr(i, true);
        let uses = self.attribute_uses(&found[1], &found[2]);
        AttributeGroupDef { name, uses, annotation, span: self.el(i).map_or(0..0, |e| e.range.clone()), name_span }
    }

    // ---- elements and attributes ----

    fn element(&mut self, i: usize, top: bool) -> ElementDecl {
        self.attrs(
            i,
            if top {
                &["name", "type", "substitutionGroup", "default", "fixed", "nillable", "abstract", "block", "final"]
            } else {
                &["name", "ref", "type", "default", "fixed", "nillable", "block", "form", "minOccurs", "maxOccurs"]
            },
        );
        let found = self.sequence(
            i,
            &[
                ANNOTATION,
                Slot { names: &["simpleType", "complexType"], min: 0, max: 1, needs: "" },
                Slot { names: &["unique", "key", "keyref"], min: 0, max: MANY, needs: "" },
            ],
        );
        let annotation = self.annotation_of(&found[0]);
        let reference = if top { None } else { self.qname_attr(i, "ref") };
        let has_ref = self.attr(i, "ref").is_some();
        let (name, name_span) = if has_ref { (None, self.el(i).map_or(0..0, |e| e.name_range.clone())) } else { self.name_attr(i, true) };
        if has_ref && let Some(a) = self.attr(i, "name") {
            self.report(Code::XsdConflict, a.name_range.clone(), &["name", "ref"]);
        }
        let type_name = self.qname_attr(i, "type");
        let inline_type = found[1].first().and_then(|&c| match self.el(c).map(|e| e.local.as_str()) {
            Some("simpleType") => Some(Box::new(InlineType::Simple(self.simple_type(c, false)))),
            Some("complexType") => Some(Box::new(InlineType::Complex(self.complex_type(c, false)))),
            _ => None,
        });
        if self.attr(i, "type").is_some()
            && inline_type.is_some()
            && let Some(a) = self.attr(i, "type")
        {
            self.report(Code::XsdConflict, a.name_range.clone(), &["type", "a simpleType or complexType child"]);
        }
        if has_ref {
            for forbidden in ["type", "default", "fixed", "nillable", "form", "block"] {
                if let Some(a) = self.attr(i, forbidden) {
                    self.report(Code::XsdConflict, a.name_range.clone(), &[forbidden, "ref"]);
                }
            }
            if inline_type.is_some() || !found[2].is_empty() {
                let span = found[1].first().or(found[2].first()).and_then(|&c| self.el(c)).map(|e| e.name_range.clone());
                if let Some(span) = span {
                    self.report(Code::XsdConflict, span, &["a child of element", "ref"]);
                }
            }
        }
        let default = self.constraint(i, "default");
        let fixed = self.constraint(i, "fixed");
        if let (Some(_), Some(f)) = (&default, &fixed) {
            self.report(Code::XsdConflict, f.span.clone(), &["fixed", "default"]);
        }
        let identity_constraints = found[2].iter().filter_map(|&c| self.identity_constraint(c)).collect();
        ElementDecl {
            name,
            reference,
            type_name,
            inline_type,
            substitution_group: if top { self.qname_attr(i, "substitutionGroup") } else { None },
            default,
            fixed,
            nillable: self.value(i, "nillable", values::boolean),
            abstract_: if top { self.value(i, "abstract", values::boolean) } else { None },
            block: self.value(i, "block", |v| values::derivation_set(v, &["extension", "restriction", "substitution"])),
            final_: if top { self.value(i, "final", |v| values::derivation_set(v, &["extension", "restriction"])) } else { None },
            form: if top { None } else { self.value(i, "form", values::form) },
            identity_constraints,
            annotation,
            span: self.el(i).map_or(0..0, |e| e.range.clone()),
            name_span,
        }
    }

    fn attribute(&mut self, i: usize, top: bool) -> AttributeDecl {
        self.attrs(i, if top { &["name", "type", "default", "fixed"] } else { &["name", "ref", "type", "use", "default", "fixed", "form"] });
        let found = self.sequence(i, &[ANNOTATION, SIMPLE_TYPE_SLOT]);
        let annotation = self.annotation_of(&found[0]);
        let has_ref = self.attr(i, "ref").is_some();
        let reference = if top { None } else { self.qname_attr(i, "ref") };
        let (name, name_span) = if has_ref { (None, self.el(i).map_or(0..0, |e| e.name_range.clone())) } else { self.name_attr(i, true) };
        if has_ref && let Some(a) = self.attr(i, "name") {
            self.report(Code::XsdConflict, a.name_range.clone(), &["name", "ref"]);
        }
        if name.as_deref() == Some("xmlns")
            && let Some(a) = self.attr(i, "name")
        {
            self.report(Code::XsdBadValue, a.value_range.clone(), &["name", "an attribute cannot be named 'xmlns'"]);
        }
        let type_name = self.qname_attr(i, "type");
        let inline_type = found[1].first().map(|&c| Box::new(self.simple_type(c, false)));
        if self.attr(i, "type").is_some()
            && inline_type.is_some()
            && let Some(a) = self.attr(i, "type")
        {
            self.report(Code::XsdConflict, a.name_range.clone(), &["type", "a simpleType child"]);
        }
        if has_ref {
            for forbidden in ["type", "form"] {
                if let Some(a) = self.attr(i, forbidden) {
                    self.report(Code::XsdConflict, a.name_range.clone(), &[forbidden, "ref"]);
                }
            }
            if let Some(&c) = found[1].first()
                && let Some(e) = self.el(c)
            {
                self.report(Code::XsdConflict, e.name_range.clone(), &["a simpleType child", "ref"]);
            }
        }
        let use_ = self.value(i, "use", |v| match collapse(v).as_str() {
            "optional" => Ok(AttributeUse::Optional),
            "required" => Ok(AttributeUse::Required),
            "prohibited" => Ok(AttributeUse::Prohibited),
            _ => Err(format!("'{v}' is not optional, required or prohibited")),
        });
        let default = self.constraint(i, "default");
        let fixed = self.constraint(i, "fixed");
        if let (Some(_), Some(f)) = (&default, &fixed) {
            self.report(Code::XsdConflict, f.span.clone(), &["fixed", "default"]);
        }
        if let (Some(d), Some(u)) = (&default, use_)
            && u != AttributeUse::Optional
        {
            self.report(Code::XsdConflict, d.span.clone(), &["default", "use other than optional"]);
        }
        AttributeDecl {
            name,
            reference,
            type_name,
            inline_type,
            use_,
            default,
            fixed,
            form: if top { None } else { self.value(i, "form", values::form) },
            annotation,
            span: self.el(i).map_or(0..0, |e| e.range.clone()),
            name_span,
        }
    }

    fn identity_constraint(&mut self, i: usize) -> Option<IdentityConstraintDef> {
        let kind = match self.el(i)?.local.as_str() {
            "unique" => IdentityKind::Unique,
            "key" => IdentityKind::Key,
            _ => IdentityKind::Keyref,
        };
        self.attrs(i, if kind == IdentityKind::Keyref { &["name", "refer"] } else { &["name"] });
        let found = self.sequence(
            i,
            &[
                ANNOTATION,
                Slot { names: &["selector"], min: 1, max: 1, needs: "a selector" },
                Slot { names: &["field"], min: 1, max: MANY, needs: "at least one field" },
            ],
        );
        self.annotation_of(&found[0]);
        let (name, name_span) = self.name_attr(i, true);
        let refer = if kind == IdentityKind::Keyref {
            self.require(i, "refer");
            self.qname_attr(i, "refer")
        } else {
            None
        };
        let xpath = |this: &mut Self, c: usize| -> Option<(String, Range<usize>)> {
            this.attrs(c, &["xpath"]);
            let f = this.sequence(c, &[ANNOTATION]);
            this.annotation_of(&f[0]);
            let a = this.require(c, "xpath")?;
            Some((a.value.clone(), a.value_range.clone()))
        };
        let selector = found[1].first().and_then(|&c| xpath(self, c));
        let fields = found[2].iter().filter_map(|&c| xpath(self, c)).collect();
        Some(IdentityConstraintDef {
            kind,
            name,
            refer,
            selector,
            fields,
            namespaces: self.dom.in_scope(i),
            span: self.el(i)?.range.clone(),
            name_span,
        })
    }
}
