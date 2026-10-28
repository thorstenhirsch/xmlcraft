//! The rules about references to general entities that need the DTD (XML 1.0 §4.1 and §4.3.2): the entity is declared, is parsed, is not
//! external inside an attribute value, does not refer to itself, has no `<` in an attribute value, and its replacement text is
//! well-formed where it is used.
//!
//! Nothing is expanded. Every entity is analysed once for each place it can be used (in text, in an attribute value) and the verdict is
//! remembered, so the work is the size of the declarations and a document that refers to an entity a million times costs a million
//! look-ups, whatever the entities contain.

use std::collections::HashMap;
use std::ops::Range;

use xmlcraft_syntax::{Code, Diagnostic, Severity, SyntaxKind, SyntaxNode, is_name_char, is_name_start_char, parse};

use crate::model::{Default, Dtd, EntityDecl, EntityValue};
use crate::parse::{DtdDiagnostic, LoadRequest, MAX_DIAGNOSTICS, MAX_LOADED_BYTES, MAX_LOADS, SubsetLoader, char_reference, split_text_decl};

/// How deep entities may refer to entities, when each is analysed in the middle of the one that refers to it.
const MAX_NESTING: usize = 256;

/// Where a reference is, as far as the rules go.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Spot {
    /// In text.
    Content,
    /// In an attribute value.
    Attribute,
}

impl Spot {
    fn slot(self) -> usize {
        self as usize
    }
}

/// What is wrong with using an entity somewhere: the code and arguments of the diagnostic.
#[derive(Clone, Debug)]
struct Problem {
    code: Code,
    args: Vec<String>,
}

impl Problem {
    fn new(code: Code, args: &[&str]) -> Self {
        Self { code, args: args.iter().map(|&a| a.to_owned()).collect() }
    }
}

/// What is known of an entity used somewhere.
#[derive(Clone)]
enum State {
    /// Being analysed: a reference met again is a cycle.
    Open,
    Done(Option<Problem>),
}

/// Where a reference is.
#[derive(Clone, Copy)]
struct Site {
    /// The reference is in the document or in its internal subset: the rule that the entity be declared holds. In the external subset or
    /// in a parameter entity the name may be declared where it was not read.
    declared_matters: bool,
    /// For a reference in the default value of an attribute: where that is (source, offset). The entity must be declared before.
    before: Option<(usize, usize)>,
}

/// What the XML declaration says that the rules need.
struct Facts {
    standalone: bool,
    /// The document is XML 1.0 (a document without a declaration is).
    version_1_0: bool,
}

fn facts_of(root: &SyntaxNode) -> Facts {
    let (mut version, mut standalone, mut last): (Option<String>, bool, Option<String>) = (None, false, None);
    if let Some(declaration) = root.children().find(|n| n.kind() == SyntaxKind::XmlDecl) {
        for token in declaration.children_with_tokens().filter_map(|c| c.into_token()) {
            match token.kind() {
                SyntaxKind::Name => last = Some(token.text().to_owned()),
                SyntaxKind::LiteralText => match last.as_deref() {
                    Some("version") => version = Some(token.text().to_owned()),
                    Some("standalone") => standalone = token.text() == "yes",
                    _ => {}
                },
                _ => {}
            }
        }
    }
    Facts { standalone, version_1_0: version.as_deref().is_none_or(|v| v == "1.0") }
}

/// Whether the XML declaration of the document says `standalone="yes"`.
pub fn is_standalone(root: &SyntaxNode) -> bool {
    facts_of(root).standalone
}

/// Whether the text is a name.
fn is_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(is_name_start_char) && chars.all(is_name_char)
}

/// What is wrong with an attribute value, or with the replacement text of an entity used in one.
enum Scan {
    /// A `<`.
    LessThan,
    /// A `&` that does not start a reference, or a character reference to a character XML does not allow.
    Bad(String),
}

