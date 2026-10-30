//! A document generated from an assembled XML Schema.
//!
//! The content of an element is made from the particles of its type as they were written: a sequence is each of its parts in order, a
//! choice is one of its branches, and each particle occurs as often as the options say, kept between its least and its greatest number.
//! What ends the nesting of a recursive schema is the *height* of each element, the least depth of a valid tree below it: at the depth
//! the options allow only what is required is made, and of a choice the branch that is the shallowest.

use xmlcraft_xsd::model::{ProcessContents, QName};
use xmlcraft_xsd::schema::{ContentType, ElementId, NamespaceSet, Particle, Schema, Term, TypeId, TypeKind, Wildcard};

use crate::tree::{AttrValue, Node, XSI_NAMESPACE};
use crate::value::Values;
use crate::{ChoicePolicy, Options, Sample, SampleError};

/// The height of an element that no valid tree can be made for.
const INFINITE: u32 = u32::MAX;
/// How deep a document may get before it is refused, however the options are set (the required content of a schema can be deep).
const HARD_DEPTH: usize = 300;
/// The most elements of a document, and the most times one particle may be asked to occur.
const MAX_NODES: usize = 200_000;
const MAX_OCCURRENCES: u32 = 10_000;

/// The names of the global elements a document may start with (the ones that are not abstract), in the order they were declared.
pub fn roots(schema: &Schema) -> Vec<QName> {
    (0..schema.elements.len())
        .map(ElementId)
        .filter(|&id| schema.is_global(id))
        .filter_map(|id| schema.element(id))
        .filter(|e| !e.abstract_)
        .map(|e| e.name.clone())
        .collect()
}

pub fn sample(schema: &Schema, root: Option<&QName>, options: &Options) -> Result<Sample, SampleError> {
    let mut generator = Generator::new(schema, options);
    let id = match root {
        Some(name) => schema.element_by_name(name).ok_or_else(|| SampleError::UnknownRoot(name.to_string()))?,
        None => {
            let candidates =
                (0..schema.elements.len()).map(ElementId).filter(|&id| schema.is_global(id) && schema.element(id).is_some_and(|e| !e.abstract_));
            candidates.into_iter().find(|id| generator.height(*id) != INFINITE).ok_or(SampleError::NoRoot)?
        }
    };
    let id = generator.effective(id).ok_or_else(|| SampleError::Unsatisfiable(schema.element(id).map(|e| e.name.to_string()).unwrap_or_default()))?;
    let node = generator.element(id, 0)?;
    if generator.values.dangling_references() {
        generator.warnings.push("The document has a reference to an ID, and no ID.".to_owned());
    }
    Ok(Sample { text: crate::tree::serialize(&node, options.schema_location.as_deref()), warnings: generator.warnings })
}

struct Generator<'a> {
    schema: &'a Schema,
    options: &'a Options,
    values: Values,
    /// The height of each element (by number).
    heights: Vec<u32>,
    /// The concrete (not abstract) elements of each element's substitution group, itself included when it is concrete.
    members: Vec<Vec<ElementId>>,
    rotation: usize,
    nodes: usize,
    /// The elements being made, outermost first (a wildcard is not filled with one of these).
    path: Vec<ElementId>,
    warnings: Vec<String>,
}

impl<'a> Generator<'a> {
    fn new(schema: &'a Schema, options: &'a Options) -> Self {
        let n = schema.elements.len();
        let mut members: Vec<Vec<ElementId>> = vec![Vec::new(); n];
        for (i, element) in schema.elements.iter().enumerate() {
            if element.abstract_ {
                continue;
            }
            // This element stands for itself and for each head of the groups it is in.
            let mut head = Some(ElementId(i));
            let mut steps = 0usize;
            while let Some(h) = head {
                if let Some(list) = members.get_mut(h.0) {
                    list.push(ElementId(i));
                }
                head = schema.element(h).and_then(|e| e.substitution_group);
                steps += 1;
                if steps > n {
                    break; // a cycle in the groups was reported when the schema was assembled
                }
            }
        }
        let mut generator = Self {
            schema,
            options,
            values: Values::new(options.fill),
            heights: vec![INFINITE; n],
            members,
            rotation: 0,
            nodes: 0,
            path: Vec::new(),
            warnings: Vec::new(),
        };
        generator.compute_heights();
        generator
    }

