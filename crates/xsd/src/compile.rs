//! The content models of the complex types, compiled for validation: an automaton over the child elements for a sequence, a choice or
//! a wildcard, a set of slots for an `xs:all` group. Substitution groups are expanded, so a member can take the place of its head.
//! The compilation also checks the two constraints that are about a whole content model: Unique Particle Attribution (no element can
//! be taken by two particles) and Element Declarations Consistent (elements of one name in a model have one type).

use std::collections::{HashMap, HashSet};

use xmlcraft_grammar::{Automaton, Name, NamespaceConstraint, Occurs, Particle as GParticle, State};
use xmlcraft_syntax::{Code, Diagnostic};

use crate::loader::SourcedDiagnostic;
use crate::schema::{ContentType, ElementId, NamespaceSet, Particle, Schema, Term, TypeId, TypeKind, Wildcard};

/// What a particle of a compiled content model stands for: the token an automaton step gives back is an index into
/// [`ContentModel::symbols`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Symbol {
    Element(ElementId),
    Wildcard(Wildcard),
}

/// A content model, compiled.
#[derive(Debug)]
pub struct ContentModel {
    matcher: Matcher,
    symbols: Vec<Symbol>,
}

#[derive(Debug)]
enum Matcher {
    Automaton(Automaton),
    All(AllGroup),
}

/// Where a read of the children has got to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelState {
    Automaton(State),
    All(AllState),
}

/// What may come next in a content model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Next<'a> {
    pub symbol: &'a Symbol,
    /// The content cannot be finished without it.
    pub mandatory: bool,
}

impl ContentModel {
    pub fn symbols(&self) -> &[Symbol] {
        &self.symbols
    }

    pub fn start(&self) -> ModelState {
        match &self.matcher {
            Matcher::Automaton(a) => ModelState::Automaton(a.start()),
            Matcher::All(a) => ModelState::All(a.start()),
        }
    }

    /// Read the child element `name`: the state after it and what took it, or `None` when it cannot come here.
    pub fn step(&self, state: &ModelState, name: &Name) -> Option<(ModelState, &Symbol)> {
        match (&self.matcher, state) {
            (Matcher::Automaton(a), ModelState::Automaton(s)) => {
                let step = a.step(s, name)?;
                Some((ModelState::Automaton(step.state), self.symbols.get(step.token)?))
            }
            (Matcher::All(a), ModelState::All(s)) => {
                let (next, token) = a.step(s, name)?;
                Some((ModelState::All(next), self.symbols.get(token)?))
            }
            _ => None,
        }
    }

    /// Can the content end here?
    pub fn is_accepting(&self, state: &ModelState) -> bool {
        match (&self.matcher, state) {
            (Matcher::Automaton(a), ModelState::Automaton(s)) => a.is_accepting(s),
            (Matcher::All(a), ModelState::All(s)) => a.is_accepting(s),
            _ => false,
        }
    }

    /// The shortest run of elements that finishes what was read: empty when the content is complete, `None` when nothing finishes it.
    /// In an `xs:all` group it is the first option of each slot still to be filled that is required.
    pub fn shortest_completion(&self, state: &ModelState) -> Option<Vec<&Symbol>> {
        match (&self.matcher, state) {
            (Matcher::Automaton(a), ModelState::Automaton(s)) => {
                Some(a.shortest_completion(s)?.into_iter().filter_map(|symbol| self.symbols.get(symbol.token)).collect())
            }
            (Matcher::All(a), ModelState::All(s)) => Some(a.required_tokens(s).into_iter().filter_map(|token| self.symbols.get(token)).collect()),
            _ => None,
        }
    }

    /// What may come next.
    pub fn expected(&self, state: &ModelState) -> Vec<Next<'_>> {
        match (&self.matcher, state) {
            (Matcher::Automaton(a), ModelState::Automaton(s)) => {
                a.expected(s).into_iter().filter_map(|e| Some(Next { symbol: self.symbols.get(e.token)?, mandatory: e.mandatory })).collect()
            }
            (Matcher::All(a), ModelState::All(s)) => {
                a.expected(s).into_iter().filter_map(|(token, mandatory)| Some(Next { symbol: self.symbols.get(token)?, mandatory })).collect()
            }
            _ => Vec::new(),
        }
    }
}

/// An `xs:all` group: each slot is taken once, in any order; the elements that can stand for a slot (the element and its substitutes)
/// are its options.
#[derive(Debug)]
struct AllGroup {
    slots: Vec<Slot>,
    /// The group may be left out altogether.
    optional: bool,
}

#[derive(Debug)]
struct Slot {
    required: bool,
    options: Vec<(Name, usize)>,
}

/// The slots of an `xs:all` group that have been taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllState {
    taken: Vec<bool>,
}

impl AllGroup {
    fn start(&self) -> AllState {
        AllState { taken: vec![false; self.slots.len()] }
    }

