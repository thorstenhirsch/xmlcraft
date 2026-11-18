//! XBRL Dimensions 1.0: the dimensional relationships of a taxonomy's definition linkbases ([`Dimensions`], built by
//! [`crate::taxonomy::load`]) and the dimensional validation of an instance's facts ([`check_dimensions`]).
//!
//! **The model** (sections 2.1–2.7 of the specification). The definition arcs with the six dimensional arcroles, after prohibition and
//! priority, make *has-hypercube* relationships (`all`, `notAll`: a primary item to a hypercube, with `xbrldt:contextElement` and
//! `xbrldt:closed`), *hypercube-dimension*, *dimension-domain*, *domain-member* and *dimension-default* relationships. Consecutive
//! relationships are looked up in the same extended link role, or in the one an arc's `xbrldt:targetRole` names (2.4). The domain of
//! an explicit dimension in a hypercube is every member reachable from its dimension-domain relationships through domain-member ones,
//! less the targets of arcs with `xbrldt:usable="false"` (2.5.3.3: a member unusable anywhere in the domain is excluded). A primary
//! item has the hypercubes of its own has-hypercube relationships and inherits those of its ancestors along domain-member relationships
//! in the same base set (2.6). Every walk is bounded (seen sets, a depth cap, caps on members and relationships).
//!
//! **The validation** (3.1). A fact whose concept has hypercubes is dimensionally valid when, in at least one base set (extended link
//! role) of its has-hypercube relationships, its context is valid for every `all` hypercube and for no `notAll` one. A context is valid
//! for a hypercube when, in the container the relationship names (the segment or the scenario), each dimension of the hypercube has a
//! value that is a usable member of its domain (an explicit dimension without a value takes its default, when it has one; a typed
//! dimension needs a value), and, when the hypercube is closed, the container has no value of another dimension. The checks of the
//! dimension values themselves (3.1.4: an explicit member of a dimension that is not explicit, a typed member of one that is not typed,
//! a member that names no element, the same dimension twice, a default written out, a typed member that is not one element) come
//! first; a context they find wrong is not judged again for each of its facts.
//!
//! Not done: the content of a typed member is not validated against its `xbrldt:typedDomainRef` declaration (only that it is one
//! element); roles are not checked to be declared (`xbrldte:TargetRoleNotResolvedError`), nor cycles reported (they are cut);
//! tuples are not primary items here.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::taxonomy::Taxonomy;
use crate::{Context, Finding, Instance, Level};

/// The XBRL Dimensions taxonomy namespace (`xbrldt`).
pub const XBRLDT: &str = "http://xbrl.org/2005/xbrldt";
const ARCROLE: &str = "http://xbrl.org/int/dim/arcrole/";
/// The most members walked for one dimension domain.
const MAX_MEMBERS: usize = 100_000;
/// How deep a chain of domain-member relationships is followed.
const MAX_DEPTH: usize = 256;
/// The most has-hypercube entries the primary items get (inherited ones counted).
const MAX_ENTRIES: usize = 1_000_000;
/// The most problems of the taxonomy listed.
const MAX_PROBLEMS: usize = 200;
/// The most findings about facts and contexts.
const MAX_FINDINGS: usize = 10_000;

/// The arcrole of a dimensional relationship.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Arcrole {
    All,
    NotAll,
    HypercubeDimension,
    DimensionDomain,
    DomainMember,
    DimensionDefault,
}

