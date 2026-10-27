//! One open document: its text and editor, how its file is encoded, and whether it has changes that are not saved.
//!
//! The text is the model (CLAUDE.md): opening a file and saving it without editing writes the same bytes, whatever the encoding, byte
//! order mark and line endings. Where reading had to replace something (bytes that are not valid in the encoding), the document says so
//! and refuses to overwrite the file it came from, so nothing is lost without the user choosing it.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use xmlcraft_editor::{Editor, Settings};
use xmlcraft_text::encoding::{Detected, EncodeError, Encoding, Unmappable, decode, encode_chunks, sniff};
use xmlcraft_text::eol::LineEndings;

/// Files larger than this are not opened (until large-file mode, M1.15).
pub const MAX_OPEN_BYTES: usize = 256 * 1024 * 1024;

/// What a file looked like when it was last read or written, to notice that something else changed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
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
    /// A character the file's encoding cannot hold. Writing it as a character reference, or another encoding, is the user's choice.
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
    saved_version: u64,
    /// Set by anything that changes what would be written without changing the text (another encoding).
    forced_dirty: bool,
    stamp: Option<FileStamp>,
}

impl Document {
    /// A new, empty document that has no file yet.
    pub fn untitled(id: u64, number: usize) -> Self {
        Self {
            id,
            name: format!("Untitled-{number}"),
            path: None,
            editor: Editor::new(""),
            encoding: Detected::utf8(),
            lossy: false,
            saved_version: 0,
            forced_dirty: false,
            stamp: None,
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
        Ok(Self { id, name, path, saved_version: editor.version(), editor, encoding, lossy: decoded.lossy, forced_dirty: false, stamp })
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

    /// Whether the text, or how it would be written, differs from what was last saved.
    pub fn is_dirty(&self) -> bool {
        self.forced_dirty || self.editor.version() != self.saved_version
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

    /// The line endings in the text now.
    pub fn line_endings(&self) -> LineEndings {
        LineEndings::count_chunks(self.editor.buffer().chunks())
    }

    /// Write the file in another encoding from now on (with or without a byte-order mark). The document counts as changed.
    pub fn set_encoding(&mut self, encoding: Encoding, bom: bool) {
        if self.encoding.encoding != encoding || self.encoding.bom != bom {
            self.encoding.encoding = encoding;
            self.encoding.bom = bom && (encoding.is_utf8() || encoding.is_utf16());
            self.forced_dirty = true;
        }
    }

    /// The bytes of the file: the text in the document's encoding. With `Unmappable::Refuse`, a character the encoding cannot hold is an
    /// error; with `CharRefs` it is written as a character reference.
    pub fn to_bytes(&self, policy: Unmappable) -> Result<Vec<u8>, SaveError> {
        encode_chunks(self.editor.buffer().chunks(), self.encoding.encoding, self.encoding.bom, policy).map_err(SaveError::Unmappable)
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
        self.saved_version = self.editor.version();
        self.forced_dirty = false;
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
            assert_eq!(d.to_bytes(Unmappable::Refuse).expect("encodes"), bytes, "{bytes:?}");
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
                prop_assert_eq!(d.to_bytes(Unmappable::Refuse).expect("encodes"), input);
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
        assert_eq!(d.to_bytes(Unmappable::Refuse).expect("encodes"), b"x<a/>");
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
    fn a_character_the_encoding_cannot_hold_is_reported() {
        let mut d = open(b"<?xml version=\"1.0\" encoding=\"windows-1252\"?><a/>");
        let at = d.editor().buffer().len() - 4;
        d.editor_mut().place_caret(at, false);
        d.editor_mut().type_text("日");
        assert!(matches!(d.to_bytes(Unmappable::Refuse), Err(SaveError::Unmappable(_))));
        let bytes = d.to_bytes(Unmappable::CharRefs).expect("with references");
        assert!(bytes.windows(8).any(|w| w == b"&#x65E5;"));
    }

    #[test]
    fn changing_the_encoding_counts_as_a_change() {
        let mut d = open(b"<a>\xC3\xA9</a>");
        d.set_encoding(Encoding::UTF_16LE, true);
        assert!(d.is_dirty());
        assert_eq!(d.to_bytes(Unmappable::Refuse).expect("encodes")[..2], [0xFF, 0xFE]);
        d.mark_saved(PathBuf::from("/tmp/a.xml"), None);
        assert!(!d.is_dirty());
        d.set_encoding(Encoding::UTF_16LE, true);
        assert!(!d.is_dirty(), "the same encoding is no change");
        d.set_encoding(Encoding::WINDOWS_1252, true);
        assert!(!d.encoding().bom, "only UTF-8 and UTF-16 have a byte-order mark");
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
