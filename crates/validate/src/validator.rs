//! A push validator: it is told the elements, the text and the rest of a document in order and checks them against a grammar made from a
//! DTD (the validity constraints of XML 1.0 for elements, content and attributes). It knows nothing about trees or files; a driver
//! feeds it.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use xmlcraft_dtd::{ValueReader, check_value};
use xmlcraft_grammar::{AttributeDefault, AttributeType, Content, ElementDecl, EntityKind, Grammar, Name, State};
use xmlcraft_syntax::{Code, Diagnostic, is_xml_space};

/// How many names a message lists before it says there are more.
const LISTED: usize = 6;

/// An attribute of a start tag, as it was written.
pub struct Attribute<'a> {
    pub name: &'a str,
    /// The text between the quotes.
    pub value: &'a str,
    /// The whole attribute, for the problems found in it.
    pub range: Range<usize>,
}

struct Frame<'g> {
    name: String,
    decl: Option<&'g ElementDecl>,
    state: Option<State>,
    /// A child that does not fit was reported: what comes after is not checked against the model (one mistake, one problem).
    content_broken: bool,
    /// Something was reported about the text or the content of an EMPTY element.
    text_reported: bool,
    /// Part of the content is not known (an entity that could not be read), so the content cannot be called incomplete.
    opaque: bool,
}

/// Checks a document, event by event.
pub struct Validator<'g> {
    grammar: &'g Grammar,
    reader: ValueReader<'g>,
    standalone: bool,
    stack: Vec<Frame<'g>>,
    ids: HashMap<String, Range<usize>>,
    idrefs: Vec<(String, Range<usize>)>,
    root_seen: bool,
    diagnostics: Vec<Diagnostic>,
}

fn list(names: &[String]) -> String {
    let shown: Vec<&str> = names.iter().take(LISTED).map(String::as_str).collect();
    let more = if names.len() > LISTED { ", …" } else { "" };
    format!("{}{more}", shown.join(", "))
}

impl<'g> Validator<'g> {
    /// A validator for documents with this grammar. `standalone` is the document's `standalone="yes"`.
    pub fn new(grammar: &'g Grammar, standalone: bool) -> Self {
        Self {
            grammar,
            reader: ValueReader::new(grammar),
            standalone,
            stack: Vec::new(),
            ids: HashMap::new(),
            idrefs: Vec::new(),
            root_seen: false,
            diagnostics: Vec::new(),
        }
    }

    fn report(&mut self, code: Code, range: Range<usize>, args: &[&str]) {
        self.diagnostics.push(Diagnostic::new(code, range, args.iter().map(|a| (*a).to_owned()).collect()));
    }