impl Arcrole {
    /// The arcrole an `xlink:arcrole` URI names; none for one that is not dimensional.
    pub fn from_uri(uri: &str) -> Option<Arcrole> {
        Some(match uri.trim().strip_prefix(ARCROLE)? {
            "all" => Arcrole::All,
            "notAll" => Arcrole::NotAll,
            "hypercube-dimension" => Arcrole::HypercubeDimension,
            "dimension-domain" => Arcrole::DimensionDomain,
            "domain-member" => Arcrole::DomainMember,
            "dimension-default" => Arcrole::DimensionDefault,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Arcrole::All => "all",
            Arcrole::NotAll => "notAll",
            Arcrole::HypercubeDimension => "hypercube-dimension",
            Arcrole::DimensionDomain => "dimension-domain",
            Arcrole::DomainMember => "domain-member",
            Arcrole::DimensionDefault => "dimension-default",
        }
    }
}

/// A dimensional relationship between two concepts (`{namespace}local`) as resolved from a definition arc, with its attributes as
/// written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DefArc {
    pub role: String,
    pub arcrole: Arcrole,
    pub from: String,
    pub to: String,
    pub context_element: Option<String>,
    pub closed: Option<String>,
    pub target_role: Option<String>,
    pub usable: Option<String>,
}

/// Where a hypercube's dimensions are in a context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ContextElement {
    Segment,
    Scenario,
}

impl ContextElement {
    fn word(self) -> &'static str {
        match self {
            ContextElement::Segment => "segment",
            ContextElement::Scenario => "scenario",
        }
    }
}

/// A dimension of a hypercube as a base set has it: its members (the usable ones) when it is explicit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CubeDimension {
    pub dimension: String,
    pub typed: bool,
    pub members: BTreeSet<String>,
}

/// A hypercube with its dimensions as the relationships in one extended link role say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cube {
    pub hypercube: String,
    /// The role its hypercube-dimension relationships are in.
    pub role: String,
    pub dimensions: Vec<CubeDimension>,
}

/// A has-hypercube relationship of a primary item, its own or inherited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HasHypercube {
    /// The base set: the extended link role of the `all` or `notAll` arc.
    pub role: String,
    /// The hypercube, an index into [`Dimensions::cubes`].
    pub cube: usize,
    /// `all` (else `notAll`).
    pub all: bool,
    pub context_element: ContextElement,
    pub closed: bool,
    /// The ancestor it is inherited from, along domain-member relationships; none for the item's own.
    pub inherited_from: Option<String>,
}

/// A violation of the taxonomy rules of XBRL Dimensions 1.0, with its `xbrldte` code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaxonomyProblem {
    pub code: &'static str,
    pub message: String,
}

/// The dimensional model of a taxonomy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dimensions {
    /// The concepts in the substitution group `xbrldt:hypercubeItem`.
    pub hypercubes: BTreeSet<String>,
    /// The concepts in the substitution group `xbrldt:dimensionItem`, each with its `xbrldt:typedDomainRef` when it is typed.
    pub dimensions: BTreeMap<String, Option<String>>,
    /// The default member of each explicit dimension that has one.
    pub defaults: BTreeMap<String, String>,
    pub cubes: Vec<Cube>,
    /// The hypercubes of each primary item.
    pub primary_items: BTreeMap<String, Vec<HasHypercube>>,
    /// What breaks the taxonomy rules (the relationships at fault are left out of the model).
    pub problems: Vec<TaxonomyProblem>,
    /// A cap was reached: the model is not complete.
    pub truncated: bool,
}

impl Dimensions {
    /// Whether the taxonomy has nothing dimensional.
    pub fn is_empty(&self) -> bool {
        self.hypercubes.is_empty() && self.dimensions.is_empty() && self.primary_items.is_empty()
    }

    /// How many distinct (dimension, member) pairs the domains have.
    pub fn member_count(&self) -> usize {
        let mut seen: HashSet<(&str, &str)> = HashSet::new();
        for cube in &self.cubes {
            for d in &cube.dimensions {
                for m in &d.members {
                    seen.insert((d.dimension.as_str(), m.as_str()));
                }
            }
        }
        seen.len()
    }

    /// Whether `dimension` is a typed dimension (`Some(true)`), an explicit one (`Some(false)`) or no dimension at all (none).
    fn kind(&self, dimension: &str) -> Option<bool> {
        self.dimensions.get(dimension).map(Option::is_some)
    }
}

