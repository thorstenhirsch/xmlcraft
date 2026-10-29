//! A document generated from a DTD.
//!
//! The same way as for an XML Schema: each particle occurs as often as the options say, kept between its least and its greatest number,
//! and the nesting of a recursive DTD ends with the *height* of each element, the least depth of a valid tree below it.

use std::collections::HashMap;

use xmlcraft_dtd::{AttType, ContentSpec, Default as AttDefault, Dtd, Occurs, Particle, ParticleKind};
use xmlcraft_syntax::decode_references;
use xmlcraft_xsd::model::QName;

use crate::tree::{AttrValue, Node};
use crate::{ChoicePolicy, Fill, Options, Sample, SampleError};

const INFINITE: u32 = u32::MAX;
const HARD_DEPTH: usize = 300;
const MAX_NODES: usize = 200_000;
const MAX_OCCURRENCES: u32 = 10_000;

/// The names of the elements a document could start with: all that are declared, in the order they were.
pub fn roots(dtd: &Dtd) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for e in &dtd.elements {
        if !seen.contains(&e.name) {
            seen.push(e.name.clone());
        }
    }
    seen
}

pub fn sample(dtd: &Dtd, root: Option<&str>, options: &Options) -> Result<Sample, SampleError> {
    let mut generator = Generator::new(dtd, options);
    let name = match root {
        Some(name) => {
            dtd.element(name).ok_or_else(|| SampleError::UnknownRoot(name.to_owned()))?;
            name.to_owned()
        }
        None => dtd.elements.iter().map(|e| e.name.clone()).find(|n| generator.height(n) != INFINITE).ok_or(SampleError::NoRoot)?,
    };
    if generator.height(&name) == INFINITE {
        return Err(SampleError::Unsatisfiable(name));
    }
    let node = generator.element(&name, 0)?;
    if generator.refs > 0 && generator.ids == 0 {
        generator.warnings.push("The document has a reference to an ID, and no ID.".to_owned());
    }
    // The names of a DTD are written as they are: whatever prefix they have is the document's own.
    Ok(Sample { text: crate::tree::serialize(&node, options.schema_location.as_deref()), warnings: generator.warnings })
}

struct Generator<'a> {
    dtd: &'a Dtd,
    options: &'a Options,
    heights: HashMap<&'a str, u32>,
    rotation: usize,
    /// How often a value was made for each attribute, so that its enumeration is walked through in turn.
    turns: HashMap<String, usize>,
    ids: usize,
    refs: usize,
    nodes: usize,
    warnings: Vec<String>,
}

impl<'a> Generator<'a> {
    fn new(dtd: &'a Dtd, options: &'a Options) -> Self {
        let mut generator =
            Self { dtd, options, heights: HashMap::new(), rotation: 0, turns: HashMap::new(), ids: 0, refs: 0, nodes: 0, warnings: Vec::new() };
        generator.compute_heights();
        generator
    }