    fn step(&self, state: &AllState, name: &Name) -> Option<(AllState, usize)> {
        for (index, slot) in self.slots.iter().enumerate() {
            if state.taken.get(index).copied().unwrap_or(true) {
                continue;
            }
            if let Some((_, token)) = slot.options.iter().find(|(n, _)| n == name) {
                let mut taken = state.taken.clone();
                *taken.get_mut(index)? = true;
                return Some((AllState { taken }, *token));
            }
        }
        None
    }

    fn is_accepting(&self, state: &AllState) -> bool {
        let none_taken = !state.taken.iter().any(|t| *t);
        if none_taken && self.optional {
            return true;
        }
        self.slots.iter().zip(&state.taken).all(|(slot, taken)| *taken || !slot.required)
    }

    /// The first option of each required slot not taken yet (none when the group may be left out and nothing was taken).
    fn required_tokens(&self, state: &AllState) -> Vec<usize> {
        if !state.taken.iter().any(|t| *t) && self.optional {
            return Vec::new();
        }
        self.slots
            .iter()
            .zip(&state.taken)
            .filter(|(slot, taken)| slot.required && !**taken)
            .filter_map(|(slot, _)| slot.options.first().map(|(_, token)| *token))
            .collect()
    }

    fn expected(&self, state: &AllState) -> Vec<(usize, bool)> {
        let started = state.taken.iter().any(|t| *t);
        let mut out = Vec::new();
        for (slot, taken) in self.slots.iter().zip(&state.taken) {
            if *taken {
                continue;
            }
            for (_, token) in &slot.options {
                out.push((*token, slot.required && (started || !self.optional)));
            }
        }
        out
    }
}

/// The content models of all the complex types of `schema`, compiled; what is wrong with them is reported.
pub(crate) fn compile(schema: &Schema) -> (Vec<Option<ContentModel>>, Vec<SourcedDiagnostic>) {
    let mut compiler = Compiler { schema, members: members_of(schema), diagnostics: Vec::new() };
    let models = (0..schema.types.len()).map(|index| compiler.model_of(TypeId(index))).collect();
    (models, compiler.diagnostics)
}

/// The elements that are directly in the substitution group of each element.
fn members_of(schema: &Schema) -> HashMap<ElementId, Vec<ElementId>> {
    let mut members: HashMap<ElementId, Vec<ElementId>> = HashMap::new();
    for (index, element) in schema.elements.iter().enumerate() {
        if let Some(head) = element.substitution_group {
            members.entry(head).or_default().push(ElementId(index));
        }
    }
    members
}

struct Compiler<'s> {
    schema: &'s Schema,
    members: HashMap<ElementId, Vec<ElementId>>,
    diagnostics: Vec<SourcedDiagnostic>,
}

/// The state of building one model.
struct Building {
    symbols: Vec<Symbol>,
    /// An `xs:all` group that is not the whole model was met.
    misplaced_all: bool,
}

impl Building {
    fn symbol(&mut self, symbol: Symbol) -> usize {
        if let Some(index) = self.symbols.iter().position(|s| *s == symbol) {
            return index;
        }
        self.symbols.push(symbol);
        self.symbols.len() - 1
    }
}

