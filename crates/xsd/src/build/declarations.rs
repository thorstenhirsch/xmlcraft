//! Element and attribute declarations, attribute groups, value constraints and identity constraints.

use std::rc::Rc;

use super::wildcards::intersect_wildcards;
use super::*;
use crate::schema as s;
use crate::xpath::{ConstraintPaths, parse_field, parse_selector};

impl<'a> Builder<'a> {
    // ---- elements ----

    /// Reserve the number of a global element, so that it can be referred to before it is built.
    pub(super) fn register_element(&mut self, doc: usize, place: Place, e: &ElementDecl) {
        let Some(name) = &e.name else { return };
        let qname = self.named(doc, name);
        if self.element_places.contains_key(&qname) {
            self.report(doc, Code::XsdDuplicateComponent, e.name_span.clone(), &["element", &qname.to_string()]);
            return;
        }
        let id = ElementId(self.schema.elements.len());
        self.schema.elements.push(s::ElementDecl {
            name: qname.clone(),
            type_id: self.any_type(),
            scope: None,
            nillable: false,
            value_constraint: None,
            abstract_: false,
            substitution_group: None,
            block: DerivationSet::default(),
            final_: DerivationSet::default(),
            identity_constraints: Vec::new(),
            origin: Self::origin(doc, &e.span),
        });
        self.schema.global_elements.insert(qname.clone(), id);
        self.element_places.insert(qname, place);
    }

    pub(super) fn build_global_elements(&mut self) {
        let mut places: Vec<(Place, QName)> = self.element_places.iter().map(|(n, p)| (*p, n.clone())).collect();
        places.sort();
        for (_, name) in places {
            if let Some(&id) = self.schema.global_elements.get(&name) {
                let _ = self.ensure_element(id);
            }
        }
    }

    /// Make sure the global element `id` is built. One that is being built is in a circular substitution group.
    pub(super) fn ensure_element(&mut self, id: ElementId) -> Result<(), Lookup> {
        if self.element_built.contains(&id) {
            return Ok(());
        }
        let Some(name) = self.schema.elements.get(id.0).map(|e| e.name.clone()) else { return Ok(()) };
        let key = (Kind::Element, name.clone());
        if self.failed.contains(&key) {
            return Err(Lookup::Failed);
        }
        if self.progress.contains(&key) {
            return Err(Lookup::Circular);
        }
        if self.depth >= MAX_DEPTH {
            return Err(Lookup::TooDeep);
        }
        let Some(&place) = self.element_places.get(&name) else { return Ok(()) };
        let Some(Item::Element(decl)) = self.item(place) else { return Ok(()) };
        self.progress.insert(key.clone());
        self.depth += 1;
        let built = self.element_decl(place.0, decl, None, Some(id)).is_some();
        self.depth -= 1;
        self.progress.remove(&key);
        if built {
            self.element_built.insert(id);
            Ok(())
        } else {
            self.failed.insert(key);
            Err(Lookup::Failed)
        }
    }

    /// The number of the global element a reference names.
    pub(super) fn global_element(&mut self, doc: usize, reference: &QRef) -> Option<ElementId> {
        let name = self.visible_name(doc, reference, "element")?;
        match self.schema.global_elements.get(&name) {
            Some(&id) => Some(id),
            None => {
                self.report(doc, Code::XsdUnresolved, reference.span.clone(), &["element", &name.to_string()]);
                None
            }
        }
    }