fn problem(model: &mut Dimensions, code: &'static str, message: String) {
    if model.problems.len() < MAX_PROBLEMS {
        model.problems.push(TaxonomyProblem { code, message });
    }
}

/// The local part of `{namespace}local`, for messages.
fn local(key: &str) -> &str {
    key.rsplit_once('}').map_or(key, |(_, l)| l)
}

fn namespace_of(key: &str) -> Option<&str> {
    key.strip_prefix('{').and_then(|r| r.split_once('}')).map(|(ns, _)| ns)
}

type Index<'a> = HashMap<(Arcrole, &'a str, &'a str), Vec<&'a DefArc>>;

/// Build the model from the relationships `arcs` (already resolved, prohibited ones removed), with the concepts of `taxonomy`.
pub(crate) fn build(taxonomy: &Taxonomy, arcs: Vec<DefArc>) -> Dimensions {
    let mut model = Dimensions::default();
    let hypercube_group = format!("{{{XBRLDT}}}hypercubeItem");
    let dimension_group = format!("{{{XBRLDT}}}dimensionItem");
    let mut keys: Vec<&String> = taxonomy.concepts.keys().collect();
    keys.sort();
    for key in keys {
        let Some(concept) = taxonomy.concepts.get(key) else { continue };
        if taxonomy.in_substitution_group(concept, &hypercube_group) {
            model.hypercubes.insert(key.clone());
            if !concept.abstract_concept {
                problem(&mut model, "HypercubeElementIsNotAbstractError", format!("The hypercube {} is not abstract.", local(key)));
            }
        } else if taxonomy.in_substitution_group(concept, &dimension_group) {
            model.dimensions.insert(key.clone(), concept.typed_domain_ref.clone());
            if !concept.abstract_concept {
                problem(&mut model, "DimensionElementIsNotAbstractError", format!("The dimension {} is not abstract.", local(key)));
            }
        } else if concept.typed_domain_ref.is_some() {
            problem(&mut model, "TypedDimensionError", format!("{} has an xbrldt:typedDomainRef and is not a dimension.", local(key)));
        }
    }
    let is_cube = |c: &str| model.hypercubes.contains(c);
    let is_dim = |c: &str| model.dimensions.contains_key(c);
    let is_explicit = |c: &str| model.dimensions.get(c).is_some_and(Option::is_none);
    // The relationships that keep to the rules of their arcrole (2.2, 2.5, 2.7); the others are said and left out.
    let mut kept: Vec<&DefArc> = Vec::new();
    let mut problems: Vec<(&'static str, String)> = Vec::new();
    for arc in &arcs {
        let (from, to, what) = (local(&arc.from), local(&arc.to), arc.arcrole.name());
        let fault: Option<(&'static str, String)> = match arc.arcrole {
            Arcrole::All | Arcrole::NotAll => {
                if is_cube(&arc.from) || is_dim(&arc.from) {
                    Some((
                        "HasHypercubeSourceError",
                        format!("The {what} relationship from {from} starts at a hypercube or a dimension, not a primary item."),
                    ))
                } else if !is_cube(&arc.to) {
                    Some(("HasHypercubeTargetError", format!("The {what} relationship from {from} goes to {to}, which is not a hypercube.")))
                } else {
                    match arc.context_element.as_deref() {
                        Some("segment" | "scenario") => None,
                        Some(other) => Some((
                            "HasHypercubeMissingContextElementAttributeError",
                            format!("The {what} relationship from {from} to {to} has xbrldt:contextElement=\"{other}\": it is segment or scenario."),
                        )),
                        None => Some((
                            "HasHypercubeMissingContextElementAttributeError",
                            format!("The {what} relationship from {from} to {to} has no xbrldt:contextElement."),
                        )),
                    }
                }
            }
            Arcrole::HypercubeDimension if !is_cube(&arc.from) => Some((
                "HypercubeDimensionSourceError",
                format!("The hypercube-dimension relationship to {to} starts at {from}, which is not a hypercube."),
            )),
            Arcrole::HypercubeDimension if !is_dim(&arc.to) => Some((
                "HypercubeDimensionTargetError",
                format!("The hypercube-dimension relationship from {from} goes to {to}, which is not a dimension."),
            )),
            Arcrole::DimensionDomain if !is_explicit(&arc.from) => Some((
                "DimensionDomainSourceError",
                format!("The dimension-domain relationship to {to} starts at {from}, which is not an explicit dimension."),
            )),
            Arcrole::DimensionDomain if is_cube(&arc.to) || is_dim(&arc.to) => Some((
                "DimensionDomainTargetError",
                format!("The dimension-domain relationship from {from} goes to {to}, a hypercube or a dimension."),
            )),
            Arcrole::DomainMember if is_cube(&arc.from) || is_dim(&arc.from) => {
                Some(("DomainMemberSourceError", format!("The domain-member relationship to {to} starts at {from}, a hypercube or a dimension.")))
            }
            Arcrole::DomainMember if is_cube(&arc.to) || is_dim(&arc.to) => {
                Some(("DomainMemberTargetError", format!("The domain-member relationship from {from} goes to {to}, a hypercube or a dimension.")))
            }
            Arcrole::DimensionDefault if !is_explicit(&arc.from) => Some((
                "DimensionDefaultSourceError",
                format!("The dimension-default relationship to {to} starts at {from}, which is not an explicit dimension."),
            )),
            Arcrole::DimensionDefault if is_cube(&arc.to) || is_dim(&arc.to) => Some((
                "DimensionDefaultTargetError",
                format!("The dimension-default relationship from {from} goes to {to}, a hypercube or a dimension."),
            )),
            _ => None,
        };
        match fault {
            Some(f) => problems.push(f),
            None => kept.push(arc),
        }
    }
    for (code, message) in problems {
        problem(&mut model, code, message);
    }
    // Defaults hold in every role (2.7); one dimension has at most one.
    for arc in kept.iter().filter(|a| a.arcrole == Arcrole::DimensionDefault) {
        match model.defaults.get(&arc.from) {
            Some(d) if *d != arc.to => {
                let message = format!("The dimension {} has more than one default ({} and {}).", local(&arc.from), local(d), local(&arc.to));
                problem(&mut model, "TooManyDefaultMembersError", message);
            }
            Some(_) => {}
            None => {
                model.defaults.insert(arc.from.clone(), arc.to.clone());
            }
        }
    }
    let mut index: Index = HashMap::new();
    for &arc in &kept {
        index.entry((arc.arcrole, arc.role.as_str(), arc.from.as_str())).or_default().push(arc);
    }
    let mut cubes: HashMap<(String, String), usize> = HashMap::new();
    let mut entries = 0usize;
    for arc in kept.iter().filter(|a| matches!(a.arcrole, Arcrole::All | Arcrole::NotAll)) {
        let context_element = if arc.context_element.as_deref() == Some("scenario") { ContextElement::Scenario } else { ContextElement::Segment };
        let closed = matches!(arc.closed.as_deref(), Some("true" | "1"));
        let dimensions_role = arc.target_role.clone().unwrap_or_else(|| arc.role.clone());
        let key = (arc.to.clone(), dimensions_role);
        let cube = match cubes.get(&key) {
            Some(&n) => n,
            None => {
                let made = cube_of(&model, &index, &key.0, &key.1);
                model.truncated |= made.1;
                model.cubes.push(made.0);
                let n = model.cubes.len() - 1;
                cubes.insert(key, n);
                n
            }
        };
        let own = HasHypercube { role: arc.role.clone(), cube, all: arc.arcrole == Arcrole::All, context_element, closed, inherited_from: None };
        // The item itself, then what inherits from it: its descendants along domain-member relationships of the same base set (2.6).
        let (heirs, cut) = descendants(&index, &arc.from, &arc.role);
        model.truncated |= cut;
        for (item, from) in std::iter::once((arc.from.clone(), None)).chain(heirs.into_iter().map(|h| (h, Some(arc.from.clone())))) {
            if entries >= MAX_ENTRIES {
                model.truncated = true;
                break;
            }
            let list = model.primary_items.entry(item).or_default();
            let entry = HasHypercube { inherited_from: from, ..own.clone() };
            let same = |e: &HasHypercube| {
                (e.role.as_str(), e.cube, e.all, e.context_element, e.closed)
                    == (entry.role.as_str(), entry.cube, entry.all, entry.context_element, entry.closed)
            };
            if !list.iter().any(same) {
                list.push(entry);
                entries += 1;
            }
        }
    }
    model
}

/// The role the relationships after `arc` are in: the one its `xbrldt:targetRole` names, else its own.
fn next_role(arc: &DefArc) -> &str {
    arc.target_role.as_deref().unwrap_or(&arc.role)
}

/// The hypercube `hypercube` with the dimensions its hypercube-dimension relationships in `role` give it, and their domains. True when a
/// cap cut it short.
fn cube_of(model: &Dimensions, index: &Index<'_>, hypercube: &str, role: &str) -> (Cube, bool) {
    let mut cube = Cube { hypercube: hypercube.to_owned(), role: role.to_owned(), dimensions: Vec::new() };
    let mut cut = false;
    for hd in index.get(&(Arcrole::HypercubeDimension, role, hypercube)).map(Vec::as_slice).unwrap_or_default() {
        if cube.dimensions.iter().any(|d| d.dimension == hd.to) {
            continue;
        }
        let typed = model.kind(&hd.to) == Some(true);
        let members = if typed {
            BTreeSet::new()
        } else {
            let (members, c) = domain(index, &hd.to, next_role(hd));
            cut |= c;
            members
        };
        cube.dimensions.push(CubeDimension { dimension: hd.to.clone(), typed, members });
    }
    (cube, cut)
}

/// The usable members of the domain of `dimension` that its dimension-domain relationships in `role` start; true when a cap cut it.
fn domain(index: &Index<'_>, dimension: &str, role: &str) -> (BTreeSet<String>, bool) {
    let mut usable: BTreeSet<String> = BTreeSet::new();
    let mut unusable: HashSet<String> = HashSet::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut stack: Vec<(&DefArc, usize)> = Vec::new();
    let mut cut = false;
    for dd in index.get(&(Arcrole::DimensionDomain, role, dimension)).map(Vec::as_slice).unwrap_or_default() {
        stack.push((dd, 0));
    }
    while let Some((arc, depth)) = stack.pop() {
        if usable.len().saturating_add(unusable.len()) >= MAX_MEMBERS {
            cut = true;
            break;
        }
        if matches!(arc.usable.as_deref(), Some("false" | "0")) {
            unusable.insert(arc.to.clone());
        } else {
            usable.insert(arc.to.clone());
        }
        let role = next_role(arc);
        if !seen.insert((arc.to.clone(), role.to_owned())) {
            continue;
        }
        if depth >= MAX_DEPTH {
            cut = true;
            continue;
        }
        for dm in index.get(&(Arcrole::DomainMember, role, arc.to.as_str())).map(Vec::as_slice).unwrap_or_default() {
            stack.push((dm, depth + 1));
        }
    }
    usable.retain(|m| !unusable.contains(m));
    (usable, cut)
}

/// The descendants of the primary item `item` along domain-member relationships in `role` (and the roles their `targetRole`s name);
/// true when a cap cut it.
fn descendants(index: &Index<'_>, item: &str, role: &str) -> (Vec<String>, bool) {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::from([(item.to_owned(), role.to_owned())]);
    let mut listed: HashSet<String> = HashSet::from([item.to_owned()]);
    let mut stack: Vec<(&str, &str, usize)> = vec![(item, role, 0)];
    let mut cut = false;
    while let Some((node, role, depth)) = stack.pop() {
        if depth >= MAX_DEPTH || out.len() >= MAX_MEMBERS {
            cut = true;
            continue;
        }
        for dm in index.get(&(Arcrole::DomainMember, role, node)).map(Vec::as_slice).unwrap_or_default() {
            let next = next_role(dm);
            if listed.insert(dm.to.clone()) {
                out.push(dm.to.clone());
            }
            if seen.insert((dm.to.clone(), next.to_owned())) {
                stack.push((dm.to.as_str(), next, depth + 1));
            }
        }
    }
    (out, cut)
}

/// Why a context is not valid for a hypercube; none when it is.
fn cube_fault(model: &Dimensions, context: &Context, entry: &HasHypercube) -> Option<String> {
    let cube = model.cubes.get(entry.cube)?;
    let in_scenario = entry.context_element == ContextElement::Scenario;
    let place = entry.context_element.word();
    let values: Vec<&crate::Dimension> = context.dimensions.iter().filter(|d| d.in_scenario == in_scenario).collect();
    let name = local(&cube.hypercube);
    for d in &cube.dimensions {
        match values.iter().find(|v| v.dimension == d.dimension) {
            Some(v) if d.typed => {
                if !v.typed {
                    return Some(format!(
                        "the dimension {} of the hypercube {name} is typed, and the {place} gives it an explicit member",
                        local(&d.dimension)
                    ));
                }
            }
            Some(v) => {
                if v.typed || !d.members.contains(&v.member) {
                    return Some(format!(
                        "{} is not a usable member of the dimension {} in the hypercube {name}",
                        local(&v.member),
                        local(&d.dimension)
                    ));
                }
            }
            None if d.typed => {
                return Some(format!("the hypercube {name} needs a value of the typed dimension {} in the {place}", local(&d.dimension)));
            }
            None if !model.defaults.contains_key(&d.dimension) => {
                return Some(format!(
                    "the hypercube {name} needs a member of the dimension {} in the {place}, and the dimension has no default",
                    local(&d.dimension)
                ));
            }
            None => {}
        }
    }
    if entry.closed
        && let Some(extra) = values.iter().find(|v| !cube.dimensions.iter().any(|d| d.dimension == v.dimension))
    {
        return Some(format!(
            "the hypercube {name} is closed, and the {place} has the dimension {}, which it does not have",
            local(&extra.dimension)
        ));
    }
    None
}

/// Why a fact of a primary item with these has-hypercube relationships is not valid in `context`; none when it is valid in a base set.
fn fact_fault(model: &Dimensions, context: &Context, entries: &[HasHypercube]) -> Option<String> {
    let mut by_role: BTreeMap<&str, Vec<&HasHypercube>> = BTreeMap::new();
    for e in entries {
        by_role.entry(e.role.as_str()).or_default().push(e);
    }
    let mut first: Option<String> = None;
    for (role, list) in by_role {
        let mut fault = None;
        for e in &list {
            let invalid = cube_fault(model, context, e);
            match (e.all, invalid) {
                (true, Some(why)) => fault = Some(why),
                (false, None) => {
                    let name = model.cubes.get(e.cube).map_or("", |c| local(&c.hypercube));
                    fault = Some(format!("the context matches the hypercube {name}, which the primary item excludes (notAll)"));
                }
                _ => {}
            }
            if fault.is_some() {
                break;
            }
        }
        // (Valid in this base set: the fact is valid. Else the first base set's reason is the one given.)
        let why = fault?;
        if first.is_none() {
            first = Some(format!("{why} (base set {role})"));
        }
    }
    first
}

/// What is wrong with the dimension values of `context` (3.1.4), with the taxonomy's model.
fn context_faults(context: &Context, taxonomy: &Taxonomy) -> Vec<(&'static str, String)> {
    let model = &taxonomy.dimensions;
    let mut out = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let id = &context.id;
    let loaded = |key: &str| namespace_of(key).is_some_and(|ns| taxonomy.namespaces.contains(ns));
    for v in &context.dimensions {
        let dim = local(&v.dimension);
        if !seen.insert(v.dimension.as_str()) {
            out.push((
                "xbrl.dimension-repeated",
                format!("The context {id} gives the dimension {dim} more than once (xbrldie:RepeatedDimensionInInstanceError)."),
            ));
            continue;
        }
        // A dimension in a namespace no schema document that was read declares is not judged (that is said once elsewhere).
        if namespace_of(&v.dimension).is_some() && !loaded(&v.dimension) {
            continue;
        }
        match (v.typed, model.kind(&v.dimension)) {
            (false, Some(false)) | (true, Some(true)) => {}
            (false, Some(true)) => out.push((
                "xbrl.dimension-not-explicit",
                format!(
                    "The context {id} has an explicitMember of {dim}, which is a typed dimension: use typedMember (xbrldie:ExplicitMemberNotExplicitDimensionError)."
                ),
            )),
            (true, Some(false)) => out.push((
                "xbrl.dimension-not-typed",
                format!(
                    "The context {id} has a typedMember of {dim}, which is an explicit dimension: use explicitMember (xbrldie:TypedMemberNotTypedDimensionError)."
                ),
            )),
            (false, None) => out.push((
                "xbrl.dimension-not-explicit",
                format!(
                    "The context {id} has an explicitMember of {}, which is not a dimension the taxonomy declares (xbrldie:ExplicitMemberNotExplicitDimensionError).",
                    if dim.is_empty() { "no dimension" } else { dim }
                ),
            )),
            (true, None) => out.push((
                "xbrl.dimension-not-typed",
                format!(
                    "The context {id} has a typedMember of {}, which is not a dimension the taxonomy declares (xbrldie:TypedMemberNotTypedDimensionError).",
                    if dim.is_empty() { "no dimension" } else { dim }
                ),
            )),
        }
        if v.typed {
            if v.children != 1 {
                out.push((
                    "xbrl.dimension-typed-content",
                    format!(
                        "The typedMember of {dim} in the context {id} has {} elements in it; it has one (xbrldie:IllegalTypedDimensionContentError).",
                        v.children
                    ),
                ));
            }
            continue;
        }
        let member_known = namespace_of(&v.member).is_some_and(|ns| !taxonomy.namespaces.contains(ns)) || taxonomy.concepts.contains_key(&v.member);
        if !member_known {
            out.push((
                "xbrl.dimension-member-undefined",
                format!(
                    "The member {} of the dimension {dim} in the context {id} is not an element the taxonomy declares (xbrldie:ExplicitMemberUndefinedQNameError).",
                    local(&v.member)
                ),
            ));
        } else if model.defaults.get(&v.dimension) == Some(&v.member) {
            out.push((
                "xbrl.dimension-default-used",
                format!(
                    "The context {id} gives the dimension {dim} its default member {}: a default is never written in an instance (xbrldie:DefaultValueUsedInInstanceError).",
                    local(&v.member)
                ),
            ));
        }
    }
    out
}

/// What XBRL Dimensions 1.0 says of the facts and contexts of `instance` with `taxonomy`: the problems of the taxonomy's dimensional
/// relationships (once each, at the start of the document), the dimension values of the contexts, and each fact that is not
/// dimensionally valid.
pub fn check_dimensions(instance: &Instance, taxonomy: &Taxonomy) -> Vec<Finding> {
    let model = &taxonomy.dimensions;
    let mut found: Vec<Finding> = Vec::new();
    for p in &model.problems {
        found.push(Finding {
            code: "xbrl.dimension-taxonomy",
            level: Level::Error,
            message: format!("{} (xbrldte:{})", p.message, p.code),
            span: 0..0,
        });
    }
    if model.truncated {
        found.push(Finding {
            code: "xbrl.dimension-unsupported",
            level: Level::Warning,
            message: "The dimensional relationships of the taxonomy are too many or too deep to follow in full: some are not used.".to_owned(),
            span: 0..0,
        });
    }
    let any_dimensions = instance.contexts.iter().any(|c| !c.dimensions.is_empty());
    if model.is_empty() && !any_dimensions {
        return found;
    }
    let mut bad: HashSet<&str> = HashSet::new();
    for context in &instance.contexts {
        for (code, message) in context_faults(context, taxonomy) {
            bad.insert(context.id.as_str());
            if found.len() < MAX_FINDINGS {
                found.push(Finding { code, level: Level::Error, message, span: context.span.clone() });
            }
        }
    }
    let contexts: HashMap<&str, &Context> = instance.contexts.iter().map(|c| (c.id.as_str(), c)).collect();
    for fact in &instance.facts {
        if found.len() >= MAX_FINDINGS {
            break;
        }
        let Some(entries) = model.primary_items.get(&fact.concept) else { continue };
        let Some(context) = fact.context_ref.as_deref().filter(|c| !bad.contains(c)).and_then(|c| contexts.get(c)) else { continue };
        if let Some(why) = fact_fault(model, context, entries) {
            found.push(Finding {
                code: "xbrl.dimension-invalid",
                level: Level::Error,
                message: format!(
                    "The fact {} is not dimensionally valid in the context {}: {why} (xbrldie:PrimaryItemDimensionallyInvalidError).",
                    fact.name, context.id
                ),
                span: fact.span.clone(),
            });
        }
    }
    found
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn arcroles_are_read_from_their_uris_and_others_are_not_dimensional() {
        assert_eq!(Arcrole::from_uri("http://xbrl.org/int/dim/arcrole/all"), Some(Arcrole::All));
        assert_eq!(Arcrole::from_uri(" http://xbrl.org/int/dim/arcrole/notAll "), Some(Arcrole::NotAll));
        assert_eq!(Arcrole::from_uri("http://xbrl.org/int/dim/arcrole/dimension-default"), Some(Arcrole::DimensionDefault));
        assert_eq!(Arcrole::from_uri("http://www.xbrl.org/2003/arcrole/general-special"), None);
        assert_eq!(Arcrole::from_uri("http://xbrl.org/int/dim/arcrole/"), None);
        assert_eq!(Arcrole::from_uri(""), None);
        for a in
            [Arcrole::All, Arcrole::NotAll, Arcrole::HypercubeDimension, Arcrole::DimensionDomain, Arcrole::DomainMember, Arcrole::DimensionDefault]
        {
            assert_eq!(Arcrole::from_uri(&format!("{ARCROLE}{}", a.name())), Some(a));
        }
    }

    fn arc(arcrole: Arcrole, role: &str, from: &str, to: &str) -> DefArc {
        DefArc { role: role.into(), arcrole, from: from.into(), to: to.into(), context_element: None, closed: None, target_role: None, usable: None }
    }

    #[test]
    fn domains_and_descendants_end_on_cycles_and_unusable_members_are_left_out() {
        let arcs = vec![
            arc(Arcrole::DimensionDomain, "r", "D", "Dom"),
            arc(Arcrole::DomainMember, "r", "Dom", "A"),
            arc(Arcrole::DomainMember, "r", "A", "B"),
            arc(Arcrole::DomainMember, "r", "B", "A"),
            DefArc { usable: Some("false".into()), ..arc(Arcrole::DomainMember, "r", "Dom", "C") },
            arc(Arcrole::DomainMember, "r", "C", "C2"),
        ];
        let mut index: Index = HashMap::new();
        for a in &arcs {
            index.entry((a.arcrole, a.role.as_str(), a.from.as_str())).or_default().push(a);
        }
        let (members, cut) = domain(&index, "D", "r");
        assert!(!cut);
        assert_eq!(members.into_iter().collect::<Vec<_>>(), ["A", "B", "C2", "Dom"], "C is unusable, its children are not");
        let (heirs, cut) = descendants(&index, "A", "r");
        assert!(!cut);
        assert_eq!(heirs, ["B"], "a cycle back to A ends");
    }
}
