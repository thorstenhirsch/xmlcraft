//! Reading the declarations: a recursive-descent reader over a stack of texts (the subset being read, and the replacement text of the
//! parameter entities that are being expanded in it).

use std::collections::HashMap;
use std::ops::Range;

use xmlcraft_syntax::{Code, Diagnostic, is_name_char, is_name_start_char, is_xml_char, is_xml_space};

use crate::model::{
    AttDef, AttType, AttlistDecl, ContentSpec, Default, Dtd, ElementDecl, EntityDecl, EntityValue, ExternalId, NotationDecl, Occurs, Particle,
    ParticleKind, Place,
};

/// How deep parameter entities may be nested in each other.
pub const MAX_PE_DEPTH: usize = 40;
/// The most bytes the expansion of parameter entities may produce, in all.
pub const MAX_EXPANDED_BYTES: usize = 10_000_000;
/// The most external texts one DTD may load.
pub const MAX_LOADS: usize = 256;
/// The most bytes of external text one DTD may load.
pub const MAX_LOADED_BYTES: usize = 64 * 1024 * 1024;
/// The most declarations read.
pub const MAX_DECLARATIONS: usize = 1_000_000;
/// How deep the groups of a content model may nest.
pub const MAX_GROUP_DEPTH: usize = 256;
/// The most problems listed.
pub const MAX_DIAGNOSTICS: usize = 1000;

/// What is asked of a [`SubsetLoader`].
pub struct LoadRequest<'a> {
    /// `"external subset"` or `"parameter entity"`.
    pub kind: &'static str,
    pub public: Option<&'a str>,
    /// The system identifier as written; it is relative to `base` when it is a relative reference.
    pub system: &'a str,
    /// The location of the text that has the reference (a URI or a path; empty when there is none).
    pub base: &'a str,
}

/// An external text that was read.
pub struct Loaded {
    /// The text, already decoded.
    pub text: String,
    /// Where it was read from (what relative references inside it are resolved against).
    pub location: String,
}

/// Where the external parts of a DTD come from. The reader never reads a file or a URL itself.
pub trait SubsetLoader {
    /// The text the request names, or the reason there is none.
    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String>;
}

/// A loader that has nothing: the external parts are not read.
pub struct NoLoader;

impl SubsetLoader for NoLoader {
    fn load(&mut self, _: &LoadRequest<'_>) -> Result<Loaded, String> {
        Err("external parts are not read here".to_owned())
    }
}

/// A problem found in the DTD, and in which text: `source` is 0 for the document (its internal subset, with offsets of the document) and
/// otherwise an index into [`Parsed::sources`] (external texts, with offsets of that text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DtdDiagnostic {
    pub source: usize,
    pub diagnostic: Diagnostic,
}

/// What reading a DTD gave.
#[derive(Clone, Debug)]
pub struct Parsed {
    pub dtd: Dtd,
    pub diagnostics: Vec<DtdDiagnostic>,
    /// The locations of the external texts that were read; `sources[0]` is the document (empty here), `sources[n]` is source `n`.
    pub sources: Vec<String>,
}

/// The external subset a DOCTYPE names.
pub struct ExternalRef<'a> {
    pub public: Option<&'a str>,
    pub system: Option<&'a str>,
}

/// What a DOCTYPE says about its DTD.
pub struct DoctypeInput<'a> {
    pub external: Option<ExternalRef<'a>>,
    /// The text between `[` and `]`, and the offset in the document where it begins.
    pub internal: Option<(&'a str, usize)>,
    /// Where the document is (for relative system identifiers).
    pub location: &'a str,
}

/// Read the DTD of a document: its internal subset, then the external subset (declarations of the internal subset come first, so they
/// win over the external ones for the names that are declared twice).
pub fn parse_doctype(input: &DoctypeInput<'_>, loader: &mut dyn SubsetLoader) -> Parsed {
    let mut parser = Parser::new(loader, input.location);
    // With an external subset, a name that the internal subset does not declare may be declared there (XML 1.0 §4.1).
    parser.dtd.has_external_parts = input.external.is_some();
    if let Some((text, offset)) = input.internal {
        parser.run_internal(text, offset);
    }
    if let Some(external) = &input.external {
        match external.system {
            Some(system) => parser.run_external_subset(external.public, system, input.location),
            None => parser.dtd.incomplete = true,
        }
    }
    parser.finish()
}

/// Read `text` as an internal subset that starts at `offset` of its document (nothing is loaded).
pub fn parse_internal_subset(text: &str, offset: usize) -> Parsed {
    parse_doctype(&DoctypeInput { external: None, internal: Some((text, offset)), location: "" }, &mut NoLoader)
}

/// Read `text` as an external subset (nothing is loaded).
pub fn parse_external_subset(text: &str) -> Parsed {
    let mut loader = NoLoader;
    let mut parser = Parser::new(&mut loader, "");
    parser.dtd.has_external_parts = true;
    let source = parser.new_source(String::new());
    parser.run_text(text, source);
    parser.finish()
}

// --- the machine ---------------------------------------------------------------------------------------------------------------

/// One text being read.
struct Frame {
    text: String,
    pos: usize,
    /// The text is something that was read (the internal subset, an external subset or parameter entity) and its offsets are its own;
    /// otherwise it is the replacement text of an internal parameter entity, and what is wrong in it is reported where it is referenced.
    real: bool,
    /// The source `pos` belongs to (real) or `origin` does (not real).
    source: usize,
    /// For a real text, where `pos` 0 (after the padding) is in its source: the length of what was taken off the front of it (a text
    /// declaration), or where the subset begins in the document.
    base: usize,
    /// White space put at each end of the text (a reference to a parameter entity stands for its text with a space at each end), which
    /// is not in the source.
    pad: usize,
    /// For a replacement text, where the reference is.
    origin: usize,
    /// The reading is in the internal subset (a reference to a parameter entity cannot be inside a declaration here).
    internal: bool,
    /// The parameter entity this is the text of, to see that it does not contain itself.
    name: Option<String>,
}

impl Frame {
    /// Where the next character is in its source (a text that was read), without the padding.
    fn offset(&self) -> usize {
        let inner = self.text.len().saturating_sub(2 * self.pad);
        self.base.saturating_add(self.pos.saturating_sub(self.pad).min(inner))
    }
}

