//! One open document: its text and editor, how its file is encoded, and whether it has changes that are not saved.
//!
//! The text is the model (CLAUDE.md): opening a file and saving it without editing writes the same bytes, whatever the encoding, byte
//! order mark and line endings. Where reading had to replace something (bytes that are not valid in the encoding), the document says so
//! and refuses to overwrite the file it came from, so nothing is lost without the user choosing it.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use xmlcraft_editor::{Editor, Settings, Syntax};

use crate::language::Language;
use xmlcraft_format::{BlankLines, FormatError, FormatOptions, Indent};
use xmlcraft_syntax::{Lexer, TokenKind};
use xmlcraft_text::Edit;
use xmlcraft_text::encoding::{
    Detected, EncodeError, Encoding, Unmappable, declared_encoding_span, decode, encode_chunks, sniff, unmappable_characters,
};
use xmlcraft_text::eol::LineEndings;

/// Files larger than this are not opened (until large-file mode, M1.15).
pub const MAX_OPEN_BYTES: usize = 256 * 1024 * 1024;

/// How much of the start of the text is looked at for the XML declaration.
const DECLARATION_HEAD: usize = 4096;

/// How many unmappable characters are collected; more than this is not a few stray characters.
const MAX_UNMAPPABLE: usize = 100_000;

fn syntax_of(language: Language) -> Syntax {
    match language {
        Language::Xml => Syntax::Xml,
        Language::Json(crate::language::JsonFlavor::Json5) => Syntax::Json5,
        Language::Json(_) | Language::Bson => Syntax::Json,
    }
}

