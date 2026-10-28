//! A DTD as the grammar it declares: the first declaration of each element and attribute is the one that counts, content models are
//! compiled to automata, and the validity constraints that the declarations themselves must meet are checked (XML 1.0 §3.2, §3.3, §4.7).

use std::collections::HashMap;

use xmlcraft_grammar as g;
use xmlcraft_grammar::{Automaton, Grammar, Name, Occurs, Particle, SchemaKind};
use xmlcraft_syntax::{Code, Diagnostic};

use crate::attribute::{ValueReader, check_value};
use crate::model::{AttDef, AttType, ContentSpec, Default, Dtd, EntityValue, Occurs as DtdOccurs, ParticleKind, Place};
use crate::parse::DtdDiagnostic;

/// A DTD compiled.
#[derive(Debug)]
pub struct Compiled {
    pub grammar: Grammar,
    /// What is wrong with the declarations as a grammar: validity constraints they break, and content models that are not deterministic
    /// (a warning).
    pub diagnostics: Vec<DtdDiagnostic>,
}

fn origin(place: &Place) -> g::Origin {
    g::Origin { source: place.source, range: place.range.clone() }
}

fn occurs(o: DtdOccurs) -> Occurs {
    match o {
        DtdOccurs::Once => Occurs::ONCE,
        DtdOccurs::Optional => Occurs::OPTIONAL,
        DtdOccurs::ZeroOrMore => Occurs::ZERO_OR_MORE,
        DtdOccurs::OneOrMore => Occurs::ONE_OR_MORE,
    }
}

/// The particle of a content model, with the position of the element it names (or [`g::NO_DECLARATION`]) as its token.
fn particle(p: &crate::model::Particle, indices: &HashMap<&str, usize>) -> Particle {
    let converted = match &p.kind {
        ParticleKind::Name(name) => Particle::element(Name::local(name), indices.get(name.as_str()).copied().unwrap_or(g::NO_DECLARATION)),
        ParticleKind::Choice(items) => Particle::choice(items.iter().map(|i| particle(i, indices)).collect()),
        ParticleKind::Sequence(items) => Particle::sequence(items.iter().map(|i| particle(i, indices)).collect()),
    };
    converted.occurring(occurs(p.occurs))
}

fn attribute_type(ty: &AttType) -> g::AttributeType {
    match ty {
        AttType::Cdata => g::AttributeType::Cdata,
        AttType::Id => g::AttributeType::Id,
        AttType::Idref => g::AttributeType::Idref,
        AttType::Idrefs => g::AttributeType::Idrefs,
        AttType::Entity => g::AttributeType::Entity,
        AttType::Entities => g::AttributeType::Entities,
        AttType::Nmtoken => g::AttributeType::Nmtoken,
        AttType::Nmtokens => g::AttributeType::Nmtokens,
        AttType::Notation(list) => g::AttributeType::Notation(list.clone()),
        AttType::Enumeration(list) => g::AttributeType::Enumeration(list.clone()),
    }
}

fn external(id: &crate::model::ExternalId) -> (Option<String>, Option<String>) {
    (id.public.clone(), id.system.clone())
}

struct Compiler<'a> {
    dtd: &'a Dtd,
    grammar: Grammar,
    diagnostics: Vec<DtdDiagnostic>,
}