struct Parser<'a> {
    frames: Vec<Frame>,
    dtd: Dtd,
    diags: Vec<DtdDiagnostic>,
    sources: Vec<String>,
    loader: &'a mut dyn SubsetLoader,
    /// The location the next relative reference is relative to.
    base: String,
    loads: usize,
    loaded_bytes: usize,
    expanded: usize,
    declarations: usize,
    /// Inside a declaration (so that a parameter-entity reference is not allowed in the internal subset).
    in_decl: bool,
    /// The text of the external parameter entities that were loaded, by name: (text without its text declaration, source, where in the
    /// source the text begins).
    external_pes: HashMap<String, Option<(String, usize, usize)>>,
    /// Reading has been given up (a limit was passed).
    stopped: bool,
    capped: bool,
}

impl<'a> Parser<'a> {
    fn new(loader: &'a mut dyn SubsetLoader, base: &str) -> Self {
        Self {
            frames: Vec::new(),
            dtd: Dtd::default(),
            diags: Vec::new(),
            sources: vec![String::new()],
            loader,
            base: base.to_owned(),
            loads: 0,
            loaded_bytes: 0,
            expanded: 0,
            declarations: 0,
            in_decl: false,
            external_pes: HashMap::new(),
            stopped: false,
            capped: false,
        }
    }

    fn finish(self) -> Parsed {
        Parsed { dtd: self.dtd, diagnostics: self.diags, sources: self.sources }
    }

    fn new_source(&mut self, location: String) -> usize {
        self.sources.push(location);
        self.sources.len() - 1
    }

    // --- reporting ---

    /// Where reading is now: the source and the offset in it.
    fn here(&self) -> (usize, usize) {
        match self.frames.last() {
            Some(f) if f.real => (f.source, f.offset()),
            Some(f) => (f.source, f.origin),
            None => (0, 0),
        }
    }

    fn report(&mut self, code: Code, args: Vec<String>, from: (usize, usize)) {
        let (source, end) = self.here();
        // A problem is where the construct began, up to where it was noticed (at least a character wide when that is the same place).
        let range: Range<usize> = if from.0 == source && from.1 < end { from.1..end } else { end..end };
        self.push(source, Diagnostic::new(code, range, args));
    }

    /// List a problem in `source`, unless there are too many already: then one note says so, once.
    fn push(&mut self, source: usize, diagnostic: Diagnostic) {
        if self.diags.len() >= MAX_DIAGNOSTICS {
            if !self.capped {
                self.capped = true;
                let at = diagnostic.span.start;
                self.diags.push(DtdDiagnostic { source, diagnostic: Diagnostic::new(Code::TooManyDiagnostics, at..at, Vec::new()) });
            }
            return;
        }
        self.diags.push(DtdDiagnostic { source, diagnostic });
    }

    /// A problem at the current place.
    fn fail(&mut self, code: Code, detail: &str) {
        let here = self.here();
        self.report(code, vec![detail.to_owned()], here);
    }

    // --- the text ---

    /// Take the exhausted replacement texts off the stack: the reading is back in what referred to them.
    fn settle(&mut self) {
        while self.frames.len() > 1 && self.frames.last().is_some_and(|f| f.pos >= f.text.len()) {
            self.frames.pop();
        }
    }

    fn rest(&mut self) -> &str {
        self.settle();
        self.frames.last().and_then(|f| f.text.get(f.pos..)).unwrap_or("")
    }

    fn peek(&mut self) -> Option<char> {
        self.rest().chars().next()
    }

    /// The next character of the text being read, without going back to what referred to it when that text is finished: a literal does
    /// not run on from a replacement text into the text that refers to it.
    fn peek_raw(&self) -> Option<char> {
        self.frames.last().and_then(|f| f.text.get(f.pos..)).and_then(|rest| rest.chars().next())
    }

    fn bump_raw(&mut self) -> Option<char> {
        let c = self.peek_raw()?;
        self.advance(c.len_utf8());
        Some(c)
    }

    fn rest_raw(&self) -> &str {
        self.frames.last().and_then(|f| f.text.get(f.pos..)).unwrap_or("")
    }

    fn peek_second(&mut self) -> Option<char> {
        self.rest().chars().nth(1)
    }

    fn advance(&mut self, bytes: usize) {
        if let Some(f) = self.frames.last_mut() {
            f.pos = (f.pos + bytes).min(f.text.len());
        }
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.advance(c.len_utf8());
        Some(c)
    }

    fn starts_with(&mut self, text: &str) -> bool {
        self.rest().starts_with(text)
    }

    fn eat(&mut self, text: &str) -> bool {
        if self.starts_with(text) {
            self.advance(text.len());
            true
        } else {
            false
        }
    }

    /// A character that is not allowed in a document, at the current place.
    fn check_char(&mut self, c: char) {
        if !is_xml_char(c) {
            // The character has been taken: the problem is the character itself (in a text that was read; a replacement text has no places).
            let (source, end) = self.here();
            let real = self.frames.last().is_some_and(|f| f.real);
            let from = if real { end.saturating_sub(c.len_utf8()) } else { end };
            self.report(Code::InvalidChar, vec![format!("{:04X}", u32::from(c))], (source, from));
        }
    }

    // --- tokens ---

    fn name(&mut self) -> Option<String> {
        let first = self.peek().filter(|&c| is_name_start_char(c))?;
        let mut out = String::from(first);
        self.advance(first.len_utf8());
        while let Some(c) = self.peek().filter(|&c| is_name_char(c)) {
            out.push(c);
            self.advance(c.len_utf8());
        }
        Some(out)
    }

    fn nmtoken(&mut self) -> Option<String> {
        let mut out = String::new();
        while let Some(c) = self.peek().filter(|&c| is_name_char(c)) {
            out.push(c);
            self.advance(c.len_utf8());
        }
        (!out.is_empty()).then_some(out)
    }

    /// A keyword: it must not run on into a longer name.
    fn keyword(&mut self, word: &str) -> bool {
        let rest = self.rest();
        let ends_here = rest.strip_prefix(word).is_some_and(|after| after.chars().next().is_none_or(|c| !is_name_char(c)));
        if ends_here {
            self.advance(word.len());
        }
        ends_here
    }

    /// A `%` that starts a reference to a parameter entity (the name follows at once).
    fn pe_reference_ahead(&mut self) -> bool {
        self.peek() == Some('%') && self.peek_second().is_some_and(is_name_start_char)
    }