    /// Heights are found by starting with none known and applying the rule until nothing changes: a tree below an element needs trees
    /// below the elements it requires.
    fn compute_heights(&mut self) {
        for _ in 0..=self.heights.len() {
            let mut changed = false;
            for i in 0..self.heights.len() {
                let h = self.height_of_declaration(ElementId(i));
                if self.heights.get(i).is_some_and(|&old| h < old) {
                    self.heights[i] = h;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    fn height(&self, id: ElementId) -> u32 {
        let own = self.heights.get(id.0).copied().unwrap_or(INFINITE);
        match self.schema.element(id) {
            // An abstract element is made as one of the elements that can stand for it.
            Some(e) if e.abstract_ => {
                self.members.get(id.0).into_iter().flatten().map(|m| self.heights.get(m.0).copied().unwrap_or(INFINITE)).min().unwrap_or(INFINITE)
            }
            _ => own,
        }
    }

    fn height_of_declaration(&self, id: ElementId) -> u32 {
        let Some(decl) = self.schema.element(id) else { return INFINITE };
        match self.concrete_type(decl.type_id) {
            Some(t) => self.type_cost(t).saturating_add(1),
            None => INFINITE,
        }
    }

    /// The type an element is made with: its own, or when that is an abstract complex type one that is derived from it.
    fn concrete_type(&self, id: TypeId) -> Option<TypeId> {
        let entry = self.schema.type_entry(id)?;
        match &entry.kind {
            TypeKind::Complex(c) if c.abstract_ => (0..self.schema.types.len()).map(TypeId).find(|&t| {
                t != id
                    && self.schema.derives_from(t, id)
                    && matches!(self.schema.type_entry(t).map(|e| &e.kind), Some(TypeKind::Complex(c)) if !c.abstract_)
            }),
            _ => Some(id),
        }
    }

    fn type_cost(&self, id: TypeId) -> u32 {
        match self.schema.type_entry(id).map(|e| &e.kind) {
            Some(TypeKind::Complex(c)) => match &c.content {
                ContentType::ElementOnly(content) | ContentType::Mixed(content) => self.particle_cost(&content.particle),
                ContentType::Empty | ContentType::Simple(_) => 0,
            },
            _ => 0,
        }
    }

    /// The least height a particle needs: nothing when it may be left out.
    fn particle_cost(&self, p: &Particle) -> u32 {
        if p.min == 0 {
            return 0;
        }
        match &p.term {
            Term::Element(id) => self.height(*id),
            Term::Wildcard(_) => 1,
            Term::Sequence(items) | Term::All(items) => items.iter().map(|i| self.particle_cost(i)).max().unwrap_or(0),
            Term::Choice(items) => items.iter().map(|i| self.particle_cost(i)).min().unwrap_or(INFINITE),
        }
    }

    /// The element to make for `id`: itself, or when it is abstract one that can stand for it (the groups are walked in turn).
    fn effective(&mut self, id: ElementId) -> Option<ElementId> {
        let decl = self.schema.element(id)?;
        if !decl.abstract_ {
            return (self.height(id) != INFINITE).then_some(id);
        }
        let usable: Vec<ElementId> = self.members.get(id.0).into_iter().flatten().copied().filter(|m| self.height(*m) != INFINITE).collect();
        let at = self.next_rotation();
        usable.get(at % usable.len().max(1)).copied()
    }

    fn next_rotation(&mut self) -> usize {
        self.rotation += 1;
        self.rotation - 1
    }

    fn element(&mut self, id: ElementId, depth: usize) -> Result<Node, SampleError> {
        self.path.push(id);
        let made = self.element_inner(id, depth);
        self.path.pop();
        made
    }

    fn element_inner(&mut self, id: ElementId, depth: usize) -> Result<Node, SampleError> {
        if depth > HARD_DEPTH {
            return Err(SampleError::TooDeep);
        }
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(SampleError::TooLarge);
        }
        let schema = self.schema;
        let Some(decl) = schema.element(id) else { return Err(SampleError::NoRoot) };
        let mut node = Node::new(decl.name.clone());
        let Some(type_id) = self.concrete_type(decl.type_id) else {
            return Err(SampleError::Unsatisfiable(decl.name.to_string()));
        };
        if type_id != decl.type_id
            && let Some(name) = schema.type_entry(type_id).and_then(|t| t.name.clone())
        {
            node.attributes.push((QName::new(Some(XSI_NAMESPACE), "type"), AttrValue::Name(name)));
        }
        let Some(entry) = schema.type_entry(type_id) else { return Err(SampleError::NoRoot) };
        match &entry.kind {
            TypeKind::Simple(simple) => {
                node.text = Some(match &decl.value_constraint {
                    Some(c) => c.lexical.clone(),
                    None => self.value(simple, &decl.name.local, "element"),
                });
            }
            TypeKind::Complex(complex) => {
                for usage in &complex.attributes {
                    let Some(attribute) = schema.attribute(usage.attribute) else { continue };
                    let constraint = usage.value_constraint.as_ref().or(attribute.value_constraint.as_ref());
                    // An optional attribute with a fixed value says nothing the schema does not.
                    if !usage.required && !self.options.optional {
                        continue;
                    }
                    let text = match constraint {
                        Some(c) => c.lexical.clone(),
                        None => self.value(&attribute.type_, &attribute.name.local, "attribute"),
                    };
                    node.attributes.push((attribute.name.clone(), AttrValue::Text(text)));
                }
                match &complex.content {
                    ContentType::Empty => {}
                    ContentType::Simple(simple) => {
                        node.text = Some(match &decl.value_constraint {
                            Some(c) => c.lexical.clone(),
                            None => self.value(simple, &decl.name.local, "element"),
                        });
                    }
                    ContentType::ElementOnly(content) | ContentType::Mixed(content) => {
                        let mut children = Vec::new();
                        self.particle(&content.particle, depth, &mut children)?;
                        node.children = children;
                    }
                }
            }
        }
        Ok(node)
    }

    fn value(&mut self, ty: &xmlcraft_datatypes::SimpleType, name: &str, what: &str) -> String {
        match self.values.make(ty, name) {
            Some(v) => v,
            None => {
                self.warnings.push(format!("No value that fits the type of the {what} '{name}' was found."));
                String::new()
            }
        }
    }

    fn occurrences(&self, p: &Particle, at_cap: bool) -> u32 {
        if at_cap {
            return p.min;
        }
        if p.min == 0 && !self.options.optional {
            return 0;
        }
        let most = p.max.unwrap_or(MAX_OCCURRENCES);
        u32::try_from(self.options.repeat).unwrap_or(u32::MAX).clamp(p.min, most.max(p.min))
    }

    fn particle(&mut self, p: &Particle, depth: usize, out: &mut Vec<Node>) -> Result<(), SampleError> {
        let at_cap = depth >= self.options.max_depth;
        let count = self.occurrences(p, at_cap);
        if count > MAX_OCCURRENCES {
            return Err(SampleError::TooLarge);
        }
        for _ in 0..count {
            self.term(&p.term, depth, at_cap, p.min > 0, out)?;
        }
        Ok(())
    }

    /// `required`: the particle that holds `term` must occur at least once (only a wildcard asks).
    fn term(&mut self, term: &Term, depth: usize, at_cap: bool, required: bool, out: &mut Vec<Node>) -> Result<(), SampleError> {
        match term {
            Term::Element(id) => match self.effective(*id) {
                Some(effective) => out.push(self.element(effective, depth + 1)?),
                None => {
                    let name = self.schema.element(*id).map(|e| e.name.to_string()).unwrap_or_default();
                    return Err(SampleError::Unsatisfiable(name));
                }
            },
            Term::Sequence(items) | Term::All(items) => {
                for item in items {
                    self.particle(item, depth, out)?;
                }
            }
            Term::Choice(items) => {
                let usable: Vec<usize> = (0..items.len()).filter(|&i| items.get(i).is_some_and(|p| self.particle_cost(p) != INFINITE)).collect();
                let chosen = if at_cap {
                    usable.iter().copied().min_by_key(|&i| items.get(i).map(|p| self.particle_cost(p)).unwrap_or(INFINITE))
                } else {
                    match self.options.choice {
                        ChoicePolicy::First => usable.first().copied(),
                        ChoicePolicy::Rotate => {
                            let at = self.next_rotation();
                            usable.get(at % usable.len().max(1)).copied()
                        }
                    }
                };
                match chosen.and_then(|i| items.get(i)) {
                    Some(branch) => self.particle(branch, depth, out)?,
                    None if items.is_empty() => {}
                    None => return Err(SampleError::Unsatisfiable("a choice no branch of which can be made".to_owned())),
                }
            }
            Term::Wildcard(wildcard) => self.wildcard(wildcard, depth, required, out)?,
        }
        Ok(())
    }

    /// Something the wildcard allows: the global element, declared in a namespace it lets in, that is the shallowest (the first of them),
    /// leaving out the elements that are being made, so that a wildcard does not nest the document in itself. When none is declared, an
    /// extension point that may be left out is left out; one that is required and does not check what it lets in (`lax` or `skip`) gets a
    /// placeholder element, which is valid since nothing is checked of it; a required `strict` one cannot be satisfied, and says so.
    fn wildcard(&mut self, wildcard: &Wildcard, depth: usize, required: bool, out: &mut Vec<Node>) -> Result<(), SampleError> {
        let found = (0..self.schema.elements.len())
            .map(ElementId)
            .filter(|id| !self.path.contains(id))
            .filter(|&id| {
                self.schema.is_global(id)
                    && self.schema.element(id).is_some_and(|e| !e.abstract_ && wildcard.constraint.allows(e.name.namespace.as_deref()))
                    && self.height(id) != INFINITE
            })
            .min_by_key(|&id| self.height(id));
        match found {
            Some(id) => out.push(self.element(id, depth + 1)?),
            None if !required => {}
            None if wildcard.process_contents != ProcessContents::Strict => {
                self.nodes += 1;
                if self.nodes > MAX_NODES {
                    return Err(SampleError::TooLarge);
                }
                out.push(placeholder(&wildcard.constraint));
            }
            None => self.warnings.push("A wildcard needs an element, and none is declared that it allows.".to_owned()),
        }
        Ok(())
    }
}

/// The namespace the placeholder element of a wildcard is put in when the wildcard lets in any.
const PLACEHOLDER_NAMESPACE: &str = "urn:example:extension";

/// An element no schema declares, in a namespace the wildcard lets in.
fn placeholder(constraint: &NamespaceSet) -> Node {
    let namespace = match constraint {
        NamespaceSet::Any => Some(PLACEHOLDER_NAMESPACE),
        // Anything but the target namespace (and no namespace): the placeholder's own, unless that is the one left out.
        NamespaceSet::Not(excluded) if excluded.as_deref() == Some(PLACEHOLDER_NAMESPACE) => Some("urn:example:other"),
        NamespaceSet::Not(_) => Some(PLACEHOLDER_NAMESPACE),
        // The first namespace that is listed; if only no namespace is, none.
        NamespaceSet::List(list) => list.iter().find_map(|n| n.as_deref()),
    };
    Node::new(QName::new(namespace, "extension"))
}