/// The names of the general entities an attribute value refers to, or the first thing wrong with it. Character references are checked,
/// and not followed.
fn attribute_refs(text: &str) -> Result<Vec<&str>, Scan> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(['&', '<']) {
        let Some(after) = rest.get(at..).unwrap_or_default().strip_prefix('&') else { return Err(Scan::LessThan) };
        let Some(end) = after.find(';') else { return Err(Scan::Bad("'&' must start a reference that ends with ';'".to_owned())) };
        let body = after.get(..end).unwrap_or_default();
        match body.strip_prefix('#') {
            Some(number) if char_reference(number).is_none() => return Err(Scan::Bad(format!("'&{body};' is not a character XML allows"))),
            Some(_) => {}
            None if is_name(body) => names.push(body),
            None => return Err(Scan::Bad("'&' must start a reference".to_owned())),
        }
        rest = after.get(end + 1..).unwrap_or_default();
    }
    Ok(names)
}

fn range_of(node: &SyntaxNode) -> Range<usize> {
    usize::from(node.text_range().start())..usize::from(node.text_range().end())
}

struct Checker<'a> {
    entities: HashMap<&'a str, &'a EntityDecl>,
    /// The rule that an entity be declared holds: there is no other place it might be declared.
    complete: bool,
    standalone: bool,
    version_1_0: bool,
    /// Where the document is (for relative system identifiers).
    location: &'a str,
    loader: &'a mut dyn SubsetLoader,
    memo: HashMap<String, [Option<State>; 2]>,
    depth: usize,
    loads: usize,
    loaded_bytes: usize,
    /// External entities that could not be read since the last report: (entity, why).
    not_read: Vec<(String, String)>,
}

impl<'a> Checker<'a> {
    fn new(dtd: &'a Dtd, facts: &Facts, location: &'a str, loader: &'a mut dyn SubsetLoader) -> Self {
        let mut entities: HashMap<&str, &EntityDecl> = HashMap::new();
        for entity in &dtd.general_entities {
            // The first declaration is the one that counts.
            entities.entry(entity.name.as_str()).or_insert(entity);
        }
        // "Entity Declared" is a well-formedness constraint where there is nothing outside the document to declare the entity, and always
        // for a document that says it is standalone.
        let complete = facts.standalone || (!dtd.has_external_parts && !dtd.has_parameter_entity_references);
        Self {
            entities,
            complete,
            standalone: facts.standalone,
            version_1_0: facts.version_1_0,
            location,
            loader,
            memo: HashMap::new(),
            depth: 0,
            loads: 0,
            loaded_bytes: 0,
            not_read: Vec::new(),
        }
    }

    /// What is wrong with a reference to the entity `name` in `spot`, or `None`.
    fn use_entity(&mut self, name: &str, spot: Spot, site: Site) -> Option<Problem> {
        if matches!(name, "lt" | "gt" | "amp" | "quot" | "apos") {
            return None;
        }
        let Some(&decl) = self.entities.get(name) else {
            return (self.complete && site.declared_matters).then(|| Problem::new(Code::UndeclaredEntity, &[name]));
        };
        if site.declared_matters {
            let declared_at = (decl.place.source, decl.place.range.start);
            // In a default value the declaration must come first.
            if self.complete && site.before.is_some_and(|here| declared_at >= here) {
                return Some(Problem::new(Code::UndeclaredEntity, &[name]));
            }
            if self.standalone && decl.place.source != 0 {
                return Some(Problem::new(Code::EntityExternalStandalone, &[name]));
            }
        }
        if decl.notation.is_some() {
            return Some(Problem::new(Code::EntityUnparsed, &[name]));
        }
        match self.memo.get(name).and_then(|states| states[spot.slot()].as_ref()) {
            Some(State::Done(problem)) => return problem.clone(),
            Some(State::Open) => return Some(Problem::new(Code::DtdRecursiveEntity, &[name])),
            None => {}
        }
        if self.depth >= MAX_NESTING {
            return Some(Problem::new(Code::DtdLimit, &[&format!("entities refer to entities more than {MAX_NESTING} deep")]));
        }
        self.memo.entry(name.to_owned()).or_default()[spot.slot()] = Some(State::Open);
        self.depth += 1;
        let problem = self.analyze(name, decl, spot);
        self.depth -= 1;
        self.memo.entry(name.to_owned()).or_default()[spot.slot()] = Some(State::Done(problem.clone()));
        problem
    }