    /// White space, and the parameter-entity references between tokens (they stand for their text, with a space at each end). Inside a
    /// declaration of the internal subset such a reference is an error. Returns whether there was any white space or reference.
    fn s(&mut self) -> bool {
        let mut any = false;
        loop {
            match self.peek() {
                Some(c) if is_xml_space(c) => {
                    self.advance(c.len_utf8());
                    any = true;
                }
                Some('%') if self.pe_reference_ahead() => {
                    let internal = self.frames.last().is_some_and(|f| f.internal);
                    let start = self.here();
                    if self.in_decl && internal {
                        let name = self.read_pe_reference().unwrap_or_default();
                        self.report(Code::PeInDeclaration, vec![name], start);
                    } else if !self.expand_reference() {
                        return any;
                    }
                    any = true;
                }
                _ => return any,
            }
            if self.stopped {
                return any;
            }
        }
    }

    fn require_s(&mut self, code: Code, what: &str) -> bool {
        if self.s() {
            true
        } else {
            self.fail(code, &format!("white space is needed {what}"));
            false
        }
    }

    // --- parameter entities ---

    /// Read `%name;` at the current place; the name.
    fn read_pe_reference(&mut self) -> Option<String> {
        let start = self.here();
        self.advance(1);
        let name = self.name()?;
        if !self.eat(";") {
            // Not a reference: there is nothing to expand, and one mistake is one problem.
            self.report(Code::DtdBadPeReference, vec![format!("'%{name}' must end with ';'")], start);
            return None;
        }
        Some(name)
    }

    /// The replacement text of the parameter entity `name`: its text, and when that is an external text the source it is in and where in
    /// the source the text begins.
    fn pe_text(&mut self, name: &str, start: (usize, usize)) -> Option<(String, Option<(usize, usize)>)> {
        let Some(decl) = self.dtd.parameter_entity(name).cloned() else {
            if self.dtd.has_external_parts {
                self.dtd.incomplete = true;
                self.report(Code::DtdUndeclaredPeUnread, vec![name.to_owned()], start);
            } else {
                self.report(Code::DtdUndeclaredPe, vec![name.to_owned()], start);
            }
            return None;
        };
        match &decl.value {
            EntityValue::Internal(text) => Some((text.clone(), None)),
            EntityValue::External(id) => {
                if let Some(known) = self.external_pes.get(name) {
                    return known.clone().map(|(text, source, offset)| (text, Some((source, offset))));
                }
                let base = decl.location.clone().unwrap_or_else(|| self.base.clone());
                let loaded = self.load("parameter entity", id.public.as_deref(), id.system.as_deref().unwrap_or_default(), &base, start);
                let entry = loaded.map(|(text, source)| {
                    let (rest, offset) = strip_text_decl(self, &text, source);
                    (rest, source, offset)
                });
                self.external_pes.insert(name.to_owned(), entry.clone());
                entry.map(|(text, source, offset)| (text, Some((source, offset))))
            }
        }
    }

    /// Load an external text; `None` (with a note) when it cannot be had or a limit is passed.
    fn load(&mut self, kind: &'static str, public: Option<&str>, system: &str, base: &str, start: (usize, usize)) -> Option<(String, usize)> {
        self.dtd.has_external_parts = true;
        if self.loads >= MAX_LOADS {
            self.dtd.incomplete = true;
            self.report(Code::DtdLimit, vec![format!("more than {MAX_LOADS} external texts")], start);
            return None;
        }
        self.loads += 1;
        match self.loader.load(&LoadRequest { kind, public, system, base }) {
            Ok(loaded) => {
                self.loaded_bytes = self.loaded_bytes.saturating_add(loaded.text.len());
                if self.loaded_bytes > MAX_LOADED_BYTES {
                    self.dtd.incomplete = true;
                    self.report(Code::DtdLimit, vec![format!("more than {MAX_LOADED_BYTES} bytes of external text")], start);
                    self.stopped = true;
                    return None;
                }
                let source = self.new_source(loaded.location);
                Some((loaded.text, source))
            }
            Err(reason) => {
                self.dtd.incomplete = true;
                self.report(Code::DtdExternalNotRead, vec![format!("{kind} '{system}'"), reason], start);
                None
            }
        }
    }

    /// `%name;` between tokens or declarations: the entity's text is read here, with a space at each end. Returns whether it was
    /// (a reference that cannot be expanded is skipped).
    fn expand_reference(&mut self) -> bool {
        let start = self.here();
        let internal_context = self.frames.last().is_some_and(|f| f.internal);
        let Some(name) = self.read_pe_reference() else { return false };
        if self.frames.len() > MAX_PE_DEPTH {
            self.report(Code::DtdLimit, vec![format!("parameter entities nested more than {MAX_PE_DEPTH} deep")], start);
            self.stopped = true;
            return false;
        }
        if self.frames.iter().any(|f| f.name.as_deref() == Some(name.as_str())) {
            self.report(Code::DtdRecursiveEntity, vec![format!("%{name};")], start);
            return true;
        }
        let Some((text, source)) = self.pe_text(&name, start) else { return true };
        self.expanded = self.expanded.saturating_add(text.len());
        if self.expanded > MAX_EXPANDED_BYTES {
            self.report(Code::DtdLimit, vec![format!("parameter entities expand to more than {MAX_EXPANDED_BYTES} bytes")], start);
            self.stopped = true;
            return false;
        }
        let internal = internal_context && source.is_none();
        let (here_source, here_offset) = self.here();
        let frame = match source {
            // A text that was read: its own source and offsets.
            Some((source, offset)) => Frame {
                text: format!(" {text} "),
                pos: 0,
                real: true,
                source,
                base: offset,
                pad: 1,
                origin: here_offset,
                internal: false,
                name: Some(name),
            },
            None => Frame {
                text: format!(" {text} "),
                pos: 0,
                real: false,
                source: here_source,
                base: 0,
                pad: 1,
                origin: here_offset,
                internal,
                name: Some(name),
            },
        };
        self.frames.push(frame);
        true
    }

    // --- the subsets ---

    fn run_internal(&mut self, text: &str, offset: usize) {
        self.frames.push(Frame { text: text.to_owned(), pos: 0, real: true, source: 0, base: offset, pad: 0, origin: 0, internal: true, name: None });
        self.declarations_loop(false);
        self.frames.clear();
    }

    fn run_external_subset(&mut self, public: Option<&str>, system: &str, base: &str) {
        let start = self.here();
        let Some((text, source)) = self.load("external subset", public, system, base, start) else { return };
        self.base = self.sources.get(source).cloned().unwrap_or_default();
        self.run_text(&text, source);
    }

