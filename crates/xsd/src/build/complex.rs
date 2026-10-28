//! Complex type definitions: the content and the attributes of a type, by itself or derived from another.

use xmlcraft_datatypes::restrict;

use super::wildcards::union_wildcards;
use super::*;

/// What the definition of a complex type comes to.
struct Parts {
    base: TypeId,
    method: DerivationMethod,
    content: ContentType,
    attributes: Vec<SAttributeUse>,
    wildcard: Option<SWildcard>,
}

/// The base of a derivation: its number and what is inherited from it.
struct Base {
    id: TypeId,
    content: ContentType,
    attributes: Vec<SAttributeUse>,
    wildcard: Option<SWildcard>,
}

impl<'a> Builder<'a> {
    /// Build a complex type definition into `slot` (the number reserved for a top-level one), or at the end of the table.
    pub(super) fn complex_type(&mut self, doc: usize, def: &'a ComplexTypeDef, name: Option<QName>, slot: Option<TypeId>) -> Option<TypeId> {
        let shown = name.as_ref().map_or_else(|| "(anonymous type)".to_owned(), ToString::to_string);
        let origin = Self::origin(doc, &def.span);
        let id = slot.unwrap_or_else(|| self.reserve_type(origin.clone()));
        let document = self.document(doc)?;
        let (block_default, final_default) = (document.document.block_default, document.document.final_default);
        let derivations = |d: DerivationSet| DerivationSet { extension: d.extension, restriction: d.restriction, ..DerivationSet::default() };
        let parts = match &def.content {
            ComplexContentDef::Implicit { particle, attributes } => {
                let content = self.explicit_content(doc, particle.as_ref(), def.mixed.unwrap_or(false), id);
                let set = self.attribute_set(doc, attributes);
                Parts { base: self.any_type(), method: DerivationMethod::Restriction, content, attributes: set.uses, wildcard: set.wildcard }
            }
            ComplexContentDef::Complex { method, base, mixed, particle, attributes } => {
                self.complex_content(doc, def, &shown, id, *method, base.as_ref()?, mixed.or(def.mixed), particle.as_ref(), attributes)?
            }
            ComplexContentDef::Simple { method, base, inline_base, facets, attributes } => {
                self.simple_content(doc, def, &shown, *method, base.as_ref()?, inline_base.as_deref(), facets, attributes)?
            }
        };
        self.check_one_id(doc, &def.span, &shown, &parts.attributes);
        let complex = ComplexType {
            abstract_: def.abstract_.unwrap_or(false),
            block: def.block.unwrap_or(derivations(block_default)),
            content: parts.content,
            attributes: parts.attributes,
            attribute_wildcard: parts.wildcard,
        };
        let final_ = def.final_.unwrap_or(derivations(final_default));
        Some(self.put_type(
            Some(id),
            TypeEntry {
                name,
                kind: TypeKind::Complex(Box::new(complex)),
                base: Some(parts.base),
                derivation: Some(parts.method),
                final_,
                origin: Some(origin),
            },
        ))
    }

    /// The content of a type from the particle written in it; `scope` is the type.
    fn explicit_content(&mut self, doc: usize, particle: Option<&'a Particle>, mixed: bool, scope: TypeId) -> ContentType {
        match particle.and_then(|p| self.particle(doc, p, Some(scope))) {
            Some(p) if !Self::vacuous(&p) => {
                let content = Content { particle: p };
                if mixed { ContentType::Mixed(content) } else { ContentType::ElementOnly(content) }
            }
            // No elements: the type is empty, or has only text.
            _ if mixed => ContentType::Mixed(Content { particle: SParticle { min: 1, max: Some(1), term: STerm::Sequence(Vec::new()) } }),
            _ => ContentType::Empty,
        }
    }

    /// The base of a derivation, complete, and whether the derivation is allowed.
    fn derivation_base(&mut self, doc: usize, shown: &str, reference: &QRef, method: DerivationMethod) -> Option<TypeId> {
        let id = self.resolve_type_built(doc, reference)?;
        let entry = self.schema.types.get(id.0)?;
        let blocked = match method {
            DerivationMethod::Extension => entry.final_.extension,
            DerivationMethod::Restriction => entry.final_.restriction,
        };
        if blocked {
            let word = if method == DerivationMethod::Extension { "extension" } else { "restriction" };
            let base_name = entry.name.as_ref().map_or_else(|| "(anonymous type)".to_owned(), ToString::to_string);
            self.report(doc, Code::XsdDerivationNotAllowed, reference.span.clone(), &[shown, &format!("'{base_name}' is final for {word}")]);
        }
        Some(id)
    }