    /// What is wrong with the replacement text of `decl` used in `spot`.
    fn analyze(&mut self, name: &str, decl: &'a EntityDecl, spot: Spot) -> Option<Problem> {
        match (&decl.value, spot) {
            (EntityValue::Internal(text), Spot::Attribute) => match attribute_refs(text) {
                Ok(names) => self.use_all(names.iter().map(|n| (*n, Spot::Attribute)), decl),
                Err(Scan::LessThan) => Some(Problem::new(Code::EntityLtInAttribute, &[name])),
                Err(Scan::Bad(why)) => Some(Problem::new(Code::EntityNotWellFormed, &[name, &why])),
            },
            (EntityValue::Internal(text), Spot::Content) => self.analyze_content(name, text, decl),
            (EntityValue::External(_), Spot::Attribute) => Some(Problem::new(Code::EntityExternalInAttribute, &[name])),
            (EntityValue::External(id), Spot::Content) => {
                if self.loads >= MAX_LOADS {
                    return Some(Problem::new(Code::DtdLimit, &[&format!("more than {MAX_LOADS} external entities")]));
                }
                self.loads += 1;
                let base = decl.location.as_deref().unwrap_or(self.location);
                let request =
                    LoadRequest { kind: "external entity", public: id.public.as_deref(), system: id.system.as_deref().unwrap_or_default(), base };
                let loaded = match self.loader.load(&request) {
                    Ok(loaded) => loaded,
                    Err(why) => {
                        // What is not read cannot be checked.
                        self.not_read.push((name.to_owned(), why));
                        return None;
                    }
                };
                self.loaded_bytes = self.loaded_bytes.saturating_add(loaded.text.len());
                if self.loaded_bytes > MAX_LOADED_BYTES {
                    return Some(Problem::new(Code::DtdLimit, &[&format!("more than {MAX_LOADED_BYTES} bytes of external text")]));
                }
                let split = split_text_decl(&loaded.text);
                if let Some(problem) = split.problem {
                    return Some(Problem::new(Code::EntityNotWellFormed, &[name, &format!("its text declaration is malformed: {problem}")]));
                }
                if self.version_1_0 && split.version.as_deref().is_some_and(|v| v != "1.0") {
                    return Some(Problem::new(Code::EntityNotWellFormed, &[name, "it is XML 1.1 and the document is XML 1.0"]));
                }
                self.analyze_content(name, loaded.text.get(split.content..).unwrap_or_default(), decl)
            }
        }
    }

    /// The text as the content of an element: well-formed, and the entities it refers to in turn fit where they are.
    fn analyze_content(&mut self, name: &str, text: &str, decl: &'a EntityDecl) -> Option<Problem> {
        // The text is parsed as the content of an element, so that a start tag in it must be closed in it and an end tag in it closes
        // nothing. (An end tag that closes the wrapper leaves a second root or a stray end tag behind, which is an error too.)
        let parsed = parse(&format!("<_>{text}</_>"));
        // What is wrong with the text, not with its names (which belong to the document that uses it) or where it is declared.
        let wrong = parsed
            .diagnostics()
            .iter()
            .find(|d| d.severity() == Severity::Error && d.code.key().starts_with("xml.") && d.code != Code::UndeclaredEntity);
        if let Some(wrong) = wrong {
            return Some(Problem::new(Code::EntityNotWellFormed, &[name, &wrong.message()]));
        }
        let tree = parsed.syntax();
        let refs: Vec<(String, Spot)> = tree
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .filter(|token| token.kind() == SyntaxKind::EntityRef)
            .map(|token| {
                let spot = if token.parent().is_some_and(|p| p.kind() == SyntaxKind::AttrValue) { Spot::Attribute } else { Spot::Content };
                (token.text().trim_start_matches('&').trim_end_matches(';').to_owned(), spot)
            })
            .collect();
        drop(tree);
        self.use_all(refs.iter().map(|(n, p)| (n.as_str(), *p)), decl)
    }