    /// Read `text` as the content of an external subset.
    fn run_text(&mut self, text: &str, source: usize) {
        let (text, offset) = strip_text_decl(self, text, source);
        self.frames.clear();
        self.frames.push(Frame { text, pos: 0, real: true, source, base: offset, pad: 0, origin: 0, internal: false, name: None });
        self.declarations_loop(false);
        self.frames.clear();
    }

    /// The declarations of a subset (or of an `INCLUDE` section, which ends at `]]>`).
    fn declarations_loop(&mut self, in_section: bool) {
        loop {
            if self.stopped {
                return;
            }
            self.in_decl = false;
            // Between declarations: white space and parameter-entity references (allowed in the internal subset too).
            loop {
                match self.peek() {
                    Some(c) if is_xml_space(c) => self.advance(c.len_utf8()),
                    Some('%') if self.pe_reference_ahead() => {
                        self.expand_reference();
                    }
                    _ => break,
                }
                if self.stopped {
                    return;
                }
            }
            let start = self.here();
            let Some(c) = self.peek() else {
                if in_section {
                    self.report(Code::DtdBadConditional, vec!["the section is not closed: expected ']]>'".to_owned()], start);
                }
                return;
            };
            self.declarations += 1;
            if self.declarations > MAX_DECLARATIONS {
                self.report(Code::DtdLimit, vec![format!("more than {MAX_DECLARATIONS} declarations")], start);
                self.stopped = true;
                return;
            }
            match c {
                '<' => {
                    if self.starts_with("<!--") {
                        self.comment();
                    } else if self.starts_with("<?") {
                        self.processing_instruction();
                    } else if self.starts_with("<!ELEMENT") {
                        self.element_decl();
                    } else if self.starts_with("<!ATTLIST") {
                        self.attlist_decl();
                    } else if self.starts_with("<!ENTITY") {
                        self.entity_decl();
                    } else if self.starts_with("<!NOTATION") {
                        self.notation_decl();
                    } else if self.starts_with("<![") {
                        self.conditional_section();
                    } else {
                        self.advance(1);
                        let shown: String = self.rest().chars().take(12).take_while(|c| !is_xml_space(*c)).collect();
                        self.report(Code::UnknownDeclaration, vec![format!("<{shown}")], start);
                        self.skip_declaration();
                    }
                }
                ']' if in_section && self.starts_with("]]>") => {
                    self.advance(3);
                    return;
                }
                other => {
                    // A run of characters that are no declaration is one problem.
                    let mut shown = String::from(other);
                    self.advance(other.len_utf8());
                    while let Some(next) = self.peek_raw().filter(|c| !is_xml_space(*c) && !matches!(c, '<' | '%')) {
                        if shown.chars().count() < 12 {
                            shown.push(next);
                        }
                        self.advance(next.len_utf8());
                    }
                    self.report(Code::UnexpectedCharacter, vec![shown], start);
                }
            }
        }
    }

    /// After a mistake in a declaration: go on after the next `>`.
    fn skip_declaration(&mut self) {
        self.in_decl = false;
        while let Some(c) = self.bump() {
            if c == '>' {
                return;
            }
        }
    }

    // --- comments, processing instructions, text declarations ---

    fn comment(&mut self) {
        let start = self.here();
        self.advance(4);
        loop {
            // A run of hyphens: two end the comment when a `>` follows; any more, or two with something else after, are a mistake (and a
            // comment that ends `--->` is still ended there).
            let hyphens = self.rest().bytes().take_while(|&b| b == b'-').count();
            if hyphens >= 2 {
                self.advance(hyphens);
                let ends = self.eat(">");
                if ends && hyphens == 2 {
                    return;
                }
                self.report(Code::DoubleHyphenInComment, Vec::new(), start);
                if ends {
                    return;
                }
                continue;
            }
            match self.bump() {
                Some(c) => self.check_char(c),
                None => {
                    self.report(Code::UnterminatedComment, Vec::new(), start);
                    return;
                }
            }
        }
    }

    fn processing_instruction(&mut self) {
        let start = self.here();
        self.advance(2);
        match self.name() {
            None => self.report(Code::MissingPiTarget, Vec::new(), start),
            Some(target) => {
                if target.eq_ignore_ascii_case("xml") {
                    self.report(Code::ReservedPiTarget, vec![target], start);
                } else if target.contains(':') {
                    self.report(Code::ColonInName, vec![target], start);
                }
                // The target is followed by white space or the end of the instruction.
                match self.peek() {
                    Some(c) if !is_xml_space(c) && !self.starts_with("?>") => {
                        let at = self.here();
                        self.report(Code::MissingSpace, vec![c.to_string()], at);
                    }
                    _ => {}
                }
            }
        }
        loop {
            if self.eat("?>") {
                return;
            }
            match self.bump() {
                Some(c) => self.check_char(c),
                None => {
                    self.report(Code::UnterminatedPi, Vec::new(), start);
                    return;
                }
            }
        }
    }

    // --- element declarations ---

    fn element_decl(&mut self) {
        let start = self.here();
        self.advance("<!ELEMENT".len());
        self.in_decl = true;
        let Some(name) = self.declaration_name(Code::DtdBadContentModel, "the element's name") else { return self.skip_declaration() };
        if !self.require_s(Code::DtdBadContentModel, "between the name and the content specification") {
            return self.skip_declaration();
        }
        let Some(spec) = self.content_spec() else { return self.skip_declaration() };
        self.s();
        if !self.eat(">") {
            self.fail(Code::DtdBadContentModel, "expected '>' to end the declaration");
            return self.skip_declaration();
        }
        self.in_decl = false;
        let place = self.place_from(start);
        self.dtd.elements.push(ElementDecl { name, spec, place });
    }

    /// The name that follows a declaration keyword and its white space.
    fn declaration_name(&mut self, code: Code, what: &str) -> Option<String> {
        if !self.require_s(code, "after the keyword") {
            return None;
        }
        let name = self.name();
        if name.is_none() {
            self.fail(code, &format!("expected {what}"));
        }
        name
    }

    fn place_from(&mut self, start: (usize, usize)) -> Place {
        let (source, end) = self.here();
        let from = if start.0 == source { start.1.min(end) } else { end };
        Place { source, range: from..end }
    }