/// What a file looked like when it was last read or written, to notice that something else changed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
    /// When the file's inode last changed, as (seconds, nanoseconds): any write moves it, even one that keeps the size and puts the
    /// modification time back. Only where the platform has it (Unix).
    pub changed: Option<(i64, i64)>,
    /// Which file it is, as (device, inode), so that two paths to one file are known to be one. Only where the platform has it (Unix).
    pub file_id: Option<(u64, u64)>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    TooLarge {
        len: usize,
        max: usize,
    },
    /// The bytes are not what the name says they are (a `.bson` file that is not BSON).
    Invalid(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::TooLarge { len, max } => write!(f, "the file is {len} bytes, more than the {max} bytes XmlCraft opens"),
            OpenError::Invalid(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for OpenError {}

#[derive(Debug, PartialEq, Eq)]
pub enum SaveError {
    /// A character the file's encoding cannot hold. Writing it as a character reference (where XML allows one), or another encoding, is
    /// the user's choice: [`Document::apply_character_references`] and [`Document::set_encoding`].
    Unmappable(EncodeError),
    /// Reading the file replaced bytes that were not valid; saving over it would make that permanent.
    Lossy,
    /// The text is not what the file format needs (a BSON file whose text is not valid Extended JSON): the reason.
    Invalid(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Unmappable(e) => e.fmt(f),
            SaveError::Lossy => f.write_str("the file has bytes that are not valid in its encoding, and saving would replace them for good"),
            SaveError::Invalid(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for SaveError {}

/// What [`Document::prepare_save_as`] did, to be taken back if the save fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveAsPrep {
    edited: Option<u64>,
}

pub struct Document {
    id: u64,
    name: String,
    path: Option<PathBuf>,
    editor: Editor,
    encoding: Detected,
    lossy: bool,
    /// The editor's text state ([`Editor::state_id`]) when the file was last read or written: undoing back to it makes the document
    /// clean again.
    saved_state: u64,
    /// The encoding and byte-order mark the file has: another one makes the document dirty until it is the same again.
    saved_encoding: (Encoding, bool),
    stamp: Option<FileStamp>,
    /// The line endings of the text, counted for the editor version in the first part (counting reads the whole text).
    endings: Cell<Option<(u64, LineEndings)>>,
    /// A BSON file: its documents as they were read (or last saved) with the bytes of each, so that a document that is not edited is
    /// written back as the exact bytes it came from.
    bson: Option<Vec<(xmlcraft_bson::Document, Vec<u8>)>>,
}

impl Document {
    /// A new, empty document that has no file yet.
    pub fn untitled(id: u64, number: usize) -> Self {
        let editor = Editor::new("");
        let encoding = Detected::utf8();
        Self {
            id,
            name: format!("Untitled-{number}"),
            path: None,
            saved_state: editor.state_id(),
            saved_encoding: (encoding.encoding, encoding.bom),
            editor,
            encoding,
            lossy: false,
            stamp: None,
            endings: Cell::new(None),
            bson: None,
        }
    }

    /// A document read from the bytes of a file.
    pub fn from_bytes(id: u64, path: Option<PathBuf>, bytes: &[u8], stamp: Option<FileStamp>) -> Result<Self, OpenError> {
        if bytes.len() > MAX_OPEN_BYTES {
            return Err(OpenError::TooLarge { len: bytes.len(), max: MAX_OPEN_BYTES });
        }
        let name = path.as_deref().and_then(Path::file_name).map_or_else(|| "Untitled".to_owned(), |n| n.to_string_lossy().into_owned());
        if Language::of_name(&name) == Language::Bson && !bytes.is_empty() {
            return Self::from_bson(id, name, path, bytes, stamp);
        }
        let encoding = sniff(bytes);
        let decoded = decode(bytes, &encoding);
        let settings = settings_for(&decoded.text);
        let mut editor = Editor::with_settings("", settings);
        editor.load(&decoded.text);
        let saved_encoding = (encoding.encoding, encoding.bom);
        editor.set_syntax(syntax_of(Language::of_name(&name)));
        Ok(Self {
            id,
            name,
            path,
            saved_state: editor.state_id(),
            saved_encoding,
            editor,
            encoding,
            lossy: decoded.lossy,
            stamp,
            endings: Cell::new(None),
            bson: None,
        })
    }

    /// A BSON file: shown as the Extended JSON text of its documents. A file that is not BSON is not opened as text (it would be saved
    /// back as something else).
    fn from_bson(id: u64, name: String, path: Option<PathBuf>, bytes: &[u8], stamp: Option<FileStamp>) -> Result<Self, OpenError> {
        let read = xmlcraft_bson::read_with_ranges(bytes).map_err(|e| OpenError::Invalid(format!("this is not a BSON file ({e})")))?;
        let documents: Vec<xmlcraft_bson::Document> = read.iter().map(|(d, _)| d.clone()).collect();
        let (text, _) = xmlcraft_bson::to_text(&documents, true)
            .map_err(|e| OpenError::Invalid(format!("this BSON file cannot be shown as text ({})", e.message)))?;
        let originals = read.into_iter().map(|(d, range)| (d, bytes.get(range).unwrap_or_default().to_vec())).collect();
        let encoding = Detected::utf8();
        let mut editor = Editor::with_settings("", settings_for(&text));
        editor.load(&text);
        editor.set_syntax(Syntax::Json);
        Ok(Self {
            id,
            name,
            path,
            saved_state: editor.state_id(),
            saved_encoding: (encoding.encoding, encoding.bom),
            editor,
            encoding,
            lossy: false,
            stamp,
            endings: Cell::new(None),
            bson: Some(originals),
        })
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// The name shown on the tab: the file's name, or `Untitled-N`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What the document is written in, by its name.
    pub fn language(&self) -> Language {
        Language::of_name(&self.name)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Give a document that is not a file yet (generated text, a new document) the name it is shown and saved under by default.
    /// A document that is a file keeps the name of its file.
    pub fn name_untitled(&mut self, name: &str) {
        if self.path.is_none() && !name.is_empty() {
            self.name = name.to_owned();
            self.editor.set_syntax(syntax_of(self.language()));
        }
    }

    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    pub fn editor_mut(&mut self) -> &mut Editor {
        &mut self.editor
    }

    /// Make typing in the name of a tag change the name in the element's other tag too, if the single caret is in a tag name. For a caller
    /// that has no analysis of the text (a window has: it links with `Analysis::linked_names`); the text is analysed now. Returns whether
    /// names are linked.
    pub fn link_tag_names(&mut self) -> bool {
        self.link_tag_names_by(crate::analysis::analyze_guarded)
    }

    /// [`Self::link_tag_names`] with the analysis done by `analyse` (the fuzzer's self-check passes the unguarded one, so that a panic in
    /// the analysis is seen and not turned into "no pairs").
    pub fn link_tag_names_by(&mut self, analyse: impl FnOnce(&str, u64) -> crate::analysis::Analysis) -> bool {
        let editor = &mut self.editor;
        let [caret] = editor.carets() else {
            editor.unlink_names();
            return false;
        };
        if !caret.is_empty() {
            editor.unlink_names();
            return false;
        }
        if editor.caret_in_linked_names() {
            return true;
        }
        let at = caret.head();
        // Only a caret in a tag can be in a name, and a text too large to analyse has no pairs: neither is worth copying the text for.
        if editor.buffer().len() > crate::analysis::MAX_ANALYSED_BYTES || !editor.caret_in_tag() {
            editor.unlink_names();
            return false;
        }
        let analysis = analyse(&editor.text(), editor.version());
        match analysis.skipped.is_none().then(|| analysis.linked_names(at)).flatten() {
            Some([a, b]) => editor.link_names(a, b),
            None => {
                editor.unlink_names();
                false
            }
        }
    }

    /// Whether the text, or how it would be written, differs from what was last saved. Undoing back to the saved text is clean again.
    pub fn is_dirty(&self) -> bool {
        self.editor.state_id() != self.saved_state || (self.encoding.encoding, self.encoding.bom) != self.saved_encoding
    }

    /// How the file is encoded, with its byte-order mark and what its XML declaration says.
    pub fn encoding(&self) -> &Detected {
        &self.encoding
    }

    /// Reading replaced bytes that were not valid in the encoding.
    pub fn is_lossy(&self) -> bool {
        self.lossy
    }

    pub fn stamp(&self) -> Option<&FileStamp> {
        self.stamp.as_ref()
    }

    /// Remember the file as it is on disk now, without reading it or touching the text: the user chose to keep their version, so the
    /// change that was noticed is not asked about again.
    pub fn set_stamp(&mut self, stamp: Option<FileStamp>) {
        self.stamp = stamp;
    }

    /// The line endings in the text now. Counted once per version of the text, so asking every frame costs nothing while it does not
    /// change.
    pub fn line_endings(&self) -> LineEndings {
        let version = self.editor.version();
        if let Some((counted_for, endings)) = self.endings.get()
            && counted_for == version
        {
            return endings;
        }
        let endings = LineEndings::count_chunks(self.editor.buffer().chunks());
        self.endings.set(Some((version, endings)));
        endings
    }

    /// Write the file in another encoding from now on (with or without a byte-order mark; only UTF-8 and UTF-16 have one). The XML
    /// declaration is brought in line as an edit of the text: a different `encoding` is replaced by the new name, a declaration that
    /// lacks one gets it (and a document with no declaration gets one) when the new encoding cannot be told from the file's bytes.
    /// Undoing that edit leaves the declaration as it was, not the encoding. Returns whether anything changed.
    pub fn set_encoding(&mut self, encoding: Encoding, bom: bool) -> bool {
        let bom = bom && (encoding.is_utf8() || encoding.is_utf16());
        if self.encoding.encoding == encoding && self.encoding.bom == bom {
            return false;
        }
        self.encoding.encoding = encoding;
        self.encoding.bom = bom;
        self.sync_declaration();
        true
    }

    /// Make the XML declaration say what the encoding is (see [`Self::set_encoding`]).
    fn sync_declaration(&mut self) {
        self.sync_declaration_for(self.language());
    }

    fn sync_declaration_for(&mut self, language: Language) {
        // Only XML has a declaration; a JSON file's encoding is whatever its bytes are.
        if language != Language::Xml {
            return;
        }
        let buffer = self.editor.buffer();
        let mut end = buffer.len().min(DECLARATION_HEAD);
        while end < buffer.len() && !buffer.is_boundary(end) {
            end += 1;
        }
        let Ok(head) = buffer.slice(0..end) else { return };
        let encoding = self.encoding.encoding;
        // A plain `UTF-16` names either byte order.
        let label = if encoding.is_utf16() { "UTF-16" } else { encoding.name() };
        // The bytes alone tell UTF-8, and UTF-16 with a byte-order mark; anything else needs the declaration.
        let must_declare = !(encoding.is_utf8() || (encoding.is_utf16() && self.encoding.bom));
        let edit = if let Some(span) = declared_encoding_span(&head) {
            let now = Detected { encoding, bom: self.encoding.bom, declared: head.get(span.clone()).map(str::to_owned) };
            (!now.declaration_matches()).then(|| Edit::replace(span, label.to_owned()))
        } else if has_declaration(&head) {
            version_end(&head).filter(|_| must_declare).map(|at| Edit::insert(at, format!(" encoding=\"{label}\"")))
        } else if must_declare {
            Some(Edit::insert(0, format!("<?xml version=\"1.0\" encoding=\"{label}\"?>{}", self.editor.settings().eol.as_str())))
        } else {
            None
        };
        if let Some(edit) = edit {
            self.editor.apply_edits(&[edit]);
        }
        let buffer = self.editor.buffer();
        let mut end = buffer.len().min(DECLARATION_HEAD);
        while end < buffer.len() && !buffer.is_boundary(end) {
            end += 1;
        }
        if let Ok(head) = buffer.slice(0..end) {
            self.encoding.declared = declared_encoding_span(&head).and_then(|span| head.get(span).map(str::to_owned));
        }
    }

    /// How the document is pretty-printed: with the indentation its new lines use and the line ending its text has most of now (the
    /// one it was opened with, when the text has no line break). Use it with [`xmlcraft_format::pretty_print_settled`], which writes the
    /// ending most of the result's breaks will have.
    pub fn format_options(&self) -> FormatOptions {
        let settings = self.editor.settings();
        let indent = if settings.use_tabs { Indent::Tab } else { Indent::Spaces(settings.indent_size) };
        let endings = self.line_endings();
        let eol = if endings.total() == 0 { settings.eol } else { endings.dominant() };
        FormatOptions { indent, eol, blank_lines: BlankLines::Keep }
    }

    /// Pretty-print the text in the way of its language: only the white space between nodes (XML) or tokens (JSON) changes (see
    /// `xmlcraft-format`), as one undoable edit. Returns whether anything changed. A document with a syntax error is refused and left as it
    /// is; so is a form this language is not given (JSON Lines are not indented).
    pub fn pretty_print(&mut self) -> Result<bool, FormatError> {
        let edits = self.language().format(&self.editor.text(), &self.format_options(), false)?;
        Ok(self.editor.apply_edits(&edits) && !edits.is_empty())
    }

    /// Remove the white space between nodes (XML) or tokens (JSON) that means nothing, as one undoable edit. Returns whether anything
    /// changed. A document with a syntax error is refused.
    pub fn minify(&mut self) -> Result<bool, FormatError> {
        let edits = self.language().format(&self.editor.text(), &self.format_options(), true)?;
        Ok(self.editor.apply_edits(&edits) && !edits.is_empty())
    }

    /// The characters the file's encoding cannot hold (the first ones, if there are very many), with where they are in the text.
    pub fn unmappable_characters(&self) -> Vec<EncodeError> {
        unmappable_characters(self.editor.buffer().chunks(), self.encoding.encoding, MAX_UNMAPPABLE)
    }

    /// Write every character the encoding cannot hold as a character reference (`&#x65E5;`), as one undoable edit of the text, so that
    /// the text is what the file will hold. A reference is only right in character data and attribute values: in a comment, a CDATA
    /// section, a processing instruction or a name it would be literal or invalid, so then nothing changes and the first character that
    /// cannot take one is the error (the user can choose another encoding instead). Returns how many were replaced.
    pub fn apply_character_references(&mut self) -> Result<usize, SaveError> {
        let found = self.unmappable_characters();
        let Some(first) = found.first() else { return Ok(0) };
        if found.len() >= MAX_UNMAPPABLE {
            return Err(SaveError::Unmappable(*first)); // too many to be a few stray characters: another encoding is the answer
        }
        let text = self.editor.text();
        if let Language::Json(flavor) = self.language() {
            return self.apply_json_escapes(&text, &found, flavor.dialect());
        }
        let mut tokens = Lexer::new(&text);
        let mut current = tokens.next();
        let mut edits = Vec::with_capacity(found.len());
        for e in &found {
            while current.as_ref().is_some_and(|t| t.end <= e.offset) {
                current = tokens.next();
            }
            let allowed = current.as_ref().is_some_and(|t| t.start <= e.offset && matches!(t.kind, TokenKind::Text | TokenKind::AttrText));
            if !allowed {
                return Err(SaveError::Unmappable(*e));
            }
            edits.push(Edit::replace(e.offset..e.offset + e.ch.len_utf8(), format!("&#x{:X};", u32::from(e.ch))));
        }
        if self.editor.apply_edits(&edits) { Ok(edits.len()) } else { Err(SaveError::Unmappable(*first)) }
    }

    /// [`Self::apply_character_references`] for JSON: a character in a string is written as a `\\uXXXX` escape (two for a character
    /// outside the Basic Multilingual Plane); anywhere else (a comment, say) it cannot be, and is the error.
    fn apply_json_escapes(&mut self, text: &str, found: &[EncodeError], dialect: xmlcraft_json_syntax::Dialect) -> Result<usize, SaveError> {
        let Some(first) = found.first() else { return Ok(0) };
        let offsets: Vec<usize> = found.iter().map(|e| e.offset).collect();
        let inside = xmlcraft_json_syntax::inside_strings(text, dialect, &offsets);
        let mut edits = Vec::with_capacity(found.len());
        for (e, in_string) in found.iter().zip(inside) {
            if !in_string {
                return Err(SaveError::Unmappable(*e));
            }
            let mut units = [0u16; 2];
            let escaped: String = e.ch.encode_utf16(&mut units).iter().map(|u| format!("\\u{u:04X}")).collect();
            edits.push(Edit::replace(e.offset..e.offset + e.ch.len_utf8(), escaped));
        }
        if self.editor.apply_edits(&edits) { Ok(edits.len()) } else { Err(SaveError::Unmappable(*first)) }
    }

    /// Get ready to be written under `path`, which may be a name of another language (Save As): a JSON document that becomes XML in an
    /// encoding the bytes do not tell gets the XML declaration that says so. Call it before [`Self::to_bytes`].
    ///
    /// Nothing is lost if the save then fails: pass the result to [`Self::undo_save_as`] and the edit is taken back.
    pub fn prepare_save_as(&mut self, path: &Path) -> SaveAsPrep {
        let target = path.file_name().map_or(Language::Xml, |n| Language::of_name(&n.to_string_lossy()));
        let before = self.editor.version();
        if target == Language::Xml && self.language() != Language::Xml {
            self.sync_declaration_for(Language::Xml);
        }
        SaveAsPrep { edited: (self.editor.version() != before).then(|| self.editor.version()) }
    }

    /// The save that [`Self::prepare_save_as`] was for did not happen: take back what it wrote, if nothing has been edited since.
    pub fn undo_save_as(&mut self, prep: SaveAsPrep) {
        if prep.edited == Some(self.editor.version()) {
            self.editor.undo();
        }
    }

    /// The bytes of the file: the text in the document's encoding. A character the encoding cannot hold is an error (see
    /// [`Self::apply_character_references`] and [`Self::set_encoding`]); nothing is ever replaced silently.
    pub fn to_bytes(&self) -> Result<Vec<u8>, SaveError> {
        if self.language() == Language::Bson {
            return self.bson_bytes();
        }
        encode_chunks(self.editor.buffer().chunks(), self.encoding.encoding, self.encoding.bom, Unmappable::Refuse).map_err(SaveError::Unmappable)
    }

    /// The bytes of a BSON file: the documents the text stands for. A document that is the same as the one that was read at the same
    /// place is written as the exact bytes it was read from (so nothing changes in what nobody edited); the others are encoded.
    fn bson_bytes(&self) -> Result<Vec<u8>, SaveError> {
        let text = self.editor.text();
        let documents = xmlcraft_bson::from_text(&text).map_err(|errors| {
            let first = errors
                .first()
                .map(|e| format!("line {}: {}", self.editor.buffer().line_of(e.offset.min(self.editor.buffer().len())) + 1, e.message))
                .unwrap_or_default();
            SaveError::Invalid(format!("the text is not valid Extended JSON ({first})"))
        })?;
        let mut out = Vec::new();
        for (i, document) in documents.iter().enumerate() {
            let encoded = xmlcraft_bson::write_all(std::slice::from_ref(document)).map_err(|e| SaveError::Invalid(e.message))?;
            // Unchanged is: the text of the document is the text of the one that was read there (what the text cannot say, such as the
            // payload of a NaN, is then not lost).
            let shown = |d: &xmlcraft_bson::Document| xmlcraft_bson::to_text(std::slice::from_ref(d), true).ok().map(|(t, _)| t);
            let unchanged = self.bson.as_ref().and_then(|b| b.get(i)).filter(|(old, _)| shown(old).is_some() && shown(old) == shown(document));
            match unchanged {
                Some((_, original)) => out.extend_from_slice(original),
                None => out.extend_from_slice(&encoded),
            }
        }
        Ok(out)
    }

    /// Whether saving over the file the document came from is allowed.
    pub fn can_overwrite_source(&self) -> bool {
        !self.lossy
    }

    /// The document was written to `path` (the same or a new one): it is clean, has that name, and no longer has bytes that were lost
    /// (the text now is what the file holds).
    pub fn mark_saved(&mut self, path: PathBuf, stamp: Option<FileStamp>) {
        self.name = path.file_name().map_or_else(|| self.name.clone(), |n| n.to_string_lossy().into_owned());
        self.path = Some(path);
        self.editor.set_syntax(syntax_of(self.language()));
        self.stamp = stamp;
        self.saved_state = self.editor.state_id();
        self.saved_encoding = (self.encoding.encoding, self.encoding.bom);
        self.lossy = false;
        self.editor.break_undo_group();
        // What was written is what a BSON file is now made of (worked out while the old documents are still there, so that those that
        // nobody edited keep the exact bytes they had).
        if self.language() == Language::Bson {
            self.bson = self.bson_bytes().ok().and_then(|bytes| {
                xmlcraft_bson::read_with_ranges(&bytes)
                    .ok()
                    .map(|read| read.into_iter().map(|(d, r)| (d, bytes.get(r).unwrap_or_default().to_vec())).collect())
            });
        } else {
            self.bson = None;
        }
    }

    /// Replace the text with the contents of the file as they are now on disk (revert, or accepting a change made elsewhere). The undo
    /// history is dropped.
    pub fn reload(&mut self, bytes: &[u8], stamp: Option<FileStamp>) -> Result<(), OpenError> {
        let fresh = Self::from_bytes(self.id, self.path.clone(), bytes, stamp)?;
        let before = self.editor.version();
        *self = fresh;
        // Versions only go up, so nothing that was worked out about the old text can pass for the new one's.
        self.editor.raise_version_above(before);
        Ok(())
    }
}

/// Whether the text starts with an XML declaration (`<?xml` and white space).
fn has_declaration(head: &str) -> bool {
    head.strip_prefix("<?xml").and_then(|rest| rest.chars().next()).is_some_and(|c| matches!(c, ' ' | '\t' | '\r' | '\n'))
}

/// Where the `version` value of the XML declaration ends (just after its closing quote), if the declaration starts with one.
fn version_end(head: &str) -> Option<usize> {
    let rest = head.strip_prefix("<?xml")?;
    let after_space = rest.trim_start_matches([' ', '\t', '\r', '\n']);
    let after_name = after_space.strip_prefix("version")?.trim_start_matches([' ', '\t', '\r', '\n']);
    let after_eq = after_name.strip_prefix('=')?.trim_start_matches([' ', '\t', '\r', '\n']);
    let quote = after_eq.chars().next().filter(|q| matches!(q, '"' | '\''))?;
    let value = after_eq.get(1..)?;
    let close = value.find(quote)?;
    // The offset of the closing quote's end: everything consumed so far plus the value and the quote.
    Some(head.len() - value.len() + close + 1)
}

/// Editor settings for a text: its dominant line ending and its indentation.
fn settings_for(text: &str) -> Settings {
    let endings = LineEndings::count(text);
    let (use_tabs, indent_size) = detect_indent(text);
    Settings { eol: endings.dominant(), use_tabs, indent_size, ..Settings::default() }
}

/// Tabs or spaces, and how many spaces, judged from the first thousand lines: the most common step by which indentation deepens.
fn detect_indent(text: &str) -> (bool, usize) {
    let (mut tabs, mut spaces) = (0usize, 0usize);
    let mut steps = [0usize; 9];
    let mut previous = 0usize;
    for line in text.lines().take(1000) {
        if line.trim().is_empty() {
            continue;
        }
        let leading = line.len() - line.trim_start_matches([' ', '\t']).len();
        if line.starts_with('\t') {
            tabs += 1;
        } else if leading > 0 {
            spaces += 1;
            if leading > previous && leading - previous <= 8 {
                steps[leading - previous] += 1;
            }
        }
        previous = if line.starts_with('\t') { 0 } else { leading };
    }
    let best = steps.iter().enumerate().skip(1).max_by_key(|(_, count)| **count).filter(|(_, count)| **count > 0).map_or(2, |(size, _)| size);
    (tabs > spaces, best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn open(bytes: &[u8]) -> Document {
        Document::from_bytes(1, Some(PathBuf::from("/tmp/a.xml")), bytes, None).expect("opens")
    }

    #[test]
    fn an_unedited_file_is_written_back_identically() {
        for bytes in [
            &b"<a/>"[..],
            b"<a>\r\n  <b/>\r\n</a>\r\n",
            b"\xEF\xBB\xBF<a>h\xC3\xA9</a>",
            b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a>h\xE9llo</a>",
            b"\xFF\xFE<\0a\0/\0>\0",
            b"<a>\r<b/>\n\r\n</a>",
            b"",
        ] {
            let d = open(bytes);
            assert!(!d.is_lossy() && !d.is_dirty());
            assert_eq!(d.to_bytes().expect("encodes"), bytes, "{bytes:?}");
        }
    }

    proptest! {
        /// For any bytes at all: the document is flagged lossy, or saving gives back exactly the bytes that were read.
        #[test]
        fn open_then_save_is_the_identity_or_flagged(bytes in proptest::collection::vec(any::<u8>(), 0..300), prefix in 0usize..4) {
            let prefixes: [&[u8]; 4] = [b"", b"\xEF\xBB\xBF", b"<?xml version=\"1.0\" encoding=\"windows-1252\"?>", b"\xFF\xFE"];
            let mut input = prefixes[prefix].to_vec();
            input.extend_from_slice(&bytes);
            let d = open(&input);
            if !d.is_lossy() {
                prop_assert_eq!(d.to_bytes().expect("encodes"), input);
            } else {
                prop_assert!(!d.can_overwrite_source());
            }
        }
    }

    #[test]
    fn editing_makes_it_dirty_and_saving_makes_it_clean() {
        let mut d = open(b"<a/>");
        assert!(!d.is_dirty());
        d.editor_mut().type_text("x");
        assert!(d.is_dirty());
        assert_eq!(d.to_bytes().expect("encodes"), b"x<a/>");
        d.mark_saved(PathBuf::from("/tmp/b.xml"), None);
        assert!(!d.is_dirty());
        assert_eq!((d.name(), d.path()), ("b.xml", Some(Path::new("/tmp/b.xml"))));
        d.editor_mut().undo();
        assert!(d.is_dirty(), "undoing past the saved state is a change");
    }

    #[test]
    fn lossy_files_cannot_be_saved_over_but_can_be_saved_elsewhere() {
        let mut d = open(b"<a>\xFF</a>");
        assert!(d.is_lossy() && !d.can_overwrite_source());
        d.mark_saved(PathBuf::from("/tmp/elsewhere.xml"), None);
        assert!(!d.is_lossy() && d.can_overwrite_source(), "the new file holds the text as it is");
    }

    #[test]
    fn a_character_the_encoding_cannot_hold_is_reported_and_becomes_a_reference_in_the_text() {
        let mut d = open(b"<?xml version=\"1.0\" encoding=\"windows-1252\"?><a b=\"x\">t</a>");
        let at = d.editor().buffer().len() - 4;
        d.editor_mut().place_caret(at, false);
        d.editor_mut().type_text("日");
        assert!(matches!(d.to_bytes(), Err(SaveError::Unmappable(e)) if e.ch == '日'));
        d.mark_saved(PathBuf::from("/tmp/a.xml"), None);
        let in_value = d.editor().text().find("x\"").unwrap_or(0);
        d.editor_mut().place_caret(in_value, false);
        d.editor_mut().type_text("€日");
        // The euro sign is in windows-1252; only the other character needs a reference, in the text and in the attribute value.
        assert_eq!(d.unmappable_characters().len(), 2);
        assert_eq!(d.apply_character_references(), Ok(2));
        assert!(d.editor().text().contains("b=\"€&#x65E5;x\"") && d.editor().text().contains("&#x65E5;</a>"), "{}", d.editor().text());
        // The text is what the file will hold: saving gives exactly it, and the document is dirty because the text changed.
        assert!(d.is_dirty());
        let bytes = d.to_bytes().expect("now it encodes");
        assert!(bytes.windows(8).any(|w| w == b"&#x65E5;") && bytes.contains(&0x80));
        // One undo step takes the references out again.
        d.editor_mut().undo();
        assert_eq!(d.unmappable_characters().len(), 2);
        assert_eq!(d.apply_character_references(), Ok(2));
        assert_eq!(d.apply_character_references(), Ok(0), "nothing left to replace");
    }

    #[test]
    fn a_character_reference_is_not_written_where_it_would_be_literal() {
        for (src, bad) in [
            ("<a><!-- 日 --></a>", "in a comment"),
            ("<a><![CDATA[日]]></a>", "in a CDATA section"),
            ("<a><?pi 日?></a>", "in a processing instruction"),
            ("<日/>", "in a name"),
        ] {
            let mut d = open(b"<?xml version=\"1.0\" encoding=\"windows-1252\"?><a/>");
            // Character data next to the construct could take a reference, but it is all or nothing.
            let with_text = format!("<?xml version=\"1.0\" encoding=\"windows-1252\"?>{}", src.replacen("<a>", "<a>日", 1));
            d.editor_mut().load(&with_text);
            let result = d.apply_character_references();
            assert!(matches!(result, Err(SaveError::Unmappable(e)) if e.ch == '日'), "{bad}: {result:?}");
            assert_eq!(d.editor().text(), with_text, "{bad}: nothing changed");
        }
    }

    #[test]
    fn changing_the_encoding_counts_as_a_change_until_it_is_the_same_again() {
        let mut d = open(b"<a>\xC3\xA9</a>");
        assert!(d.set_encoding(Encoding::UTF_16LE, true));
        assert!(d.is_dirty());
        assert_eq!(d.to_bytes().expect("encodes")[..2], [0xFF, 0xFE]);
        // Back to what the file has: clean again, with nothing saved in between.
        assert!(d.set_encoding(Encoding::UTF_8, false));
        assert!(!d.is_dirty(), "UTF-8 without a mark is what was read");
        assert!(d.set_encoding(Encoding::UTF_16LE, true));
        d.mark_saved(PathBuf::from("/tmp/a.xml"), None);
        assert!(!d.is_dirty());
        assert!(!d.set_encoding(Encoding::UTF_16LE, true), "the same encoding is no change");
        assert!(d.set_encoding(Encoding::WINDOWS_1252, true));
        assert!(!d.encoding().bom, "only UTF-8 and UTF-16 have a byte-order mark");
        assert!(!d.set_encoding(Encoding::WINDOWS_1252, true), "asking for a mark it cannot have is no change either");
        assert!(d.is_dirty());
    }

    #[test]
    fn a_mark_that_the_encoding_cannot_have_does_not_dirty_a_document() {
        let mut d = open(b"<?xml version=\"1.0\" encoding=\"windows-1252\"?><a/>");
        assert!(!d.set_encoding(Encoding::WINDOWS_1252, true));
        assert!(!d.is_dirty());
    }

    #[test]
    fn the_xml_declaration_follows_the_encoding() {
        // A declaration that names the old encoding is edited, keeping its quotes.
        let mut d = open(b"<?xml version=\"1.0\" encoding='ISO-8859-1' standalone=\"yes\"?>\n<a>\xE9</a>");
        assert!(d.set_encoding(Encoding::UTF_8, false));
        assert_eq!(d.editor().text(), "<?xml version=\"1.0\" encoding='UTF-8' standalone=\"yes\"?>\n<a>é</a>");
        assert_eq!(d.encoding().declared.as_deref(), Some("UTF-8"));
        assert!(d.encoding().declaration_matches());
        // A declaration that already names it (in any case) is left alone.
        let mut d = open(b"<?xml version=\"1.0\" encoding=\"utf-8\"?><a/>");
        assert!(d.set_encoding(Encoding::UTF_8, true));
        assert_eq!(d.editor().text(), "<?xml version=\"1.0\" encoding=\"utf-8\"?><a/>");
        // Plain `UTF-16` names both byte orders.
        let mut d = open(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><a/>");
        d.set_encoding(Encoding::UTF_16BE, true);
        assert!(d.editor().text().starts_with("<?xml version=\"1.0\" encoding=\"UTF-16\"?>"));
        // No `encoding` in the declaration, or no declaration: UTF-8 and UTF-16 with a mark need none; the others do.
        let mut d = open(b"<?xml version=\"1.0\"?>\r\n<a/>");
        d.set_encoding(Encoding::UTF_16LE, true);
        assert_eq!(d.editor().text(), "<?xml version=\"1.0\"?>\r\n<a/>");
        d.set_encoding(Encoding::WINDOWS_1252, false);
        assert_eq!(d.editor().text(), "<?xml version=\"1.0\" encoding=\"windows-1252\"?>\r\n<a/>");
        let mut d = open(b"<a/>");
        d.set_encoding(Encoding::UTF_16LE, true);
        assert_eq!(d.editor().text(), "<a/>");
        d.set_encoding(Encoding::UTF_16LE, false);
        assert_eq!(d.editor().text(), "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n<a/>", "UTF-16 without a mark must say so");
        // And it round-trips: the file written in the new encoding is read back as that encoding.
        let bytes = d.to_bytes().expect("encodes");
        assert_eq!(sniff(&bytes).encoding, Encoding::UTF_16LE);
    }

    #[test]
    fn undoing_back_to_the_saved_text_is_clean() {
        let mut d = open(b"<a/>");
        d.editor_mut().type_text("x");
        d.editor_mut().undo();
        assert!(!d.is_dirty(), "undone before any save");
        d.editor_mut().redo();
        assert!(d.is_dirty());
        d.mark_saved(PathBuf::from("/tmp/a.xml"), None);
        assert!(!d.is_dirty());
        d.editor_mut().type_text("y");
        d.editor_mut().break_undo_group();
        d.editor_mut().type_text("z");
        assert!(d.is_dirty());
        d.editor_mut().undo();
        assert!(d.is_dirty(), "one step back is still not the saved text");
        d.editor_mut().undo();
        assert!(!d.is_dirty(), "two steps back is");
        d.editor_mut().undo();
        assert!(d.is_dirty(), "before the saved text is a change");
        d.editor_mut().redo();
        assert!(!d.is_dirty());
        // A different edit after undoing is not the saved text.
        d.editor_mut().type_text("q");
        d.editor_mut().undo();
        assert!(!d.is_dirty());
        d.editor_mut().type_text("q");
        assert!(d.is_dirty());
    }

    #[test]
    fn new_lines_use_the_files_line_ending_and_its_indentation() {
        let mut d = open(b"<a>\r\n    <b>\r\n        <c/>\r\n    </b>\r\n</a>\r\n");
        assert_eq!(d.editor().settings().eol, xmlcraft_text::eol::Eol::CrLf);
        assert_eq!((d.editor().settings().use_tabs, d.editor().settings().indent_size), (false, 4));
        d.editor_mut().place_caret(3, false);
        d.editor_mut().insert_newline();
        assert!(d.editor().text().starts_with("<a>\r\n    \r\n"), "after a start tag: the file's line ending and one more level of its indentation");
        let d = open(b"<a>\n\t<b>\n\t\t<c/>\n\t</b>\n</a>\n");
        assert!(d.editor().settings().use_tabs);
        let d = open(b"<a/>");
        assert_eq!((d.editor().settings().use_tabs, d.editor().settings().indent_size), (false, 2), "no evidence: two spaces");
    }

    #[test]
    fn the_line_ending_count_follows_the_text_but_is_counted_once_per_version() {
        let mut d = open(b"a\nb\nc");
        assert_eq!((d.line_endings().lf, d.line_endings().crlf), (2, 0));
        d.editor_mut().apply_edits(&[Edit::insert(0, "x\r\ny\r\n")]); // as a formatter would: no conversion to the document's ending
        assert_eq!((d.line_endings().lf, d.line_endings().crlf), (2, 2), "an edit is counted");
        assert!(d.line_endings().is_mixed());
        d.editor_mut().undo();
        assert_eq!((d.line_endings().lf, d.line_endings().crlf), (2, 0), "and so is undoing it");
    }

    #[test]
    fn pretty_printing_is_one_undoable_edit_that_follows_the_documents_style() {
        let mut d = open(b"<a>\r\n    <b><c/></b>\r\n</a>");
        // The document indents by four spaces and ends its lines with CRLF; that is how it is formatted.
        assert_eq!(d.pretty_print(), Ok(true));
        assert_eq!(d.editor().text(), "<a>\r\n    <b>\r\n        <c/>\r\n    </b>\r\n</a>");
        assert!(d.is_dirty());
        d.editor_mut().undo();
        assert_eq!(d.editor().text(), "<a>\r\n    <b><c/></b>\r\n</a>", "one undo puts it all back");
        assert!(!d.is_dirty(), "and the document is as saved");
        assert_eq!(d.pretty_print(), Ok(true));
        assert_eq!(d.pretty_print(), Ok(false), "formatted already: nothing to do, nothing recorded");
        assert_eq!(d.minify(), Ok(true));
        assert_eq!(d.editor().text(), "<a><b><c/></b></a>");
        // Broken markup is refused and the text is untouched.
        let mut broken = open(b"<a><b></a>");
        assert!(matches!(broken.pretty_print(), Err(FormatError::NotWellFormed { .. })));
        assert!(matches!(broken.minify(), Err(FormatError::NotWellFormed { .. })));
        assert_eq!((broken.editor().text().as_str(), broken.is_dirty()), ("<a><b></a>", false));
        // Tabs when the document uses them.
        let mut tabs = open(b"<a>\n\t<b>\n\t\t<c/></b>\n</a><!--x-->");
        assert_eq!(tabs.pretty_print(), Ok(true));
        assert_eq!(tabs.editor().text(), "<a>\n\t<b>\n\t\t<c/>\n\t</b>\n</a>\n<!--x-->");
    }

    #[test]
    fn a_reload_never_reuses_a_version_the_old_text_had() {
        let mut d = open(b"<a/>");
        d.editor_mut().type_text("x");
        d.editor_mut().type_text("y");
        let before = d.editor().version();
        d.reload(b"<b/>", None).expect("reloads");
        assert!(d.editor().version() > before, "{} after {before}", d.editor().version());
        assert_eq!(d.editor().text(), "<b/>");
        assert!(!d.is_dirty());
    }

    #[test]
    fn indentation_detection() {
        assert_eq!(detect_indent("a\n  b\n    c\n  d\ne"), (false, 2));
        assert_eq!(detect_indent("a\n   b\n      c\n"), (false, 3));
        assert_eq!(detect_indent("a\n\tb\n\t\tc\n"), (true, 2));
        assert_eq!(detect_indent(""), (false, 2));
        assert_eq!(detect_indent("a\n b\n  c\n   d\n"), (false, 1));
    }

    #[test]
    fn a_huge_file_is_refused_not_attempted() {
        let big = vec![b'a'; MAX_OPEN_BYTES + 1];
        let result = Document::from_bytes(1, None, &big, None);
        assert!(matches!(result, Err(OpenError::TooLarge { .. })));
    }

    #[test]
    fn keeping_ones_version_remembers_the_stamp_and_changes_nothing_else() {
        let mut d = open(b"<a/>");
        d.editor_mut().type_text("x");
        let stamp = FileStamp { len: 9, modified: None, changed: None, file_id: None };
        d.set_stamp(Some(stamp));
        assert_eq!(d.stamp(), Some(&stamp));
        assert!(d.is_dirty(), "the text is still not saved");
        assert_eq!(d.editor().text(), "x<a/>");
    }

    #[test]
    fn reloading_replaces_the_text_and_drops_the_history() {
        let mut d = open(b"<a/>");
        d.editor_mut().type_text("x");
        d.reload(b"<b/>", None).expect("reloads");
        assert_eq!(d.editor().text(), "<b/>");
        assert!(!d.is_dirty() && !d.editor().can_undo());
        assert_eq!(d.path(), Some(Path::new("/tmp/a.xml")));
    }

    #[test]
    fn a_document_links_the_tag_names_the_caret_is_in_without_an_analysis_at_hand() {
        let mut d = open(b"<a><b>x</b></a>");
        d.editor_mut().set_selection(5, 5);
        assert!(d.link_tag_names());
        d.editor_mut().type_input("c");
        assert_eq!(d.editor().text(), "<a><bc>x</bc></a>");
        // Not in a name, with a selection: nothing is linked (and an earlier link is dropped).
        d.editor_mut().set_selection(7, 7);
        assert!(!d.link_tag_names());
        assert_eq!(d.editor().linked_names(), None);
        d.editor_mut().set_selection(4, 6);
        assert!(!d.link_tag_names());
        // A call while the caret is still in a linked name keeps the link without analysing again.
        d.editor_mut().set_selection(5, 5);
        assert!(d.link_tag_names() && d.link_tag_names());
    }

    #[test]
    fn a_text_too_large_to_analyse_or_a_caret_outside_a_tag_is_not_worth_an_analysis() {
        let text = format!("<a>{}</a>", "x".repeat(crate::analysis::MAX_ANALYSED_BYTES));
        let mut d = open(text.as_bytes());
        d.editor_mut().set_selection(2, 2);
        assert!(!d.link_tag_names(), "too large: no pairs are known");
        assert_eq!(d.editor().linked_names(), None);
        // In text: no tag, so nothing to link.
        let mut d = open(b"<a>text</a>");
        d.editor_mut().set_selection(5, 5);
        assert!(!d.link_tag_names());
    }

    #[test]
    fn pretty_printing_a_document_that_mixes_line_endings_settles_in_one_pass() {
        // Found by the fuzzer: five breaks of each ending, the CRLF ones in a comment; the blank lines that go are LF.
        let mut d = open(b"<?l?><!--\r\n\r\n\r\n\r\n\r\n-->\n<foo xmlns:a=\"r\">\n\n<a a=\"\"/>\n\n</foo>");
        assert_eq!(d.pretty_print(), Ok(true));
        assert_eq!(d.pretty_print(), Ok(false), "{:?}", d.editor().text());
    }

    #[test]
    fn untitled_documents_are_numbered() {
        let d = Document::untitled(7, 3);
        assert_eq!((d.id(), d.name(), d.path().is_none(), d.is_dirty()), (7, "Untitled-3", true, false));
    }

    #[test]
    fn the_name_of_a_document_decides_how_typing_and_tag_links_treat_it() {
        let json = Document::from_bytes(1, Some(PathBuf::from("/d/a.json")), b"<a></a>", None).expect("opens");
        assert_eq!((json.language(), json.editor().syntax()), (Language::Json(crate::language::JsonFlavor::Json), Syntax::Json));
        let mut json = json;
        json.editor_mut().set_selection(2, 2);
        assert!(!json.link_tag_names(), "text that looks like a tag in a JSON file has no tag names to keep in step");
        let mut d = Document::untitled(2, 1);
        assert_eq!(d.editor().syntax(), Syntax::Xml);
        d.name_untitled("x.jsonc");
        assert_eq!(d.editor().syntax(), Syntax::Json, "a generated document called x.jsonc is JSON");
        d.mark_saved(PathBuf::from("/d/y.xml"), None);
        assert_eq!((d.language(), d.editor().syntax()), (Language::Xml, Syntax::Xml), "Save As under another extension changes how it is read");
    }

    #[test]
    fn a_json_document_has_no_xml_declaration_and_its_unmappable_characters_become_escapes_in_strings_only() {
        let json = |text: &str| Document::from_bytes(1, Some(PathBuf::from("/d/a.json")), text.as_bytes(), None).expect("opens");
        let mut d = json("{\"a\": \"日 \u{1F600}\"}");
        assert!(d.set_encoding(Encoding::WINDOWS_1252, false));
        assert_eq!(d.editor().text(), "{\"a\": \"日 \u{1F600}\"}", "no declaration is written into JSON");
        assert_eq!(d.apply_character_references(), Ok(2));
        assert_eq!(d.editor().text(), "{\"a\": \"\\u65E5 \\uD83D\\uDE00\"}", "an escape per UTF-16 unit");
        assert_eq!(d.to_bytes().expect("now it fits"), d.editor().text().into_bytes());
        // Outside a string there is no way to write it: the character is the error.
        let mut d = json("{ /* 日 */ \"a\": 1}");
        d.set_encoding(Encoding::WINDOWS_1252, false);
        assert!(matches!(d.apply_character_references(), Err(SaveError::Unmappable(_))));
        assert_eq!(d.editor().text(), "{ /* 日 */ \"a\": 1}", "and nothing was changed");
    }

    #[test]
    fn a_json_document_saved_as_xml_in_an_encoding_the_bytes_do_not_tell_gets_the_declaration() {
        let mut d = Document::from_bytes(1, Some(PathBuf::from("/d/a.json")), b"{\"a\": 1}", None).expect("opens");
        d.set_encoding(Encoding::WINDOWS_1252, false);
        assert_eq!(d.editor().text(), "{\"a\": 1}");
        d.prepare_save_as(Path::new("/d/b.json"));
        assert_eq!(d.editor().text(), "{\"a\": 1}", "a name of the same language changes nothing");
        d.prepare_save_as(Path::new("/d/b.xml"));
        assert!(d.editor().text().starts_with("<?xml version=\"1.0\" encoding=\"windows-1252\"?>"), "{}", d.editor().text());
    }

    #[test]
    fn a_save_as_that_does_not_happen_takes_back_the_declaration_it_wrote() {
        let mut d = Document::from_bytes(1, Some(PathBuf::from("/d/a.json")), b"{\"a\": 1}", None).expect("opens");
        d.set_encoding(Encoding::WINDOWS_1252, false);
        let prep = d.prepare_save_as(Path::new("/d/b.xml"));
        assert!(d.editor().text().starts_with("<?xml"));
        d.undo_save_as(prep);
        assert_eq!(d.editor().text(), "{\"a\": 1}", "as it was");
        // Nothing to take back when nothing was written, and nothing is undone that the user did since.
        let none = d.prepare_save_as(Path::new("/d/c.json"));
        d.editor_mut().set_selection(0, 0);
        d.editor_mut().type_text("x");
        d.undo_save_as(none);
        assert_eq!(d.editor().text(), "x{\"a\": 1}");
        let prep = d.prepare_save_as(Path::new("/d/b.xml"));
        d.editor_mut().type_text("y");
        d.undo_save_as(prep);
        assert!(d.editor().text().contains('y'), "an edit made since is not undone");
    }

    fn bson_bytes(documents: &[xmlcraft_bson::Document]) -> Vec<u8> {
        xmlcraft_bson::write_all(documents).expect("writes")
    }

    fn two_documents() -> Vec<xmlcraft_bson::Document> {
        use xmlcraft_bson::{Bson, Document};
        vec![
            Document { fields: vec![("n".into(), Bson::Int32(1)), ("odd".into(), Bson::Double(f64::from_bits(0x7ff8_0000_0000_0001)))] },
            Document { fields: vec![("s".into(), Bson::String("x".into()))] },
        ]
    }

    #[test]
    fn a_bson_file_is_shown_as_text_and_saved_as_the_bytes_it_came_from_where_nobody_edited() {
        let bytes = bson_bytes(&two_documents());
        let mut d = Document::from_bytes(1, Some(PathBuf::from("/d/a.bson")), &bytes, None).expect("opens");
        assert_eq!((d.language(), d.editor().syntax()), (Language::Bson, Syntax::Json));
        assert!(d.editor().text().contains("\"$numberInt\":\"1\"") && d.editor().text().lines().count() == 2, "{}", d.editor().text());
        assert_eq!(d.to_bytes().expect("saves"), bytes, "nothing edited: the same bytes, whatever the text cannot say (a NaN payload)");
        // Edit the second document only: the first keeps its exact bytes.
        let text = d.editor().text().replace("\"x\"", "\"y\"");
        d.editor_mut().select_all();
        d.editor_mut().type_text(&text);
        let saved = d.to_bytes().expect("saves");
        let first = bson_bytes(&two_documents()[..1]);
        assert_eq!(&saved[..first.len()], first.as_slice(), "the unedited document is byte for byte what it was");
        assert_ne!(saved, bytes);
        let read = xmlcraft_bson::read_all(&saved).expect("reads");
        assert_eq!(read[1].fields[0].1, xmlcraft_bson::Bson::String("y".into()));
        // After saving, what was written is the new baseline.
        d.mark_saved(PathBuf::from("/d/a.bson"), None);
        assert_eq!(d.to_bytes().expect("saves"), saved);
    }

    #[test]
    fn text_that_is_not_extended_json_is_not_saved_and_a_file_that_is_not_bson_is_not_opened() {
        let bytes = bson_bytes(&two_documents());
        let mut d = Document::from_bytes(1, Some(PathBuf::from("/d/a.bson")), &bytes, None).expect("opens");
        d.editor_mut().select_all();
        d.editor_mut().type_text("{\"a\": {\"$oid\": \"zz\"}}");
        let Err(SaveError::Invalid(why)) = d.to_bytes() else { panic!("refused") };
        assert!(why.contains("Extended JSON") && why.contains("line 1"), "{why}");
        let Err(OpenError::Invalid(why)) = Document::from_bytes(2, Some(PathBuf::from("/d/b.bson")), b"not bson at all", None) else {
            panic!("refused")
        };
        assert!(why.contains("not a BSON file"), "{why}");
        // An empty file is an empty document list; a text document saved under a .bson name is encoded.
        let empty = Document::from_bytes(3, Some(PathBuf::from("/d/c.bson")), b"", None).expect("opens");
        assert_eq!(empty.to_bytes().expect("saves"), Vec::<u8>::new());
        let mut text = Document::untitled(4, 1);
        text.editor_mut().type_text("{\"k\": {\"$numberLong\": \"7\"}}");
        text.mark_saved(PathBuf::from("/d/new.bson"), None);
        assert_eq!(xmlcraft_bson::read_all(&text.to_bytes().expect("saves")).expect("reads")[0].fields[0].1, xmlcraft_bson::Bson::Int64(7));
        // And a BSON document saved as .json is plain text.
        let mut d = Document::from_bytes(5, Some(PathBuf::from("/d/a.bson")), &bytes, None).expect("opens");
        d.mark_saved(PathBuf::from("/d/a.json"), None);
        assert_eq!(d.to_bytes().expect("saves"), d.editor().text().into_bytes());
    }
}