    /// The first thing wrong with the references in the replacement text of `decl`.
    fn use_all<'n>(&mut self, refs: impl Iterator<Item = (&'n str, Spot)>, decl: &EntityDecl) -> Option<Problem> {
        // A name not declared in a text that is declared outside the document may be declared in a place that was not read.
        let site = Site { declared_matters: decl.place.source == 0, before: None };
        refs.into_iter().find_map(|(name, spot)| self.use_entity(name, spot, site))
    }

    /// Say about a reference what was found: its problem, and the external entities it made the checker try to read.
    fn report(&mut self, out: &mut Vec<DtdDiagnostic>, source: usize, range: Range<usize>, problem: Option<Problem>) {
        if out.len() >= MAX_DIAGNOSTICS {
            self.not_read.clear();
            return;
        }
        if let Some(problem) = problem {
            out.push(DtdDiagnostic { source, diagnostic: Diagnostic::new(problem.code, range.clone(), problem.args) });
        }
        for (entity, why) in self.not_read.drain(..) {
            out.push(DtdDiagnostic {
                source,
                diagnostic: Diagnostic::new(Code::DtdExternalNotRead, range.clone(), vec![format!("entity '{entity}'"), why]),
            });
        }
        if out.len() >= MAX_DIAGNOSTICS {
            out.push(DtdDiagnostic { source, diagnostic: Diagnostic::new(Code::TooManyDiagnostics, range.end..range.end, Vec::new()) });
        }
    }
}

/// The problems with the references to general entities in a document that has a DOCTYPE, and in the default values of the attributes its
/// DTD declares. `dtd` is what was read of the DTD, `location` is where the document is, and `loader` gives the external entities that
/// the document refers to in text (what is not read is not checked, and said so with a warning).
pub fn check_references(root: &SyntaxNode, dtd: &Dtd, location: &str, loader: &mut dyn SubsetLoader) -> Vec<DtdDiagnostic> {
    let facts = facts_of(root);
    let mut checker = Checker::new(dtd, &facts, location, loader);
    let mut out = Vec::new();
    let doctype = root.children().find(|n| n.kind() == SyntaxKind::Doctype).map(|n| range_of(&n));
    for token in root.descendants_with_tokens().filter_map(|element| element.into_token()) {
        if token.kind() != SyntaxKind::EntityRef {
            continue;
        }
        let range = usize::from(token.text_range().start())..usize::from(token.text_range().end());
        // Inside the DOCTYPE the references are in declarations, which are not references to be expanded here.
        if doctype.as_ref().is_some_and(|d| d.contains(&range.start)) {
            continue;
        }
        let spot = if token.parent().is_some_and(|p| p.kind() == SyntaxKind::AttrValue) { Spot::Attribute } else { Spot::Content };
        let name = token.text().trim_start_matches('&').trim_end_matches(';');
        let problem = checker.use_entity(name, spot, Site { declared_matters: true, before: None });
        checker.report(&mut out, 0, range, problem);
        if out.len() > MAX_DIAGNOSTICS {
            return out;
        }
    }
    for list in &dtd.attlists {
        for def in &list.defs {
            let (Default::Value(value) | Default::Fixed(value)) = &def.default else { continue };
            let site = Site { declared_matters: def.place.source == 0, before: Some((def.place.source, def.place.range.start)) };
            let problem = match attribute_refs(value) {
                Ok(names) => names.into_iter().find_map(|name| checker.use_entity(name, Spot::Attribute, site)),
                // A `<` written in a default value is the reader's to report.
                Err(Scan::LessThan) => None,
                Err(Scan::Bad(why)) => Some(Problem::new(Code::DtdBadAttlist, &[&why])),
            };
            checker.report(&mut out, def.place.source, def.place.range.clone(), problem);
            if out.len() > MAX_DIAGNOSTICS {
                return out;
            }
        }
    }
    out
}