    fn content_spec(&mut self) -> Option<ContentSpec> {
        if self.keyword("EMPTY") {
            return Some(ContentSpec::Empty);
        }
        if self.keyword("ANY") {
            return Some(ContentSpec::Any);
        }
        if self.peek() != Some('(') {
            self.fail(Code::DtdBadContentModel, "expected EMPTY, ANY or a group in parentheses");
            return None;
        }
        // `(` S? `#PCDATA` is the mixed form; anything else is a group of children.
        let mark = self.mark();
        self.advance(1);
        self.s();
        if self.starts_with("#PCDATA") {
            return self.mixed();
        }
        self.reset(mark);
        let particle = self.group(0)?;
        Some(ContentSpec::Children(particle))
    }

    /// Where reading is, to come back to it: the depth of the stack and the position of its top.
    fn mark(&mut self) -> (usize, usize) {
        self.settle();
        (self.frames.len(), self.frames.last().map_or(0, |f| f.pos))
    }

    fn reset(&mut self, mark: (usize, usize)) {
        self.frames.truncate(mark.0);
        if let Some(f) = self.frames.last_mut() {
            f.pos = mark.1.min(f.text.len());
        }
    }

    /// `(#PCDATA | a | b)*` or `(#PCDATA)`; the `(` and the white space after it are read, the cursor is at `#PCDATA`.
    fn mixed(&mut self) -> Option<ContentSpec> {
        self.advance("#PCDATA".len());
        let mut names = Vec::new();
        loop {
            self.s();
            if self.eat(")") {
                break;
            }
            if !self.eat("|") {
                self.fail(Code::DtdBadContentModel, "in mixed content only '|' may follow #PCDATA: expected '|' or ')'");
                return None;
            }
            self.s();
            match self.name() {
                Some(name) => names.push(name),
                None => {
                    self.fail(Code::DtdBadContentModel, "expected an element name in the mixed content");
                    return None;
                }
            }
        }
        let starred = self.eat("*");
        if !names.is_empty() && !starred {
            self.fail(Code::DtdBadContentModel, "mixed content with element names must end with ')*'");
            return None;
        }
        Some(ContentSpec::Mixed(names))
    }

    /// A group `( cp (, cp)* )` or `( cp (| cp)* )` and its occurrence indicator. The cursor is at `(`.
    fn group(&mut self, depth: usize) -> Option<Particle> {
        if depth > MAX_GROUP_DEPTH {
            self.fail(Code::DtdLimit, &format!("groups nested more than {MAX_GROUP_DEPTH} deep"));
            self.stopped = true;
            return None;
        }
        self.advance(1);
        self.s();
        let mut items = vec![self.particle(depth)?];
        let mut separator: Option<char> = None;
        loop {
            self.s();
            match self.peek() {
                Some(')') => {
                    self.advance(1);
                    break;
                }
                Some(c @ (',' | '|')) => {
                    match separator {
                        None => separator = Some(c),
                        Some(first) if first != c => {
                            self.fail(Code::DtdBadContentModel, "',' and '|' cannot be mixed in one group");
                            return None;
                        }
                        Some(_) => {}
                    }
                    self.advance(1);
                    self.s();
                    items.push(self.particle(depth)?);
                }
                _ => {
                    self.fail(Code::DtdBadContentModel, "expected ',', '|' or ')' in the group");
                    return None;
                }
            }
        }
        let occurs = self.occurrence();
        let kind = if separator == Some('|') { ParticleKind::Choice(items) } else { ParticleKind::Sequence(items) };
        Some(Particle { kind, occurs })
    }

    fn particle(&mut self, depth: usize) -> Option<Particle> {
        if self.peek() == Some('(') {
            return self.group(depth + 1);
        }
        match self.name() {
            Some(name) => {
                let occurs = self.occurrence();
                Some(Particle { kind: ParticleKind::Name(name), occurs })
            }
            None => {
                self.fail(Code::DtdBadContentModel, "expected an element name or a group");
                None
            }
        }
    }

    fn occurrence(&mut self) -> Occurs {
        match self.peek() {
            Some('?') => {
                self.advance(1);
                Occurs::Optional
            }
            Some('*') => {
                self.advance(1);
                Occurs::ZeroOrMore
            }
            Some('+') => {
                self.advance(1);
                Occurs::OneOrMore
            }
            _ => Occurs::Once,
        }
    }

    // --- attribute-list declarations ---

    fn attlist_decl(&mut self) {
        let start = self.here();
        self.advance("<!ATTLIST".len());
        self.in_decl = true;
        let Some(element) = self.declaration_name(Code::DtdBadAttlist, "the element's name") else { return self.skip_declaration() };
        let mut defs = Vec::new();
        loop {
            let had_space = self.s();
            if self.eat(">") {
                break;
            }
            if !had_space {
                self.fail(Code::DtdBadAttlist, "white space is needed before an attribute definition");
                return self.skip_declaration();
            }
            let def_start = self.here();
            let Some(name) = self.name() else {
                self.fail(Code::DtdBadAttlist, "expected an attribute name or '>'");
                return self.skip_declaration();
            };
            if !self.require_s(Code::DtdBadAttlist, "between the attribute's name and its type") {
                return self.skip_declaration();
            }
            let Some(ty) = self.att_type() else { return self.skip_declaration() };
            if !self.require_s(Code::DtdBadAttlist, "between the attribute's type and its default") {
                return self.skip_declaration();
            }
            let Some(default) = self.default_decl() else { return self.skip_declaration() };
            let place = self.place_from(def_start);
            defs.push(AttDef { name, ty, default, place });
        }
        self.in_decl = false;
        let place = self.place_from(start);
        self.dtd.attlists.push(AttlistDecl { element, defs, place });
    }

    fn att_type(&mut self) -> Option<AttType> {
        for (word, ty) in [
            ("CDATA", AttType::Cdata),
            ("IDREFS", AttType::Idrefs),
            ("IDREF", AttType::Idref),
            ("ID", AttType::Id),
            ("ENTITIES", AttType::Entities),
            ("ENTITY", AttType::Entity),
            ("NMTOKENS", AttType::Nmtokens),
            ("NMTOKEN", AttType::Nmtoken),
        ] {
            if self.keyword(word) {
                return Some(ty);
            }
        }
        if self.keyword("NOTATION") {
            if !self.require_s(Code::DtdBadAttlist, "after NOTATION") {
                return None;
            }
            if self.peek() != Some('(') {
                self.fail(Code::DtdBadAttlist, "expected '(' after NOTATION");
                return None;
            }
            return self.enumeration(true).map(AttType::Notation);
        }
        if self.peek() == Some('(') {
            return self.enumeration(false).map(AttType::Enumeration);
        }
        self.fail(
            Code::DtdBadAttlist,
            "expected an attribute type (CDATA, ID, IDREF, IDREFS, ENTITY, ENTITIES, NMTOKEN, NMTOKENS, NOTATION or a list in parentheses)",
        );
        None
    }