impl Compiler<'_> {
    fn report(&mut self, place: &Place, code: Code, args: &[&str]) {
        self.diagnostics.push(DtdDiagnostic {
            source: place.source,
            diagnostic: Diagnostic::new(code, place.range.clone(), args.iter().map(|a| (*a).to_owned()).collect()),
        });
    }

    fn entities_and_notations(&mut self) {
        for e in &self.dtd.general_entities {
            let kind = match (&e.value, &e.notation) {
                (EntityValue::Internal(text), _) => g::EntityKind::Internal(text.clone()),
                (EntityValue::External(id), None) => {
                    let (public, system) = external(id);
                    g::EntityKind::External { public, system }
                }
                (EntityValue::External(id), Some(notation)) => {
                    let (public, system) = external(id);
                    g::EntityKind::Unparsed { public, system, notation: notation.clone() }
                }
            };
            // A name declared more than once keeps its first declaration; nothing is said about the others (XML 1.0 §4.2).
            self.grammar.add_entity(g::EntityDecl { name: e.name.clone(), kind, origin: origin(&e.place) });
        }
        for n in &self.dtd.notations {
            let (public, system) = external(&n.id);
            if !self.grammar.add_notation(g::NotationDecl { name: n.name.clone(), public, system, origin: origin(&n.place) }) {
                self.report(&n.place, Code::VcDuplicateNotation, &[&n.name]);
            }
        }
        if !self.dtd.incomplete {
            let undeclared: Vec<(String, Place)> = self
                .grammar
                .entities()
                .iter()
                .filter_map(|e| match &e.kind {
                    g::EntityKind::Unparsed { notation, .. } if self.grammar.notation(notation).is_none() => {
                        Some((notation.clone(), Place { source: e.origin.source, range: e.origin.range.clone() }))
                    }
                    _ => None,
                })
                .collect();
            for (notation, place) in undeclared {
                self.report(&place, Code::VcNotationNotDeclared, &[&notation]);
            }
        }
    }

    fn elements(&mut self) {
        // The first declaration of a name is the one that counts, and the particles that name it refer to it by its position.
        let mut indices: HashMap<&str, usize> = HashMap::new();
        for e in &self.dtd.elements {
            let next = indices.len();
            indices.entry(e.name.as_str()).or_insert(next);
        }
        for e in &self.dtd.elements {
            let content = match &e.spec {
                ContentSpec::Empty => g::Content::Empty,
                ContentSpec::Any => g::Content::Any,
                ContentSpec::Mixed(names) => {
                    let mut unique: Vec<Name> = Vec::new();
                    for name in names {
                        let n = Name::local(name);
                        if unique.contains(&n) {
                            self.report(&e.place, Code::VcDuplicateMixedName, &[name]);
                        } else {
                            unique.push(n);
                        }
                    }
                    g::Content::Mixed(unique)
                }
                ContentSpec::Children(p) => {
                    let converted = particle(p, &indices);
                    match Automaton::compile(&converted) {
                        Ok(automaton) => {
                            if let Err(ambiguity) = automaton.check_deterministic() {
                                self.report(&e.place, Code::DtdNonDeterministic, &[&e.name, &ambiguity.term.to_string()]);
                            }
                            g::Content::Elements(Box::new(g::ElementContent { particle: converted, automaton }))
                        }
                        Err(error) => {
                            self.report(&e.place, Code::DtdLimit, &[&error.to_string()]);
                            // The model cannot be used: anything that is declared may come.
                            g::Content::Any
                        }
                    }
                }
            };
            let decl = g::ElementDecl { name: Name::local(&e.name), content, attributes: Vec::new(), origin: origin(&e.place) };
            if !self.grammar.add_element(decl) {
                self.report(&e.place, Code::VcDuplicateElement, &[&e.name]);
            }
        }
    }

    /// The attributes of each element: the first definition of a name counts. Then the validity constraints on them.
    fn attributes(&mut self) {
        let mut by_element: HashMap<&str, Vec<&AttDef>> = HashMap::new();
        for list in &self.dtd.attlists {
            let defs = by_element.entry(list.element.as_str()).or_default();
            for def in &list.defs {
                if !defs.iter().any(|d| d.name == def.name) {
                    defs.push(def);
                }
            }
        }
        // Declarations of attributes for an element that is not declared are allowed, and keep no place in the grammar.
        let names: Vec<(String, Place)> = self.dtd.elements.iter().map(|e| (e.name.clone(), e.place.clone())).collect();
        let mut done: Vec<&str> = Vec::new();
        for (name, _) in &names {
            if done.contains(&name.as_str()) {
                continue;
            }
            done.push(name);
            let Some(defs) = by_element.get(name.as_str()) else { continue };
            let element_name = Name::local(name);
            let empty = self.grammar.element(&element_name).is_some_and(|e| matches!(e.content, g::Content::Empty));
            let mut decls: Vec<g::AttributeDecl> = Vec::new();
            let (mut ids, mut notations) = (0usize, 0usize);
            for def in defs {
                let ty = attribute_type(&def.ty);
                if matches!(ty, g::AttributeType::Id) {
                    ids += 1;
                    if ids > 1 {
                        self.report(&def.place, Code::VcOneIdPerElement, &[name]);
                    }
                    if !matches!(def.default, Default::Required | Default::Implied) {
                        self.report(&def.place, Code::VcIdAttributeDefault, &[name, &def.name]);
                    }
                }
                if let g::AttributeType::Notation(list) = &ty {
                    notations += 1;
                    if notations > 1 {
                        self.report(&def.place, Code::VcOneNotationPerElement, &[name]);
                    }
                    if empty {
                        self.report(&def.place, Code::VcNotationOnEmpty, &[name, &def.name]);
                    }
                    if !self.dtd.incomplete {
                        let undeclared: Vec<&String> = list.iter().filter(|n| self.grammar.notation(n).is_none()).collect();
                        for notation in undeclared {
                            self.report(&def.place, Code::VcNotationNotDeclared, &[notation]);
                        }
                    }
                }
                if let g::AttributeType::Notation(list) | g::AttributeType::Enumeration(list) = &ty {
                    for (i, token) in list.iter().enumerate() {
                        if list.iter().take(i).any(|earlier| earlier == token) {
                            self.report(&def.place, Code::VcDuplicateToken, &[&def.name, token]);
                        }
                    }
                }
                let default = match &def.default {
                    Default::Required => g::AttributeDefault::Required,
                    Default::Implied => g::AttributeDefault::Implied,
                    Default::Fixed(v) => g::AttributeDefault::Fixed(v.clone()),
                    Default::Value(v) => g::AttributeDefault::Default(v.clone()),
                };
                decls.push(g::AttributeDecl { name: Name::local(&def.name), ty, default, origin: origin(&def.place) });
            }
            if let Some(element) = self.grammar.element_mut(&element_name) {
                element.attributes = decls;
            }
            self.default_values(name);
        }
    }

    /// A default value must be legal for the type of its attribute (it is read first: references replaced, white space turned into
    /// spaces and for the types other than CDATA trimmed and collapsed).
    fn default_values(&mut self, element: &str) {
        let Some(decl) = self.grammar.element(&Name::local(element)) else { return };
        let mut problems: Vec<(Place, String, String)> = Vec::new();
        let reader = ValueReader::new(&self.grammar);
        for attribute in &decl.attributes {
            let (g::AttributeDefault::Fixed(literal) | g::AttributeDefault::Default(literal)) = &attribute.default else { continue };
            let outcome = reader.read(literal, &attribute.ty).map_err(|p| p.to_string()).and_then(|value| check_value(&attribute.ty, &value));
            if let Err(why) = outcome {
                let place = Place { source: attribute.origin.source, range: attribute.origin.range.clone() };
                problems.push((place, attribute.name.local.clone(), why));
            }
        }
        for (place, name, why) in problems {
            self.report(&place, Code::VcAttributeDefault, &[&name, &why]);
        }
    }
}

/// Compile the declarations of a DTD into a grammar, and say what is wrong with them as one. `root` is the name the DOCTYPE gives the
/// root element.
pub fn compile(dtd: &Dtd, root: Option<&str>) -> Compiled {
    let mut grammar = Grammar::new(SchemaKind::Dtd);
    grammar.root = root.map(Name::local);
    grammar.incomplete = dtd.incomplete;
    let mut compiler = Compiler { dtd, grammar, diagnostics: Vec::new() };
    compiler.entities_and_notations();
    compiler.elements();
    compiler.attributes();
    compiler.diagnostics.sort_by_key(|d| (d.source, d.diagnostic.span.start, d.diagnostic.span.end));
    Compiled { grammar: compiler.grammar, diagnostics: compiler.diagnostics }
}