    /// Build an element declaration: a global one into the number reserved for it (`slot`), or a local one of the type `scope`.
    pub(super) fn element_decl(&mut self, doc: usize, e: &'a ElementDecl, scope: Option<TypeId>, slot: Option<ElementId>) -> Option<ElementId> {
        let name = e.name.as_deref()?;
        let global = slot.is_some();
        let document = self.document(doc)?;
        let qname = if global || e.form.unwrap_or(document.document.element_form_default) == Form::Qualified {
            self.named(doc, name)
        } else {
            QName { namespace: None, local: name.to_owned() }
        };
        let origin = Self::origin(doc, &e.span);
        let head = match (&e.substitution_group, global) {
            (Some(reference), true) => self.global_element(doc, reference).map(|id| (id, reference)),
            _ => None,
        };
        let mut substitution_group = None;
        if let (Some((head_id, reference)), Some(slot_id)) = (head, slot) {
            match self.ensure_element(head_id) {
                Err(Lookup::Circular) => {
                    self.report(
                        doc,
                        Code::XsdCircular,
                        reference.span.clone(),
                        &[&qname.to_string(), "the element is in the substitution group of itself"],
                    );
                }
                Err(Lookup::TooDeep) => self.report(doc, Code::XsdTooDeep, reference.span.clone(), &[&qname.to_string()]),
                // A head that could not be built was reported; the member still belongs to its group.
                Ok(()) | Err(Lookup::Failed) => {
                    substitution_group = Some(head_id);
                    self.pending_substitutions.push((doc, slot_id, head_id, e.span.clone()));
                }
            }
        }
        let type_id = match (&e.type_name, e.inline_type.as_deref()) {
            (Some(reference), _) => self.resolve_type(doc, reference),
            (None, Some(InlineType::Simple(def))) => self.simple_type(doc, def, None, None),
            (None, Some(InlineType::Complex(def))) => {
                let id = self.reserve_type(Self::origin(doc, &def.span));
                self.queue.push(Job { document: doc, def, id });
                Some(id)
            }
            (None, None) => substitution_group.and_then(|head_id| self.schema.elements.get(head_id.0)).map(|h| h.type_id),
        }
        .unwrap_or_else(|| self.any_type());
        let defaults = document.document.block_default;
        let finals = document.document.final_default;
        let decl = s::ElementDecl {
            name: qname,
            type_id,
            scope: if global { None } else { scope },
            nillable: e.nillable.unwrap_or(false),
            value_constraint: None,
            abstract_: e.abstract_.unwrap_or(false),
            substitution_group,
            block: e.block.unwrap_or(DerivationSet {
                extension: defaults.extension,
                restriction: defaults.restriction,
                substitution: defaults.substitution,
                ..DerivationSet::default()
            }),
            final_: if global {
                e.final_.unwrap_or(DerivationSet { extension: finals.extension, restriction: finals.restriction, ..DerivationSet::default() })
            } else {
                DerivationSet::default()
            },
            identity_constraints: Vec::new(),
            origin,
        };
        let id = match slot {
            Some(id) if id.0 < self.schema.elements.len() => {
                if let Some(place) = self.schema.elements.get_mut(id.0) {
                    *place = decl;
                }
                id
            }
            _ => {
                let id = ElementId(self.schema.elements.len());
                self.schema.elements.push(decl);
                id
            }
        };
        let constraints = self.identity_constraints(doc, e);
        if let Some(decl) = self.schema.elements.get_mut(id.0) {
            decl.identity_constraints = constraints;
        }
        if let Some((constraint, fixed)) = e.fixed.as_ref().map(|c| (c, true)).or_else(|| e.default.as_ref().map(|c| (c, false))) {
            self.pending.push(PendingValue { document: doc, element: id, constraint: constraint.clone(), fixed });
        }
        Some(id)
    }

    // ---- value constraints ----

    /// A default or fixed value of something called `shown`, checked against its type.
    pub(super) fn value_constraint(
        &mut self,
        doc: usize,
        shown: &QName,
        v: &ValueConstraint,
        fixed: bool,
        ty: &Arc<SimpleType>,
    ) -> Option<s::ValueConstraint> {
        if ty.is_derived_from_builtin("ID") {
            self.report(
                doc,
                Code::XsdValueConstraint,
                v.span.clone(),
                &[&shown.to_string(), "a value of type ID cannot have a default or a fixed value"],
            );
            return None;
        }
        match ty.validate(&v.value, &Scope(&v.namespaces)) {
            Ok(value) => Some(s::ValueConstraint { value, lexical: v.value.clone(), fixed }),
            Err(violation) => {
                self.report(doc, Code::XsdValueConstraint, v.span.clone(), &[&shown.to_string(), &violation.to_string()]);
                None
            }
        }
    }

    /// Check the default and fixed values of elements, now that their types are complete.
    pub(super) fn finish_value_constraints(&mut self) {
        for p in std::mem::take(&mut self.pending) {
            let Some(decl) = self.schema.elements.get(p.element.0) else { continue };
            let name = decl.name.clone();
            let ty = match self.schema.types.get(decl.type_id.0).map(|t| &t.kind) {
                Some(TypeKind::Simple(t)) => Some(t.clone()),
                Some(TypeKind::Complex(c)) => match &c.content {
                    ContentType::Simple(t) => Some(t.clone()),
                    ContentType::Mixed(content) if Self::emptiable(&content.particle) => SimpleType::builtin("string"),
                    _ => None,
                },
                None => None,
            };
            let Some(ty) = ty else {
                self.report(
                    p.document,
                    Code::XsdValueConstraint,
                    p.constraint.span.clone(),
                    &[&name.to_string(), "its type does not have text content"],
                );
                continue;
            };
            if let Some(constraint) = self.value_constraint(p.document, &name, &p.constraint, p.fixed, &ty)
                && let Some(decl) = self.schema.elements.get_mut(p.element.0)
            {
                decl.value_constraint = Some(constraint);
            }
        }
    }