    /// `( token | token )`; names for NOTATION, name tokens otherwise.
    fn enumeration(&mut self, names: bool) -> Option<Vec<String>> {
        self.advance(1);
        let mut values = Vec::new();
        self.s();
        loop {
            let value = if names { self.name() } else { self.nmtoken() };
            match value {
                Some(v) => values.push(v),
                None => {
                    self.fail(Code::DtdBadAttlist, if names { "expected a notation name" } else { "expected a name token" });
                    return None;
                }
            }
            self.s();
            if self.eat(")") {
                return Some(values);
            }
            if !self.eat("|") {
                self.fail(Code::DtdBadAttlist, "expected '|' or ')' in the list");
                return None;
            }
            self.s();
        }
    }

    fn default_decl(&mut self) -> Option<Default> {
        if self.eat("#REQUIRED") {
            return Some(Default::Required);
        }
        if self.eat("#IMPLIED") {
            return Some(Default::Implied);
        }
        if self.eat("#FIXED") {
            if !self.require_s(Code::DtdBadAttlist, "after #FIXED") {
                return None;
            }
            return self.att_value().map(Default::Fixed);
        }
        if matches!(self.peek(), Some('"' | '\'')) {
            return self.att_value().map(Default::Value);
        }
        self.fail(Code::DtdBadAttlist, "expected #REQUIRED, #IMPLIED, #FIXED or a default value in quotes");
        None
    }

    /// A default value: in quotes, with no `<`, and every `&` starting a reference. Its text as written.
    fn att_value(&mut self) -> Option<String> {
        let start = self.here();
        let Some(quote) = self.peek().filter(|c| matches!(c, '"' | '\'')) else {
            self.fail(Code::DtdBadAttlist, "the default value must be in quotes");
            return None;
        };
        self.advance(1);
        let mut value = String::new();
        loop {
            match self.peek_raw() {
                None => {
                    self.report(Code::UnterminatedLiteral, vec![quote.to_string()], start);
                    return None;
                }
                Some(c) if c == quote => {
                    self.advance(1);
                    return Some(value);
                }
                Some('<') => {
                    self.fail(Code::DtdBadAttlist, "'<' is not allowed in an attribute value");
                    return None;
                }
                Some('&') => {
                    let at = self.here();
                    match self.reference_text() {
                        Some(text) => value.push_str(&text),
                        None => {
                            self.report(Code::BareAmpersand, Vec::new(), at);
                            return None;
                        }
                    }
                }
                Some(c) => {
                    self.advance(c.len_utf8());
                    self.check_char(c);
                    value.push(c);
                }
            }
        }
    }

    /// `&name;` or `&#n;` or `&#xh;` as written (the cursor is at `&`); `None` when it is not one.
    fn reference_text(&mut self) -> Option<String> {
        let rest = self.rest_raw();
        let end = rest.find(';')?;
        let body = rest.get(1..end)?;
        let valid = match body.strip_prefix('#') {
            Some(number) => char_reference(number).is_some(),
            None => {
                let mut chars = body.chars();
                chars.next().is_some_and(is_name_start_char) && chars.all(is_name_char)
            }
        };
        if !valid {
            return None;
        }
        let text = rest.get(..=end)?.to_owned();
        self.advance(text.len());
        Some(text)
    }

    // --- entity declarations ---

    fn entity_decl(&mut self) {
        let start = self.here();
        self.advance("<!ENTITY".len());
        self.in_decl = true;
        if !self.require_s(Code::DtdBadEntityDecl, "after the keyword") {
            return self.skip_declaration();
        }
        // `%` and white space: a parameter entity. `%name;` was a reference, which `s` has expanded already.
        let parameter = if self.peek() == Some('%') {
            self.advance(1);
            if !self.require_s(Code::DtdBadEntityDecl, "after '%' in the declaration of a parameter entity") {
                return self.skip_declaration();
            }
            true
        } else {
            false
        };
        let Some(name) = self.name() else {
            self.fail(Code::DtdBadEntityDecl, "expected the entity's name");
            return self.skip_declaration();
        };
        // Names of entities and notations are NCNames in a namespace-aware document.
        if name.contains(':') {
            let at = self.here();
            self.report(Code::ColonInName, vec![name.clone()], at);
        }
        if !self.require_s(Code::DtdBadEntityDecl, "between the entity's name and its definition") {
            return self.skip_declaration();
        }
        let (value, notation) = match self.peek() {
            Some('"' | '\'') => {
                let Some(text) = self.entity_value() else { return self.skip_declaration() };
                (EntityValue::Internal(text), None)
            }
            _ => {
                let Some(id) = self.external_id(false, Code::DtdBadEntityDecl) else { return self.skip_declaration() };
                let mut notation = None;
                let had_space = self.s();
                if self.starts_with("NDATA") {
                    if parameter {
                        self.fail(Code::DtdBadEntityDecl, "a parameter entity cannot be unparsed (NDATA)");
                        return self.skip_declaration();
                    }
                    if !had_space {
                        self.fail(Code::DtdBadEntityDecl, "white space is needed before NDATA");
                        return self.skip_declaration();
                    }
                    self.advance("NDATA".len());
                    if !self.require_s(Code::DtdBadEntityDecl, "after NDATA") {
                        return self.skip_declaration();
                    }
                    match self.name() {
                        Some(n) => notation = Some(n),
                        None => {
                            self.fail(Code::DtdBadEntityDecl, "expected the notation's name after NDATA");
                            return self.skip_declaration();
                        }
                    }
                    self.s();
                }
                (EntityValue::External(id), notation)
            }
        };
        self.s();
        if !self.eat(">") {
            self.fail(Code::DtdBadEntityDecl, "expected '>' to end the declaration");
            return self.skip_declaration();
        }
        self.in_decl = false;
        let place = self.place_from(start);
        let location = (place.source != 0).then(|| self.sources.get(place.source).cloned().unwrap_or_default());
        let decl = EntityDecl { name, value, notation, place, location };
        if parameter { self.dtd.parameter_entities.push(decl) } else { self.dtd.general_entities.push(decl) }
    }