    /// Whether the element being read is declared with this kind of content.
    fn content_of_top(&self) -> Option<&'g Content> {
        self.stack.last().and_then(|f| f.decl).map(|d| &d.content)
    }

    /// A start tag. `name_range` is where the element name is, `attributes` are those of the tag.
    pub fn start_element(&mut self, name: &str, name_range: Range<usize>, attributes: &[Attribute<'_>]) {
        let qname = Name::local(name);
        let decl = self.grammar.element(&qname);
        // An element that is not declared is said to be so; that it is not welcome in its parent is not said again.
        self.check_as_child(&qname, name, &name_range, decl.is_some() || self.grammar.incomplete);
        if self.stack.is_empty() {
            if !self.root_seen
                && let Some(root) = &self.grammar.root
                && *root != qname
            {
                self.report(Code::VcRootElement, name_range.clone(), &[name, &root.local]);
            }
            self.root_seen = true;
        }
        if decl.is_none() && !self.grammar.incomplete {
            self.report(Code::VcElementNotDeclared, name_range.clone(), &[name]);
        }
        let state = match decl.map(|d| &d.content) {
            Some(Content::Elements(content)) => Some(content.automaton.start()),
            _ => None,
        };
        self.stack.push(Frame { name: name.to_owned(), decl, state, content_broken: false, text_reported: false, opaque: false });
        if let Some(decl) = decl {
            self.attributes(decl, name, &name_range, attributes);
        }
    }

    /// The element `name` is a child of the element being read: does the content model of that one allow it here?
    fn check_as_child(&mut self, qname: &Name, name: &str, range: &Range<usize>, say: bool) {
        let Some(top) = self.stack.last_mut() else { return };
        let Some(decl) = top.decl else { return };
        let parent = top.name.clone();
        match &decl.content {
            Content::Empty => {
                if !top.text_reported {
                    top.text_reported = true;
                    self.report(Code::VcEmptyHasContent, range.clone(), &[&parent]);
                }
            }
            Content::Any => {}
            Content::Mixed(names) => {
                if !names.contains(qname) && !top.content_broken {
                    top.content_broken = true;
                    if !say {
                        return;
                    }
                    let allowed: Vec<String> = names.iter().map(|n| n.local.clone()).collect();
                    let detail = if allowed.is_empty() {
                        format!("'{parent}' has text only")
                    } else {
                        format!("'{parent}' has text and the elements {}", list(&allowed))
                    };
                    self.report(Code::VcChildNotAllowed, range.clone(), &[name, &detail]);
                }
            }
            Content::Elements(content) => {
                if top.content_broken || top.opaque {
                    return;
                }
                let Some(state) = top.state.take() else { return };
                match content.automaton.step(&state, qname) {
                    Some(step) => top.state = Some(step.state),
                    None => {
                        top.content_broken = true;
                        if !say {
                            top.state = Some(state);
                            return;
                        }
                        let expected: Vec<String> = content.automaton.expected(&state).iter().map(|e| e.term.to_string()).collect();
                        let detail = if expected.is_empty() {
                            format!("'{parent}' cannot have more elements")
                        } else {
                            format!("'{parent}' expects {}", list(&expected))
                        };
                        top.state = Some(state);
                        self.report(Code::VcChildNotAllowed, range.clone(), &[name, &detail]);
                    }
                }
            }
        }
    }

    fn attributes(&mut self, decl: &'g ElementDecl, element: &str, name_range: &Range<usize>, attributes: &[Attribute<'_>]) {
        let mut given: HashSet<&str> = HashSet::new();
        for attribute in attributes {
            given.insert(attribute.name);
            let Some(attr_decl) = decl.attribute(&Name::local(attribute.name)) else {
                if !self.grammar.incomplete {
                    self.report(Code::VcAttributeNotDeclared, attribute.range.clone(), &[element, attribute.name]);
                }
                continue;
            };
            let value = match self.reader.read(attribute.value, &attr_decl.ty) {
                Ok(value) => value,
                Err(problem) => {
                    self.report(Code::VcAttributeValue, attribute.range.clone(), &[attribute.name, &problem.to_string()]);
                    continue;
                }
            };
            if let Err(why) = check_value(&attr_decl.ty, &value) {
                self.report(Code::VcAttributeValue, attribute.range.clone(), &[attribute.name, &why]);
                continue;
            }
            // Namespaces in XML: the names that these types hold are names without a colon.
            if matches!(
                attr_decl.ty,
                AttributeType::Id | AttributeType::Idref | AttributeType::Idrefs | AttributeType::Entity | AttributeType::Entities
            ) && value.contains(':')
            {
                self.report(Code::VcAttributeValue, attribute.range.clone(), &[attribute.name, "a name in a namespace-aware document has no colon"]);
                continue;
            }
            self.standalone_attribute(attr_decl.origin.source, attribute, &value, element);
            self.by_type(&attr_decl.ty, &value, attribute);
            if let AttributeDefault::Fixed(fixed) = &attr_decl.default {
                let wanted = self.reader.read(fixed, &attr_decl.ty).unwrap_or_else(|_| fixed.clone());
                if value != wanted {
                    self.report(Code::VcFixedAttribute, attribute.range.clone(), &[attribute.name, &wanted]);
                }
            }
        }
        for attr_decl in &decl.attributes {
            if given.contains(attr_decl.name.local.as_str()) {
                continue;
            }
            match &attr_decl.default {
                AttributeDefault::Required => self.report(Code::VcRequiredAttribute, name_range.clone(), &[element, &attr_decl.name.local]),
                AttributeDefault::Default(_) | AttributeDefault::Fixed(_) if self.standalone && attr_decl.origin.source != 0 => {
                    let what = format!(
                        "the attribute '{}' of '{element}' gets a default value from a declaration outside the document",
                        attr_decl.name.local
                    );
                    self.report(Code::VcStandalone, name_range.clone(), &[&what]);
                }
                _ => {}
            }
        }
    }

    /// A tokenized type declared outside the document normalises a value that the document alone would leave as it is.
    fn standalone_attribute(&mut self, source: usize, attribute: &Attribute<'_>, value: &str, element: &str) {
        if !self.standalone || source == 0 {
            return;
        }
        let as_cdata = self.reader.read(attribute.value, &AttributeType::Cdata).unwrap_or_default();
        if as_cdata != value {
            let what = format!(
                "the value of the attribute '{}' of '{element}' is changed by a declaration of its type outside the document",
                attribute.name
            );
            self.report(Code::VcStandalone, attribute.range.clone(), &[&what]);
        }
    }

    /// What the type of an attribute asks of its (legal, normalised) value: IDs are unique, IDREFs are looked up at the end, ENTITYs
    /// name unparsed entities.
    fn by_type(&mut self, ty: &AttributeType, value: &str, attribute: &Attribute<'_>) {
        match ty {
            AttributeType::Id => {
                if self.ids.contains_key(value) {
                    self.report(Code::VcDuplicateId, attribute.range.clone(), &[value]);
                } else {
                    self.ids.insert(value.to_owned(), attribute.range.clone());
                }
            }
            AttributeType::Idref => self.idrefs.push((value.to_owned(), attribute.range.clone())),
            AttributeType::Idrefs => self.idrefs.extend(value.split(' ').map(|v| (v.to_owned(), attribute.range.clone()))),
            AttributeType::Entity => self.unparsed_entity(value, attribute),
            AttributeType::Entities => value.split(' ').for_each(|v| self.unparsed_entity(v, attribute)),
            _ => {}
        }
    }

    fn unparsed_entity(&mut self, name: &str, attribute: &Attribute<'_>) {
        let unparsed = matches!(self.grammar.entity(name).map(|e| &e.kind), Some(EntityKind::Unparsed { .. }));
        if !unparsed && !self.grammar.incomplete {
            self.report(Code::VcEntityAttribute, attribute.range.clone(), &[name]);
        }
    }

    /// Character data that a character reference stands for. A reference is never white space in the sense of element content, even a
    /// reference to a space.
    pub fn character_reference(&mut self, range: Range<usize>) {
        self.text(false, range);
    }

    /// A reference to an entity that is not declared. A validity constraint, when the DTD has parts that make that not a well-formedness
    /// error (a parameter-entity reference in the internal subset).
    pub fn undeclared_entity(&mut self, name: &str, range: Range<usize>) {
        self.other_content(range.clone());
        if !self.grammar.incomplete {
            self.report(Code::VcEntityNotDeclared, range, &[name]);
        }
    }

    /// Character data. In an element with element content only white space is allowed.
    pub fn characters(&mut self, text: &str, range: Range<usize>) {
        if text.is_empty() {
            return;
        }
        let blank = text.chars().all(is_xml_space);
        self.text(blank, range);
    }

    /// A CDATA section, which is character data whatever is in it.
    pub fn cdata(&mut self, range: Range<usize>) {
        self.text(false, range);
    }

    fn text(&mut self, blank: bool, range: Range<usize>) {
        let Some(top) = self.stack.last_mut() else { return };
        let Some(decl) = top.decl else { return };
        let name = top.name.clone();
        match &decl.content {
            Content::Empty => {
                if !top.text_reported {
                    top.text_reported = true;
                    self.report(Code::VcEmptyHasContent, range, &[&name]);
                }
            }
            Content::Elements(_) => {
                if !blank {
                    if !top.text_reported {
                        top.text_reported = true;
                        self.report(Code::VcTextNotAllowed, range, &[&name]);
                    }
                } else if self.standalone && decl.origin.source != 0 && !top.text_reported {
                    top.text_reported = true;
                    let what = format!("the element '{name}' has white space in its element content, declared outside the document");
                    self.report(Code::VcStandalone, range, &[&what]);
                }
            }
            Content::Any | Content::Mixed(_) => {}
        }
    }

    /// A comment, a processing instruction or a reference: content, though neither text nor an element.
    pub fn other_content(&mut self, range: Range<usize>) {
        if let Some(Content::Empty) = self.content_of_top()
            && let Some(top) = self.stack.last_mut()
            && !top.text_reported
        {
            top.text_reported = true;
            let name = top.name.clone();
            self.report(Code::VcEmptyHasContent, range, &[&name]);
        }
    }

    /// Content that cannot be read (an external entity): the element cannot be called incomplete for it, nor its content wrong.
    pub fn unknown_content(&mut self, range: Range<usize>) {
        self.other_content(range);
        if let Some(top) = self.stack.last_mut() {
            top.opaque = true;
        }
    }

    /// An end tag.
    pub fn end_element(&mut self, range: Range<usize>) {
        let Some(frame) = self.stack.pop() else { return };
        let Some(decl) = frame.decl else { return };
        if let (Content::Elements(content), Some(state)) = (&decl.content, &frame.state)
            && !frame.content_broken
            && !frame.opaque
            && !content.automaton.is_accepting(state)
        {
            let needed: Vec<String> = content.automaton.shortest_completion(state).unwrap_or_default().iter().map(|s| s.term.to_string()).collect();
            let detail =
                if needed.is_empty() { "its content does not match its declaration".to_owned() } else { format!("it still needs {}", list(&needed)) };
            self.report(Code::VcContentIncomplete, range, &[&frame.name, &detail]);
        }
    }

    /// The end of the document: every IDREF must name an ID. Gives the problems found.
    pub fn finish(mut self) -> Vec<Diagnostic> {
        let idrefs = std::mem::take(&mut self.idrefs);
        for (value, range) in idrefs {
            if !self.ids.contains_key(&value) {
                self.report(Code::VcIdrefNotFound, range, &[&value]);
            }
        }
        self.diagnostics
    }
}