    /// The substitution groups: a member's type must come from the head's.
    pub(super) fn finish_substitutions(&mut self) {
        for (doc, member, head, span) in std::mem::take(&mut self.pending_substitutions) {
            let (Some(m), Some(h)) = (self.schema.elements.get(member.0), self.schema.elements.get(head.0)) else { continue };
            if !self.schema.derives_from(m.type_id, h.type_id) {
                let shown = m.name.to_string();
                let head_name = h.name.to_string();
                self.report(
                    doc,
                    Code::XsdComponent,
                    span,
                    &[&shown, &format!("its type is not derived from the type of '{head_name}', the head of its substitution group")],
                );
            }
        }
    }

    pub(super) fn build_notations(&mut self) {
        let mut places: Vec<(Place, QName)> = self.notation_places.iter().map(|(n, p)| (*p, n.clone())).collect();
        places.sort();
        for (place, name) in places {
            let Some(Item::Notation(n)) = self.item(place) else { continue };
            self.schema.global_notations.insert(name.clone(), self.schema.notations.len());
            self.schema.notations.push(s::NotationDecl {
                name,
                public: n.public.clone(),
                system: n.system.clone(),
                origin: Self::origin(place.0, &n.span),
            });
        }
    }

    // ---- identity constraints ----

    fn identity_constraints(&mut self, doc: usize, e: &'a ElementDecl) -> Vec<ConstraintId> {
        let mut ids = Vec::new();
        for c in &e.identity_constraints {
            let (Some(name), Some((selector, selector_span))) = (c.name.as_deref(), c.selector.as_ref()) else { continue };
            let qname = self.named(doc, name);
            if self.constraint_names.contains_key(&qname) {
                self.report(doc, Code::XsdDuplicateComponent, c.name_span.clone(), &["identity constraint", &qname.to_string()]);
                continue;
            }
            let paths = self.constraint_paths(doc, &qname, selector, selector_span, c);
            let id = ConstraintId(self.schema.constraints.len());
            self.schema.constraints.push(s::IdentityConstraint {
                kind: c.kind,
                name: qname.clone(),
                refer: None,
                selector: selector.clone(),
                fields: c.fields.iter().map(|(f, _)| f.clone()).collect(),
                namespaces: c.namespaces.clone(),
                paths,
                origin: Self::origin(doc, &c.span),
            });
            self.constraint_names.insert(qname, id);
            if c.kind == IdentityKind::Keyref
                && let Some(refer) = &c.refer
            {
                self.pending_refers.push(PendingRefer { document: doc, keyref: id, refer: refer.clone() });
            }
            ids.push(id);
        }
        ids
    }

    /// The selector and the fields of a constraint, parsed; what is wrong with them is reported.
    fn constraint_paths(
        &mut self,
        doc: usize,
        name: &QName,
        selector: &str,
        selector_span: &Range<usize>,
        c: &IdentityConstraintDef,
    ) -> Option<ConstraintPaths> {
        let shown = format!("the identity constraint '{name}'");
        let mut ok = true;
        let selector = match parse_selector(selector, &c.namespaces) {
            Ok(path) => Some(path),
            Err(problem) => {
                self.report(doc, Code::XsdComponent, selector_span.clone(), &[&shown, &format!("its selector is not valid: {problem}")]);
                ok = false;
                None
            }
        };
        let mut fields = Vec::new();
        for (text, span) in &c.fields {
            match parse_field(text, &c.namespaces) {
                Ok(path) => fields.push(path),
                Err(problem) => {
                    self.report(doc, Code::XsdComponent, span.clone(), &[&shown, &format!("the field '{text}' is not valid: {problem}")]);
                    ok = false;
                }
            }
        }
        match (ok, selector) {
            (true, Some(selector)) => Some(ConstraintPaths { selector, fields }),
            _ => None,
        }
    }