    /// An entity value in quotes: its replacement text. Character references and (outside the internal subset) parameter-entity
    /// references are resolved; general-entity references stay as written.
    fn entity_value(&mut self) -> Option<String> {
        let start = self.here();
        let quote = self.bump()?;
        let internal = self.frames.last().is_some_and(|f| f.internal);
        let mut value = String::new();
        loop {
            let Some(c) = self.peek_raw() else {
                self.report(Code::UnterminatedLiteral, vec![quote.to_string()], start);
                return None;
            };
            match c {
                c if c == quote => {
                    self.advance(1);
                    return Some(value);
                }
                '%' => {
                    let at = self.here();
                    if !self.pe_reference_ahead() {
                        self.report(Code::BadEntityValue, vec!["%".to_owned()], at);
                        return None;
                    }
                    if internal {
                        let name = self.read_pe_reference().unwrap_or_default();
                        self.report(Code::PeInDeclaration, vec![name], at);
                        return None;
                    }
                    let name = self.read_pe_reference()?;
                    if self.frames.iter().any(|f| f.name.as_deref() == Some(name.as_str())) {
                        self.report(Code::DtdRecursiveEntity, vec![format!("%{name};")], at);
                        return None;
                    }
                    if let Some((text, _)) = self.pe_text(&name, at) {
                        self.expanded = self.expanded.saturating_add(text.len());
                        if self.expanded > MAX_EXPANDED_BYTES || value.len().saturating_add(text.len()) > MAX_EXPANDED_BYTES {
                            self.report(Code::DtdLimit, vec![format!("entity values longer than {MAX_EXPANDED_BYTES} bytes")], at);
                            self.stopped = true;
                            return None;
                        }
                        value.push_str(&text);
                    }
                }
                '&' => {
                    let at = self.here();
                    let rest = self.rest_raw();
                    if rest.starts_with("&#") {
                        let Some(end) = rest.find(';') else {
                            self.report(Code::BadEntityValue, vec!["&".to_owned()], at);
                            return None;
                        };
                        let Some(character) = rest.get(2..end).and_then(char_reference) else {
                            let shown = rest.get(..=end).unwrap_or_default().to_owned();
                            self.report(Code::InvalidCharRef, vec![shown], at);
                            return None;
                        };
                        self.advance(end + 1);
                        value.push(character);
                    } else {
                        match self.reference_text() {
                            Some(text) => value.push_str(&text),
                            None => {
                                self.report(Code::BadEntityValue, vec!["&".to_owned()], at);
                                return None;
                            }
                        }
                    }
                }
                other => {
                    self.advance(other.len_utf8());
                    self.check_char(other);
                    value.push(other);
                }
            }
        }
    }

    /// `SYSTEM "s"` or `PUBLIC "p" "s"`; with `public_only` (a notation) `PUBLIC "p"` stands alone.
    fn external_id(&mut self, public_only: bool, code: Code) -> Option<ExternalId> {
        if !public_only && self.keyword("SYSTEM") {
            if !self.require_s(code, "after SYSTEM") {
                return None;
            }
            let system = self.system_literal(code)?;
            return Some(ExternalId { public: None, system: Some(system) });
        }
        if self.keyword("PUBLIC") {
            if !self.require_s(code, "after PUBLIC") {
                return None;
            }
            let public = self.pubid_literal(code)?;
            if public_only {
                // The system literal is optional here: it is there only when white space and a quote follow.
                let mark = self.mark();
                let had_space = self.s();
                if had_space && matches!(self.peek(), Some('"' | '\'')) {
                    let system = self.system_literal(code)?;
                    return Some(ExternalId { public: Some(public), system: Some(system) });
                }
                self.reset(mark);
                return Some(ExternalId { public: Some(public), system: None });
            }
            if !self.require_s(code, "between the public and the system identifier") {
                return None;
            }
            let system = self.system_literal(code)?;
            return Some(ExternalId { public: Some(public), system: Some(system) });
        }
        self.fail(code, if public_only { "PUBLIC" } else { "SYSTEM or PUBLIC" });
        None
    }

    fn system_literal(&mut self, code: Code) -> Option<String> {
        let start = self.here();
        let Some(quote) = self.peek().filter(|c| matches!(c, '"' | '\'')) else {
            self.fail(code, "a system identifier in quotes");
            return None;
        };
        self.advance(1);
        let mut out = String::new();
        loop {
            match self.bump_raw() {
                None => {
                    self.report(Code::UnterminatedLiteral, vec![quote.to_string()], start);
                    return None;
                }
                Some(c) if c == quote => return Some(out),
                Some(c) => {
                    self.check_char(c);
                    out.push(c);
                }
            }
        }
    }

    fn pubid_literal(&mut self, code: Code) -> Option<String> {
        let start = self.here();
        let Some(quote) = self.peek().filter(|c| matches!(c, '"' | '\'')) else {
            self.fail(code, "a public identifier in quotes");
            return None;
        };
        self.advance(1);
        let mut out = String::new();
        loop {
            match self.bump_raw() {
                None => {
                    self.report(Code::UnterminatedLiteral, vec![quote.to_string()], start);
                    return None;
                }
                Some(c) if c == quote => return Some(out),
                Some(c) if is_pubid_char(c) => out.push(c),
                Some(c) => {
                    let at = self.here();
                    self.report(Code::BadPubidLiteral, vec![c.to_string()], at);
                    return None;
                }
            }
        }
    }

    // --- notation declarations ---

    fn notation_decl(&mut self) {
        let start = self.here();
        self.advance("<!NOTATION".len());
        self.in_decl = true;
        let Some(name) = self.declaration_name(Code::DtdBadNotation, "the notation's name") else { return self.skip_declaration() };
        if name.contains(':') {
            let at = self.here();
            self.report(Code::ColonInName, vec![name.clone()], at);
        }
        if !self.require_s(Code::DtdBadNotation, "between the notation's name and its identifier") {
            return self.skip_declaration();
        }
        let Some(id) = self.external_id_for_notation() else { return self.skip_declaration() };
        self.s();
        if !self.eat(">") {
            self.fail(Code::DtdBadNotation, "expected '>' to end the declaration");
            return self.skip_declaration();
        }
        self.in_decl = false;
        let place = self.place_from(start);
        self.dtd.notations.push(NotationDecl { name, id, place });
    }