    fn attribute_name(&self, u: &SAttributeUse) -> Option<&QName> {
        self.schema.attributes.get(u.attribute.0).map(|d| &d.name)
    }

    /// The attributes of a derived type: a restriction keeps those of the base that it does not redeclare or prohibit and has only
    /// its own wildcard; an extension adds to those of the base and widens the wildcard.
    fn derive_attributes(
        &mut self,
        doc: usize,
        span: &Range<usize>,
        method: DerivationMethod,
        base: &Base,
        set: AttributeSet,
    ) -> (Vec<SAttributeUse>, Option<SWildcard>) {
        match method {
            DerivationMethod::Restriction => {
                let mut uses = set.uses.clone();
                for inherited in &base.attributes {
                    let Some(name) = self.attribute_name(inherited).cloned() else { continue };
                    let redeclared = uses.iter().any(|u| self.attribute_name(u) == Some(&name));
                    if !redeclared && !set.prohibited.contains(&name) {
                        uses.push(inherited.clone());
                    }
                }
                (uses, set.wildcard)
            }
            DerivationMethod::Extension => {
                let mut merged = AttributeSet { uses: Vec::new(), prohibited: Vec::new(), wildcard: None };
                for inherited in &base.attributes {
                    merged.uses.push(inherited.clone());
                }
                for own in set.uses {
                    self.push_use(doc, span, &mut merged, own);
                }
                let wildcard = match (set.wildcard, &base.wildcard) {
                    (Some(own), Some(inherited)) => union_wildcards(&own, inherited).or_else(|| {
                        self.report(
                            doc,
                            Code::XsdComponent,
                            span.clone(),
                            &["attribute wildcard", "the wildcard of the type and that of its base have no union that can be written"],
                        );
                        Some(own)
                    }),
                    (own, inherited) => own.or_else(|| inherited.clone()),
                };
                (merged.uses, wildcard)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn complex_content(
        &mut self,
        doc: usize,
        def: &'a ComplexTypeDef,
        shown: &str,
        id: TypeId,
        method: DerivationMethod,
        reference: &QRef,
        mixed: Option<bool>,
        particle: Option<&'a Particle>,
        attributes: &'a AttributeUses,
    ) -> Option<Parts> {
        let base_id = self.derivation_base(doc, shown, reference, method)?;
        let base = match self.schema.types.get(base_id.0).map(|t| &t.kind) {
            Some(TypeKind::Complex(c)) => {
                Base { id: base_id, content: c.content.clone(), attributes: c.attributes.clone(), wildcard: c.attribute_wildcard.clone() }
            }
            _ => {
                let base_name = self.qualify(doc, &reference.name).to_string();
                self.report(doc, Code::XsdWrongKind, reference.span.clone(), &[&base_name, "a simple type, where a complex type is needed"]);
                return None;
            }
        };
        let set = self.attribute_set(doc, attributes);
        let content = match method {
            DerivationMethod::Restriction => self.explicit_content(doc, particle, mixed.unwrap_or(false), id),
            DerivationMethod::Extension => self.extended_content(doc, &def.span, shown, id, &base.content, mixed, particle),
        };
        let (uses, wildcard) = self.derive_attributes(doc, &def.span, method, &base, set);
        Some(Parts { base: base.id, method, content, attributes: uses, wildcard })
    }

    /// The content of an extension: the content of the base, followed by the particle of the extension.
    #[allow(clippy::too_many_arguments)]
    fn extended_content(
        &mut self,
        doc: usize,
        span: &Range<usize>,
        shown: &str,
        id: TypeId,
        base: &ContentType,
        mixed: Option<bool>,
        particle: Option<&'a Particle>,
    ) -> ContentType {
        let own = self.explicit_content(doc, particle, mixed.unwrap_or(false), id);
        let (base_content, base_mixed) = match base {
            ContentType::Empty => return own,
            ContentType::Simple(t) => {
                if !matches!(own, ContentType::Empty) {
                    self.report(doc, Code::XsdComponent, span.clone(), &[shown, "a type with simple content cannot be extended with elements"]);
                }
                return ContentType::Simple(t.clone());
            }
            ContentType::ElementOnly(c) => (c, false),
            ContentType::Mixed(c) => (c, true),
        };
        // Nothing of its own: the type has the content of its base.
        let own_particle = match own {
            ContentType::ElementOnly(c) => c.particle,
            ContentType::Mixed(c) if !Self::vacuous(&c.particle) => c.particle,
            _ => return base.clone(),
        };
        if mixed.unwrap_or(false) != base_mixed {
            self.report(
                doc,
                Code::XsdComponent,
                span.clone(),
                &[shown, "an extension must be mixed if its base is, and not mixed if its base is not"],
            );
        }
        let joined =
            Content { particle: SParticle { min: 1, max: Some(1), term: STerm::Sequence(vec![base_content.particle.clone(), own_particle]) } };
        if base_mixed { ContentType::Mixed(joined) } else { ContentType::ElementOnly(joined) }
    }

    #[allow(clippy::too_many_arguments)]
    fn simple_content(
        &mut self,
        doc: usize,
        def: &'a ComplexTypeDef,
        shown: &str,
        method: DerivationMethod,
        reference: &QRef,
        inline_base: Option<&'a SimpleTypeDef>,
        facets: &'a [FacetDef],
        attributes: &'a AttributeUses,
    ) -> Option<Parts> {
        let base_id = self.derivation_base(doc, shown, reference, method)?;
        let base_name = self.qualify(doc, &reference.name).to_string();
        let (base, simple) = match self.schema.types.get(base_id.0).map(|t| &t.kind) {
            Some(TypeKind::Complex(c)) => match &c.content {
                ContentType::Simple(t) => (
                    Base { id: base_id, content: c.content.clone(), attributes: c.attributes.clone(), wildcard: c.attribute_wildcard.clone() },
                    Some(t.clone()),
                ),
                // A restriction can turn text in a mixed type that may be empty into a simple type.
                ContentType::Mixed(content) if method == DerivationMethod::Restriction && Self::emptiable(&content.particle) => {
                    (Base { id: base_id, content: c.content.clone(), attributes: c.attributes.clone(), wildcard: c.attribute_wildcard.clone() }, None)
                }
                _ => {
                    self.report(
                        doc,
                        Code::XsdWrongKind,
                        reference.span.clone(),
                        &[&base_name, "a type without simple content, where simple content is needed"],
                    );
                    return None;
                }
            },
            Some(TypeKind::Simple(t)) if method == DerivationMethod::Extension => {
                (Base { id: base_id, content: ContentType::Empty, attributes: Vec::new(), wildcard: None }, Some(t.clone()))
            }
            _ => {
                self.report(
                    doc,
                    Code::XsdWrongKind,
                    reference.span.clone(),
                    &[&base_name, "a simple type, where a complex type with simple content is needed"],
                );
                return None;
            }
        };
        let inline = inline_base.and_then(|def| self.simple_type(doc, def, None, None)).and_then(|id| self.simple_arc(id));
        let Some(mut text) = inline.or(simple) else {
            self.report(
                doc,
                Code::XsdComponent,
                def.span.clone(),
                &[shown, "a restriction of a type with mixed content needs a simple type inside it"],
            );
            return None;
        };
        if method == DerivationMethod::Restriction && !facets.is_empty() {
            let spec = self.facet_spec(doc, shown, facets);
            let namespaces = facets.first().map(|f| f.namespaces.clone()).unwrap_or_default();
            match restrict(&text, None, &spec, &Scope(&namespaces)) {
                Ok(restricted) => text = restricted,
                Err(problems) => {
                    for problem in problems {
                        self.report(doc, Code::XsdFacetProblem, def.span.clone(), &[shown, &problem]);
                    }
                }
            }
        }
        let set = self.attribute_set(doc, attributes);
        let (uses, wildcard) = self.derive_attributes(doc, &def.span, method, &base, set);
        Some(Parts { base: base.id, method, content: ContentType::Simple(text), attributes: uses, wildcard })
    }

    /// A type can have one attribute of type ID at most.
    fn check_one_id(&mut self, doc: usize, span: &Range<usize>, shown: &str, uses: &[SAttributeUse]) {
        let ids = uses.iter().filter(|u| self.schema.attributes.get(u.attribute.0).is_some_and(|d| d.type_.is_derived_from_builtin("ID"))).count();
        if ids > 1 {
            self.report(doc, Code::XsdComponent, span.clone(), &[shown, "it has more than one attribute of type ID"]);
        }
    }
}