    /// Resolve what the keyrefs refer to.
    pub(super) fn finish_constraint_refs(&mut self) {
        for p in std::mem::take(&mut self.pending_refers) {
            let name = self.qualify(p.document, &p.refer.name);
            let Some(&target) = self.constraint_names.get(&name) else {
                self.report(p.document, Code::XsdUnresolved, p.refer.span.clone(), &["key or unique constraint", &name.to_string()]);
                continue;
            };
            let (Some(keyref), Some(referred)) = (self.schema.constraints.get(p.keyref.0), self.schema.constraints.get(target.0)) else { continue };
            if referred.kind == IdentityKind::Keyref {
                self.report(
                    p.document,
                    Code::XsdWrongKind,
                    p.refer.span.clone(),
                    &[&name.to_string(), "a keyref, where a key or unique constraint is needed"],
                );
            } else if referred.fields.len() != keyref.fields.len() {
                let (have, need) = (keyref.fields.len().to_string(), referred.fields.len().to_string());
                self.report(
                    p.document,
                    Code::XsdComponent,
                    p.refer.span.clone(),
                    &[&keyref.name.to_string(), &format!("it has {have} fields, but '{name}' has {need}")],
                );
            } else if let Some(keyref) = self.schema.constraints.get_mut(p.keyref.0) {
                keyref.refer = Some(target);
            }
        }
    }

    // ---- attributes ----

    pub(super) fn register_attribute(&mut self, doc: usize, place: Place, a: &AttributeDecl) {
        let Some(name) = &a.name else { return };
        let qname = self.named(doc, name);
        if !claim(&mut self.attribute_places, &qname, place) {
            self.report(doc, Code::XsdDuplicateComponent, a.name_span.clone(), &["attribute", &qname.to_string()]);
        }
    }

    pub(super) fn any_simple_arc(&self) -> Option<Arc<SimpleType>> {
        let id = self.any_simple_type()?;
        self.simple_arc(id)
    }

    pub(super) fn simple_arc(&self, id: TypeId) -> Option<Arc<SimpleType>> {
        match self.schema.types.get(id.0).map(|t| &t.kind) {
            Some(TypeKind::Simple(t)) => Some(t.clone()),
            _ => None,
        }
    }

    /// Build the attribute declaration `a`, global or local to a type.
    fn attribute_decl(&mut self, doc: usize, a: &'a AttributeDecl, global: bool) -> Option<AttributeId> {
        let name = a.name.as_deref()?;
        let document = self.document(doc)?;
        let qname = if global || a.form.unwrap_or(document.document.attribute_form_default) == Form::Qualified {
            self.named(doc, name)
        } else {
            QName { namespace: None, local: name.to_owned() }
        };
        // An attribute whose type cannot be found still exists, with the type that allows anything.
        let ty = match (&a.type_name, a.inline_type.as_deref()) {
            (Some(reference), _) => self.resolve_simple(doc, reference).map(|(_, t)| t),
            (None, Some(def)) => self.simple_type(doc, def, None, None).and_then(|id| self.simple_arc(id)),
            (None, None) => None,
        }
        .or_else(|| self.any_simple_arc())?;
        let value_constraint = a
            .fixed
            .as_ref()
            .map(|c| (c, true))
            .or_else(|| a.default.as_ref().map(|c| (c, false)))
            .and_then(|(c, fixed)| self.value_constraint(doc, &qname, c, fixed, &ty));
        let id = AttributeId(self.schema.attributes.len());
        self.schema.attributes.push(s::AttributeDecl {
            name: qname.clone(),
            type_: ty,
            value_constraint,
            global,
            origin: Self::origin(doc, &a.span),
        });
        if global {
            self.schema.global_attributes.insert(qname, id);
        }
        Some(id)
    }

    /// The number of the global attribute a reference names; it is built when first needed.
    fn global_attribute(&mut self, doc: usize, reference: &QRef) -> Option<AttributeId> {
        let name = self.visible_name(doc, reference, "attribute")?;
        if let Some(&id) = self.schema.global_attributes.get(&name) {
            return Some(id);
        }
        let key = (Kind::Attribute, name.clone());
        if self.failed.contains(&key) {
            return None;
        }
        let Some(&place) = self.attribute_places.get(&name) else {
            self.report(doc, Code::XsdUnresolved, reference.span.clone(), &["attribute", &name.to_string()]);
            return None;
        };
        let Some(Item::Attribute(decl)) = self.item(place) else { return None };
        let built = self.attribute_decl(place.0, decl, true);
        if built.is_none() {
            self.failed.insert(key);
        }
        built
    }

    /// Build the global attributes nothing refers to as well.
    pub(super) fn build_global_attributes(&mut self) {
        let mut places: Vec<(Place, QName)> = self.attribute_places.iter().map(|(n, p)| (*p, n.clone())).collect();
        places.sort();
        for (place, name) in places {
            if !self.schema.global_attributes.contains_key(&name)
                && let Some(Item::Attribute(decl)) = self.item(place)
            {
                let _ = self.attribute_decl(place.0, decl, true);
            }
        }
    }