    /// `SYSTEM "s"`, `PUBLIC "p" "s"` or `PUBLIC "p"`.
    fn external_id_for_notation(&mut self) -> Option<ExternalId> {
        if self.starts_with("SYSTEM") {
            return self.external_id(false, Code::DtdBadNotation);
        }
        self.external_id(true, Code::DtdBadNotation)
    }

    // --- conditional sections ---

    fn conditional_section(&mut self) {
        let start = self.here();
        let internal = self.frames.last().is_some_and(|f| f.internal);
        self.advance(3);
        if internal {
            self.report(Code::DtdConditionalInInternal, Vec::new(), start);
        }
        self.in_decl = true;
        self.s();
        let include = if self.keyword("INCLUDE") {
            true
        } else if self.keyword("IGNORE") {
            false
        } else {
            self.fail(Code::DtdBadConditional, "expected INCLUDE or IGNORE");
            return self.skip_declaration();
        };
        self.s();
        if !self.eat("[") {
            self.fail(Code::DtdBadConditional, "expected '[' after INCLUDE or IGNORE");
            return self.skip_declaration();
        }
        self.in_decl = false;
        if include {
            self.declarations_loop(true);
        } else {
            self.ignore_section(start);
        }
    }

    /// The content of an `IGNORE` section, to its closing `]]>`; sections inside it nest.
    fn ignore_section(&mut self, start: (usize, usize)) {
        let mut depth = 1usize;
        while depth > 0 {
            if self.starts_with("<![") {
                self.advance(3);
                depth += 1;
            } else if self.starts_with("]]>") {
                self.advance(3);
                depth -= 1;
            } else {
                // What is ignored is still text: it can have no character that XML does not allow.
                match self.bump() {
                    Some(c) => self.check_char(c),
                    None => {
                        self.report(Code::DtdBadConditional, vec!["the section is not closed: expected ']]>'".to_owned()], start);
                        return;
                    }
                }
            }
        }
    }
}

/// The text of an external entity without its text declaration (`<?xml version? encoding?>`), which is read for what is wrong with it,
/// and where in the entity that text begins. Anything else that starts with `<?xml` later in the text is reported where it is.
fn strip_text_decl(parser: &mut Parser<'_>, text: &str, source: usize) -> (String, usize) {
    let split = split_text_decl(text);
    if let Some(problem) = split.problem {
        parser.push(source, Diagnostic::new(Code::DtdBadTextDecl, 0..split.span, vec![problem.to_owned()]));
    }
    (text.get(split.content..).unwrap_or_default().to_owned(), split.content)
}

/// The text declaration an external entity may begin with, taken apart.
pub(crate) struct TextDecl {
    /// Where the content begins (0 when there is no declaration, or when it is not closed).
    pub content: usize,
    /// How much of the text is the declaration, for reporting.
    pub span: usize,
    /// What is wrong with it.
    pub problem: Option<&'static str>,
}

/// Take the text declaration (`<?xml version? encoding?>`) off the front of an external entity, and read it for what is wrong with it.
pub(crate) fn split_text_decl(text: &str) -> TextDecl {
    let none = TextDecl { content: 0, span: 0, problem: None };
    let Some(rest) = text.strip_prefix("<?xml") else { return none };
    if rest.chars().next().is_some_and(|c| !is_xml_space(c) && c != '?') {
        return none;
    }
    let Some(end) = rest.find("?>") else {
        return TextDecl { span: 5, problem: Some("it is not closed: expected '?>'"), ..none };
    };
    let body = rest.get(..end).unwrap_or_default();
    TextDecl { content: 5 + end + 2, span: 5 + end + 2, problem: text_decl_problem(body) }
}

/// What is wrong with the pseudo-attributes of a text declaration: `version="1.x"`? then `encoding="..."` (required), nothing else.
fn text_decl_problem(body: &str) -> Option<&'static str> {
    let mut seen_version = false;
    let mut seen_encoding = false;
    let mut rest = body;
    loop {
        let trimmed = rest.trim_start_matches(is_xml_space);
        if trimmed.len() == rest.len() && !rest.is_empty() {
            return Some("white space is needed between the parts");
        }
        rest = trimmed;
        if rest.is_empty() {
            break;
        }
        let name_end = rest.find(|c: char| !is_name_char(c)).unwrap_or(rest.len());
        let name = rest.get(..name_end).unwrap_or_default();
        rest = rest.get(name_end..).unwrap_or_default().trim_start_matches(is_xml_space);
        let Some(after_equals) = rest.strip_prefix('=') else { return Some("expected '=' after the name") };
        rest = after_equals.trim_start_matches(is_xml_space);
        let Some(quote) = rest.chars().next().filter(|c| matches!(c, '"' | '\'')) else { return Some("a value must be in quotes") };
        let Some(value_end) = rest.get(1..).and_then(|after| after.find(quote)) else { return Some("a value is not closed") };
        let value = rest.get(1..1 + value_end).unwrap_or_default();
        rest = rest.get(value_end + 2..).unwrap_or_default();
        match name {
            "version" if !seen_version && !seen_encoding => {
                if !(value.starts_with("1.") && value.len() > 2 && value.get(2..).is_some_and(|d| d.chars().all(|c| c.is_ascii_digit()))) {
                    return Some("the version is not a number like 1.0");
                }
                seen_version = true;
            }
            "encoding" if !seen_encoding => {
                seen_encoding = true;
                let mut chars = value.chars();
                if !(chars.next().is_some_and(|c| c.is_ascii_alphabetic())
                    && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
                {
                    return Some("the encoding name is not valid");
                }
            }
            "version" | "encoding" => return Some("the parts are in the wrong order or repeated"),
            _ => return Some("only version and encoding are allowed"),
        }
    }
    if seen_encoding { None } else { Some("the encoding is missing") }
}

/// `PubidChar`.
fn is_pubid_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            ' ' | '\r' | '\n' | '-' | '\'' | '(' | ')' | '+' | ',' | '.' | '/' | ':' | '=' | '?' | ';' | '!' | '*' | '#' | '@' | '$' | '_' | '%'
        )
}

/// The character a reference's number (`123` or `x7B`) stands for, if it is one XML allows.
pub(crate) fn char_reference(number: &str) -> Option<char> {
    let code = match number.strip_prefix('x') {
        Some(hex) if !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()) => u32::from_str_radix(hex, 16).ok()?,
        Some(_) => return None,
        None if !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) => number.parse().ok()?,
        None => return None,
    };
    char::from_u32(code).filter(|&c| is_xml_char(c))
}
