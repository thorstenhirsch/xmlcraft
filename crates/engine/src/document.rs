//! One open document: its text and editor, how its file is encoded, and whether it has changes that are not saved.
//!
//! The text is the model (CLAUDE.md): opening a file and saving it without editing writes the same bytes, whatever the encoding, byte
//! order mark and line endings. Where reading had to replace something (bytes that are not valid in the encoding), the document says so
//! and refuses to overwrite the file it came from, so nothing is lost without the user choosing it.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use xmlcraft_editor::{Editor, Settings};
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
    TooLarge { len: usize, max: usize },
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::TooLarge { len, max } => write!(f, "the file is {len} bytes, more than the {max} bytes XmlCraft opens"),
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
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Unmappable(e) => e.fmt(f),
            SaveError::Lossy => f.write_str("the file has bytes that are not valid in its encoding, and saving would replace them for good"),
        }
    }
}

impl std::error::Error for SaveError {}

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
        }
    }

    /// A document read from the bytes of a file.
    pub fn from_bytes(id: u64, path: Option<PathBuf>, bytes: &[u8], stamp: Option<FileStamp>) -> Result<Self, OpenError> {
        if bytes.len() > MAX_OPEN_BYTES {
            return Err(OpenError::TooLarge { len: bytes.len(), max: MAX_OPEN_BYTES });
        }
        let encoding = sniff(bytes);
        let decoded = decode(bytes, &encoding);
        let settings = settings_for(&decoded.text);
        let mut editor = Editor::with_settings("", settings);
        editor.load(&decoded.text);
        let name = path.as_deref().and_then(Path::file_name).map_or_else(|| "Untitled".to_owned(), |n| n.to_string_lossy().into_owned());
        let saved_encoding = (encoding.encoding, encoding.bom);
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
        })
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// The name shown on the tab: the file's name, or `Untitled-N`.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    pub fn editor_mut(&mut self) -> &mut Editor {
        &mut self.editor
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
    /// one it was opened with, when the text has no line break).
    pub fn format_options(&self) -> FormatOptions {
        let settings = self.editor.settings();
        let indent = if settings.use_tabs { Indent::Tab } else { Indent::Spaces(settings.indent_size) };
        let endings = self.line_endings();
        let eol = if endings.total() == 0 { settings.eol } else { endings.dominant() };
        FormatOptions { indent, eol, blank_lines: BlankLines::Keep }
    }

    /// Pretty-print the text: only the white space between nodes changes (see `xmlcraft-format`), as one undoable edit. Returns whether
    /// anything changed. A document that is not well-formed is refused and left as it is.
    pub fn pretty_print(&mut self) -> Result<bool, FormatError> {
        let edits = xmlcraft_format::pretty_print(&self.editor.text(), &self.format_options())?;
        Ok(self.editor.apply_edits(&edits) && !edits.is_empty())
    }

    /// Remove the white space between nodes that means nothing, as one undoable edit. Returns whether anything changed.
    pub fn minify(&mut self) -> Result<bool, FormatError> {
        let edits = xmlcraft_format::minify(&self.editor.text())?;
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

    /// The bytes of the file: the text in the document's encoding. A character the encoding cannot hold is an error (see
    /// [`Self::apply_character_references`] and [`Self::set_encoding`]); nothing is ever replaced silently.
    pub fn to_bytes(&self) -> Result<Vec<u8>, SaveError> {
        encode_chunks(self.editor.buffer().chunks(), self.encoding.encoding, self.encoding.bom, Unmappable::Refuse).map_err(SaveError::Unmappable)
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
        self.stamp = stamp;
        self.saved_state = self.editor.state_id();
        self.saved_encoding = (self.encoding.encoding, self.encoding.bom);
        self.lossy = false;
        self.editor.break_undo_group();
    }

    /// Replace the text with the contents of the file as they are now on disk (revert, or accepting a change made elsewhere). The undo
    /// history is dropped.
    pub fn reload(&mut self, bytes: &[u8], stamp: Option<FileStamp>) -> Result<(), OpenError> {
        let fresh = Self::from_bytes(self.id, self.path.clone(), bytes, stamp)?;
        *self = fresh;
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
        assert!(d.editor().text().starts_with("<a>\r\n\r\n"));
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
    fn untitled_documents_are_numbered() {
        let d = Document::untitled(7, 3);
        assert_eq!((d.id(), d.name(), d.path().is_none(), d.is_dirty()), (7, "Untitled-3", true, false));
    }
}