impl Compiler<'_> {
    fn report(&mut self, type_id: TypeId, code: Code, args: &[&str]) {
        let Some(origin) = self.schema.types.get(type_id.0).and_then(|t| t.origin.as_ref()) else { return };
        self.diagnostics.push(SourcedDiagnostic {
            source: origin.document,
            diagnostic: Diagnostic::new(code, origin.range.clone(), args.iter().map(|a| (*a).to_owned()).collect()),
        });
    }

    fn model_of(&mut self, type_id: TypeId) -> Option<ContentModel> {
        let entry = self.schema.types.get(type_id.0)?;
        let TypeKind::Complex(complex) = &entry.kind else { return None };
        let content = match &complex.content {
            ContentType::ElementOnly(c) | ContentType::Mixed(c) => c,
            ContentType::Empty | ContentType::Simple(_) => return None,
        };
        let shown = entry.name.as_ref().map_or_else(|| "(anonymous type)".to_owned(), ToString::to_string);
        let mut building = Building { symbols: Vec::new(), misplaced_all: false };
        let matcher = match &content.particle.term {
            Term::All(children) => Matcher::All(self.all_group(&mut building, children, content.particle.min == 0)),
            _ => {
                let particle = self.particle(&mut building, &content.particle);
                match Automaton::compile(&particle) {
                    Ok(automaton) => Matcher::Automaton(automaton),
                    Err(error) => {
                        self.report(type_id, Code::XsdLimit, &[&format!("the content model of '{shown}': {error}")]);
                        return None;
                    }
                }
            }
        };
        if building.misplaced_all {
            self.report(type_id, Code::XsdComponent, &[&shown, "an xs:all group can only be the whole content model in XML Schema 1.0"]);
        }
        let model = ContentModel { matcher, symbols: building.symbols };
        self.check_model(type_id, &shown, &model);
        Some(model)
    }

    /// Unique Particle Attribution and Element Declarations Consistent.
    fn check_model(&mut self, type_id: TypeId, shown: &str, model: &ContentModel) {
        if let Matcher::Automaton(automaton) = &model.matcher
            && let Err(ambiguity) = automaton.check_deterministic()
        {
            let what = ambiguity.term.to_string();
            self.report(type_id, Code::XsdAmbiguous, &[shown, &format!("'{what}' can be taken by two particles")]);
        }
        let mut seen: HashMap<Name, TypeId> = HashMap::new();
        let mut reported: HashSet<Name> = HashSet::new();
        for symbol in &model.symbols {
            let Symbol::Element(id) = symbol else { continue };
            let Some(element) = self.schema.elements.get(id.0) else { continue };
            let name = Name { namespace: element.name.namespace.clone(), local: element.name.local.clone() };
            match seen.get(&name) {
                Some(&other) if other != element.type_id => {
                    if reported.insert(name.clone()) {
                        self.report(
                            type_id,
                            Code::XsdComponent,
                            &[shown, &format!("the elements called '{name}' in its content model do not all have the same type")],
                        );
                    }
                }
                Some(_) => {}
                None => {
                    seen.insert(name, element.type_id);
                }
            }
        }
    }

    fn all_group(&mut self, building: &mut Building, children: &[Particle], optional: bool) -> AllGroup {
        let mut slots = Vec::new();
        for child in children {
            if child.max == Some(0) {
                continue;
            }
            let mut options = Vec::new();
            match &child.term {
                Term::Element(id) => {
                    for element in self.substitutes(*id) {
                        if let Some(decl) = self.schema.elements.get(element.0) {
                            let name = Name { namespace: decl.name.namespace.clone(), local: decl.name.local.clone() };
                            options.push((name, building.symbol(Symbol::Element(element))));
                        }
                    }
                }
                // Only elements can be in an all group; the reader reported anything else.
                _ => continue,
            }
            slots.push(Slot { required: child.min > 0, options });
        }
        AllGroup { slots, optional }
    }

    fn particle(&mut self, building: &mut Building, p: &Particle) -> GParticle {
        let occurs = Occurs::new(p.min, p.max);
        let particle = match &p.term {
            Term::Element(id) => {
                let mut options: Vec<GParticle> = self
                    .substitutes(*id)
                    .into_iter()
                    .filter_map(|element| {
                        let decl = self.schema.elements.get(element.0)?;
                        let name = Name { namespace: decl.name.namespace.clone(), local: decl.name.local.clone() };
                        Some(GParticle::element(name, building.symbol(Symbol::Element(element))))
                    })
                    .collect();
                if options.len() == 1 { options.remove(0) } else { GParticle::choice(options) }
            }
            Term::Wildcard(w) => {
                let constraint = match &w.constraint {
                    NamespaceSet::Any => NamespaceConstraint::Any,
                    NamespaceSet::Not(target) => NamespaceConstraint::Other(target.clone()),
                    NamespaceSet::List(list) => NamespaceConstraint::Set(list.clone()),
                };
                GParticle::wildcard(constraint, building.symbol(Symbol::Wildcard(w.clone())))
            }
            Term::Sequence(children) => GParticle::sequence(children.iter().map(|c| self.particle(building, c)).collect()),
            Term::Choice(children) => GParticle::choice(children.iter().map(|c| self.particle(building, c)).collect()),
            Term::All(children) => {
                building.misplaced_all = true;
                GParticle::sequence(children.iter().map(|c| self.particle(building, c)).collect())
            }
        };
        particle.occurring(occurs)
    }

    /// The elements that can take the place of the declared element `id`: the element itself, unless it is abstract, and the members
    /// of its substitution group whose types are derived from its type in ways it does not block.
    fn substitutes(&self, id: ElementId) -> Vec<ElementId> {
        let Some(head) = self.schema.elements.get(id.0) else { return Vec::new() };
        let mut out = Vec::new();
        if !head.abstract_ {
            out.push(id);
        }
        if head.block.substitution {
            return out;
        }
        let mut seen: HashSet<ElementId> = HashSet::from([id]);
        let mut queue = vec![id];
        while let Some(at) = queue.pop() {
            for &member in self.members.get(&at).map(Vec::as_slice).unwrap_or_default() {
                if !seen.insert(member) {
                    continue;
                }
                queue.push(member);
                let Some(decl) = self.schema.elements.get(member.0) else { continue };
                if !decl.abstract_ && self.schema.derivation_allowed(decl.type_id, head.type_id, head.block) {
                    out.push(member);
                }
            }
        }
        out.sort_by_key(|e| e.0);
        out
    }
}
