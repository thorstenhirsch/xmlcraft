//! Simple type definitions: restriction, list and union.

use std::sync::Arc;

use xmlcraft_datatypes::{FacetSpec, Setting, SimpleType, list_of, restrict, union_of};
use xmlcraft_syntax::Code;

use super::{Builder, Scope, expanded};
use crate::model::*;
use crate::schema::{TypeEntry, TypeId, TypeKind};

impl<'a> Builder<'a> {
    /// The simple type an inline definition or a reference gives: its number and the type.
    fn simple_from(&mut self, doc: usize, reference: Option<&QRef>, inline: Option<&'a SimpleTypeDef>) -> Option<(TypeId, Arc<SimpleType>)> {
        match (reference, inline) {
            (Some(reference), None) => self.resolve_simple(doc, reference),
            (None, Some(inline)) => {
                let id = self.simple_type(doc, inline, None, None)?;
                match self.schema.types.get(id.0).map(|t| &t.kind) {
                    Some(TypeKind::Simple(arc)) => Some((id, arc.clone())),
                    _ => None,
                }
            }
            // The reader reported a definition with both or neither.
            _ => None,
        }
    }

    fn type_final(&self, id: TypeId) -> DerivationSet {
        self.schema.types.get(id.0).map(|t| t.final_).unwrap_or_default()
    }

    fn type_name_of(&self, id: TypeId) -> String {
        self.schema.types.get(id.0).and_then(|t| t.name.as_ref()).map(ToString::to_string).unwrap_or_else(|| "(anonymous type)".to_owned())
    }

    /// Build a simple type definition into `slot` (the number reserved for a top-level one), or at the end of the table.
    pub(super) fn simple_type(&mut self, doc: usize, def: &'a SimpleTypeDef, name: Option<QName>, slot: Option<TypeId>) -> Option<TypeId> {
        let shown = name.as_ref().map_or_else(|| "(anonymous type)".to_owned(), ToString::to_string);
        let origin = Some(Self::origin(doc, &def.span));
        let default_final = self.document(doc).map(|d| d.document.final_default).unwrap_or_default();
        let final_ = def.final_.unwrap_or(DerivationSet { extension: false, substitution: false, ..default_final });
        let ename = name.as_ref().map(expanded);
        match &def.kind {
            SimpleTypeKind::Restriction { base, inline_base, facets } => {
                let (base_id, base_arc) = self.simple_from(doc, base.as_ref(), inline_base.as_deref())?;
                if self.type_final(base_id).restriction {
                    let base_name = self.type_name_of(base_id);
                    self.report(doc, Code::XsdDerivationNotAllowed, def.span.clone(), &[&shown, &format!("'{base_name}' is final for restriction")]);
                }
                let spec = self.facet_spec(doc, &shown, facets);
                let namespaces = facets.first().map(|f| f.namespaces.clone()).unwrap_or_default();
                let (arc, problems) = match restrict(&base_arc, ename, &spec, &Scope(&namespaces)) {
                    Ok(arc) => (arc, Vec::new()),
                    // The type stands for its base, so that what uses it is not reported for it as well.
                    Err(problems) => (base_arc, problems),
                };
                for problem in problems {
                    self.report(doc, Code::XsdFacetProblem, def.span.clone(), &[&shown, &problem]);
                }
                Some(self.put_type(
                    slot,
                    TypeEntry {
                        name,
                        kind: TypeKind::Simple(arc),
                        base: Some(base_id),
                        derivation: Some(DerivationMethod::Restriction),
                        final_,
                        origin,
                    },
                ))
            }
            SimpleTypeKind::List { item_type, inline_item } => {
                let (item_id, item_arc) = self.simple_from(doc, item_type.as_ref(), inline_item.as_deref())?;
                if self.type_final(item_id).list {
                    self.report(doc, Code::XsdDerivationNotAllowed, def.span.clone(), &[&shown, "its item type is final for list"]);
                }
                match list_of(&item_arc, ename) {
                    Ok(arc) => {
                        let base = self.any_simple_type();
                        Some(self.put_type(slot, TypeEntry { name, kind: TypeKind::Simple(arc), base, derivation: None, final_, origin }))
                    }
                    Err(problems) => {
                        for problem in problems {
                            self.report(doc, Code::XsdFacetProblem, def.span.clone(), &[&shown, &problem]);
                        }
                        None
                    }
                }
            }
            SimpleTypeKind::Union { member_types, inline_members } => {
                let mut members = Vec::new();
                let mut missing = false;
                for reference in member_types {
                    let Some((id, arc)) = self.resolve_simple(doc, reference) else {
                        missing = true;
                        continue;
                    };
                    if self.type_final(id).union {
                        self.report(
                            doc,
                            Code::XsdDerivationNotAllowed,
                            reference.span.clone(),
                            &[&shown, &format!("'{}' is final for union", reference.name)],
                        );
                    }
                    members.push(arc);
                }
                for inline in inline_members {
                    if let Some(id) = self.simple_type(doc, inline, None, None)
                        && let Some(TypeKind::Simple(arc)) = self.schema.types.get(id.0).map(|t| &t.kind)
                    {
                        members.push(arc.clone());
                    }
                }
                // A member that could not be found was reported; a union of nothing else is not another mistake.
                if members.is_empty() && missing {
                    return None;
                }
                match union_of(&members, ename) {
                    Ok(arc) => {
                        let base = self.any_simple_type();
                        Some(self.put_type(slot, TypeEntry { name, kind: TypeKind::Simple(arc), base, derivation: None, final_, origin }))
                    }
                    Err(problems) => {
                        for problem in problems {
                            self.report(doc, Code::XsdFacetProblem, def.span.clone(), &[&shown, &problem]);
                        }
                        None
                    }
                }
            }
            SimpleTypeKind::Missing => None,
        }
    }

    /// The facets of a restriction as the datatypes crate takes them; a facet given twice is reported.
    pub(super) fn facet_spec(&mut self, doc: usize, shown: &str, facets: &[FacetDef]) -> FacetSpec {
        let mut spec = FacetSpec::default();
        for facet in facets {
            let setting = Setting { value: facet.value.clone(), fixed: facet.fixed };
            let slot = match facet.kind {
                FacetKind::Length => &mut spec.length,
                FacetKind::MinLength => &mut spec.min_length,
                FacetKind::MaxLength => &mut spec.max_length,
                FacetKind::TotalDigits => &mut spec.total_digits,
                FacetKind::FractionDigits => &mut spec.fraction_digits,
                FacetKind::WhiteSpace => &mut spec.white_space,
                FacetKind::MaxInclusive => &mut spec.max_inclusive,
                FacetKind::MaxExclusive => &mut spec.max_exclusive,
                FacetKind::MinExclusive => &mut spec.min_exclusive,
                FacetKind::MinInclusive => &mut spec.min_inclusive,
                FacetKind::Pattern => {
                    spec.patterns.push(facet.value.clone());
                    continue;
                }
                FacetKind::Enumeration => {
                    spec.enumeration.push(facet.value.clone());
                    continue;
                }
            };
            if slot.is_some() {
                self.report(doc, Code::XsdFacetProblem, facet.span.clone(), &[shown, &format!("{} is given more than once", facet.kind.local())]);
            } else {
                *slot = Some(setting);
            }
        }
        spec
    }
}