    /// The attribute uses and the wildcard an attribute list gives, attribute groups expanded.
    pub(super) fn attribute_set(&mut self, doc: usize, uses: &'a AttributeUses) -> AttributeSet {
        let mut set = AttributeSet::default();
        let mut wildcard = uses.wildcard.as_ref().map(|w| (self.wildcard(doc, w), w.span.clone()));
        for a in &uses.attributes {
            self.add_attribute(doc, a, &mut set);
        }
        for group in &uses.groups {
            let Some(inner) = self.attribute_group(doc, &group.reference) else { continue };
            self.merge_uses(doc, &group.span, &mut set, &inner);
            if let Some(w) = &inner.wildcard {
                wildcard = match wildcard {
                    None => Some((w.clone(), group.span.clone())),
                    Some((own, span)) => match intersect_wildcards(&own, w) {
                        Some(both) => Some((both, span)),
                        None => {
                            self.report(
                                doc,
                                Code::XsdComponent,
                                group.span.clone(),
                                &["attribute wildcard", "the wildcards of the attribute groups have no intersection that can be written"],
                            );
                            Some((own, span))
                        }
                    },
                };
            }
        }
        set.wildcard = wildcard.map(|(w, _)| w);
        set
    }

    fn add_attribute(&mut self, doc: usize, a: &'a AttributeDecl, set: &mut AttributeSet) {
        let use_ = a.use_.unwrap_or(AttributeUse::Optional);
        let (id, value_constraint) = if let Some(reference) = &a.reference {
            let Some(id) = self.global_attribute(doc, reference) else { return };
            let ty = self.schema.attributes.get(id.0).map(|d| (d.name.clone(), d.type_.clone()));
            let constraint = match (ty, a.fixed.as_ref().map(|c| (c, true)).or_else(|| a.default.as_ref().map(|c| (c, false)))) {
                (Some((name, ty)), Some((c, fixed))) => self.value_constraint(doc, &name, c, fixed, &ty),
                _ => None,
            };
            (id, constraint)
        } else {
            let Some(id) = self.attribute_decl(doc, a, false) else { return };
            (id, None)
        };
        let Some(name) = self.schema.attributes.get(id.0).map(|d| d.name.clone()) else { return };
        if use_ == AttributeUse::Prohibited {
            set.prohibited.push(name);
            return;
        }
        self.push_use(doc, &a.span, set, SAttributeUse { attribute: id, required: use_ == AttributeUse::Required, value_constraint });
    }

    /// Add a use to `set`; an attribute can be used once.
    pub(super) fn push_use(&mut self, doc: usize, span: &Range<usize>, set: &mut AttributeSet, new: SAttributeUse) {
        let name = self.schema.attributes.get(new.attribute.0).map(|d| d.name.clone());
        let taken = set.uses.iter().any(|u| self.schema.attributes.get(u.attribute.0).map(|d| &d.name) == name.as_ref());
        if taken {
            let shown = name.map(|n| n.to_string()).unwrap_or_default();
            self.report(doc, Code::XsdComponent, span.clone(), &[&format!("attribute '{shown}'"), "it is used more than once"]);
        } else {
            set.uses.push(new);
        }
    }

    fn merge_uses(&mut self, doc: usize, span: &Range<usize>, into: &mut AttributeSet, from: &AttributeSet) {
        for u in &from.uses {
            self.push_use(doc, span, into, u.clone());
        }
        into.prohibited.extend(from.prohibited.iter().cloned());
    }

    /// An attribute group, expanded; asked for again, it is not built again.
    fn attribute_group(&mut self, doc: usize, reference: &QRef) -> Option<Rc<AttributeSet>> {
        let name = self.visible_name(doc, reference, "attribute group")?;
        if let Some(done) = self.attribute_group_memo.get(&name) {
            return done.clone();
        }
        let key = (Kind::AttributeGroup, name.clone());
        if self.progress.contains(&key) {
            if self.reported.insert(key) {
                self.report(doc, Code::XsdCircular, reference.span.clone(), &[&name.to_string(), "the attribute group contains itself"]);
            }
            return None;
        }
        let Some(&place) = self.attribute_group_places.get(&name) else {
            self.report(doc, Code::XsdUnresolved, reference.span.clone(), &["attribute group", &name.to_string()]);
            return None;
        };
        let Some(Item::AttributeGroup(def)) = self.item(place) else { return None };
        if self.depth >= MAX_DEPTH {
            self.report(doc, Code::XsdTooDeep, reference.span.clone(), &[&name.to_string()]);
            return None;
        }
        self.progress.insert(key.clone());
        self.depth += 1;
        let set = Rc::new(self.attribute_set(place.0, &def.uses));
        self.depth -= 1;
        self.progress.remove(&key);
        self.attribute_group_memo.insert(name, Some(set.clone()));
        Some(set)
    }
}