    fn compute_heights(&mut self) {
        for _ in 0..=self.dtd.elements.len() {
            let mut changed = false;
            for e in &self.dtd.elements {
                let h = match &e.spec {
                    ContentSpec::Empty | ContentSpec::Any | ContentSpec::Mixed(_) => 1,
                    ContentSpec::Children(p) => self.cost(p).saturating_add(1),
                };
                if self.heights.get(e.name.as_str()).is_none_or(|&old| h < old) && h != INFINITE {
                    self.heights.insert(e.name.as_str(), h);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    fn height(&self, name: &str) -> u32 {
        self.heights.get(name).copied().unwrap_or(INFINITE)
    }

    fn min_of(occurs: Occurs) -> u32 {
        match occurs {
            Occurs::Once | Occurs::OneOrMore => 1,
            Occurs::Optional | Occurs::ZeroOrMore => 0,
        }
    }

    fn cost(&self, p: &Particle) -> u32 {
        if Self::min_of(p.occurs) == 0 {
            return 0;
        }
        match &p.kind {
            ParticleKind::Name(n) => self.height(n),
            ParticleKind::Sequence(items) => items.iter().map(|i| self.cost(i)).max().unwrap_or(0),
            ParticleKind::Choice(items) => items.iter().map(|i| self.cost(i)).min().unwrap_or(INFINITE),
        }
    }

    fn element(&mut self, name: &str, depth: usize) -> Result<Node, SampleError> {
        if depth > HARD_DEPTH {
            return Err(SampleError::TooDeep);
        }
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(SampleError::TooLarge);
        }
        let dtd = self.dtd;
        let mut node = Node::new(QName::new(None, name));
        self.attributes(name, &mut node);
        let Some(decl) = dtd.element(name) else {
            self.warnings.push(format!("The element '{name}' is not declared."));
            return Ok(node);
        };
        match &decl.spec {
            ContentSpec::Empty | ContentSpec::Any => {}
            ContentSpec::Mixed(names) => {
                let at_cap = depth >= self.options.max_depth;
                if names.is_empty() || at_cap || !self.options.optional {
                    node.text = Some(self.text(name));
                } else {
                    // Each of the names once, and text first.
                    node.text = Some(self.text(name));
                    let usable: Vec<&String> = names.iter().filter(|n| self.height(n) != INFINITE).collect();
                    for n in usable {
                        node.children.push(self.element(n, depth + 1)?);
                    }
                }
            }
            ContentSpec::Children(p) => {
                let mut children = Vec::new();
                self.particle(p, depth, &mut children)?;
                node.children = children;
            }
        }
        Ok(node)
    }

    fn text(&self, element: &str) -> String {
        if self.options.fill == Fill::Named { element.to_owned() } else { "text".to_owned() }
    }

    fn attributes(&mut self, element: &str, node: &mut Node) {
        let dtd = self.dtd;
        let mut seen: Vec<&str> = Vec::new();
        for def in dtd.attlists.iter().filter(|a| a.element == element).flat_map(|a| a.defs.iter()) {
            // The first declaration of an attribute is the one that counts.
            if seen.contains(&def.name.as_str()) {
                continue;
            }
            seen.push(&def.name);
            if !matches!(def.default, AttDefault::Required) && !self.options.optional {
                continue;
            }
            let text = match &def.default {
                AttDefault::Fixed(v) | AttDefault::Value(v) => decode_references(v),
                AttDefault::Required | AttDefault::Implied => self.attribute_value(&def.ty, element, &def.name),
            };
            node.attributes.push((QName::new(None, &def.name), AttrValue::Text(text)));
        }
    }

    fn attribute_value(&mut self, ty: &AttType, element: &str, name: &str) -> String {
        match ty {
            AttType::Cdata => {
                if self.options.fill == Fill::Named {
                    name.to_owned()
                } else {
                    "text".to_owned()
                }
            }
            AttType::Id => {
                self.ids += 1;
                format!("id{}", self.ids)
            }
            AttType::Idref | AttType::Idrefs => {
                self.refs += 1;
                "id1".to_owned()
            }
            AttType::Entity | AttType::Entities => match self.dtd.general_entities.first() {
                Some(e) => e.name.clone(),
                None => {
                    self.warnings.push(format!("The attribute '{name}' names an entity, and none is declared."));
                    "entity".to_owned()
                }
            },
            AttType::Nmtoken | AttType::Nmtokens => "token".to_owned(),
            AttType::Notation(values) | AttType::Enumeration(values) => {
                let count = self.turns.entry(format!("{element}@{name}")).or_insert(0);
                let at = *count;
                *count += 1;
                values.get(at % values.len().max(1)).cloned().unwrap_or_default()
            }
        }
    }

    fn next_rotation(&mut self) -> usize {
        self.rotation += 1;
        self.rotation - 1
    }

    fn occurrences(&self, occurs: Occurs, at_cap: bool) -> u32 {
        let (min, max) = match occurs {
            Occurs::Once => (1, 1),
            Occurs::Optional => (0, 1),
            Occurs::ZeroOrMore => (0, MAX_OCCURRENCES),
            Occurs::OneOrMore => (1, MAX_OCCURRENCES),
        };
        if at_cap {
            return min;
        }
        if min == 0 && !self.options.optional {
            return 0;
        }
        u32::try_from(self.options.repeat).unwrap_or(u32::MAX).clamp(min, max)
    }

    fn particle(&mut self, p: &Particle, depth: usize, out: &mut Vec<Node>) -> Result<(), SampleError> {
        let at_cap = depth >= self.options.max_depth;
        for _ in 0..self.occurrences(p.occurs, at_cap) {
            match &p.kind {
                ParticleKind::Name(n) => {
                    if self.height(n) == INFINITE {
                        return Err(SampleError::Unsatisfiable(n.clone()));
                    }
                    out.push(self.element(n, depth + 1)?);
                }
                ParticleKind::Sequence(items) => {
                    for item in items {
                        self.particle(item, depth, out)?;
                    }
                }
                ParticleKind::Choice(items) => {
                    let usable: Vec<usize> = (0..items.len()).filter(|&i| items.get(i).is_some_and(|p| self.cost(p) != INFINITE)).collect();
                    let chosen = if at_cap {
                        usable.iter().copied().min_by_key(|&i| items.get(i).map(|p| self.cost(p)).unwrap_or(INFINITE))
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
            }
        }
        Ok(())
    }
}
