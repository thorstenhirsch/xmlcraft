#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! The engine: what every frontend (the egui app, the CLI, the MCP server, the control channel) drives. It owns the open
//! documents and the command registry, and knows nothing about egui.

pub mod analysis;
pub mod commands;
mod document;

pub use document::{Document, FileStamp, MAX_OPEN_BYTES, OpenError, SaveError};

/// The open documents, and which one is shown.
#[derive(Default)]
pub struct Session {
    documents: Vec<Document>,
    active: Option<usize>,
    next_id: u64,
    untitled: usize,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("documents", &self.documents.iter().map(Document::name).collect::<Vec<_>>())
            .field("active", &self.active)
            .finish()
    }
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn documents(&self) -> &[Document] {
        &self.documents
    }

    pub fn document(&self, index: usize) -> Option<&Document> {
        self.documents.get(index)
    }

    pub fn document_mut(&mut self, index: usize) -> Option<&mut Document> {
        self.documents.get_mut(index)
    }

    /// The index of the document with this id.
    pub fn position_of(&self, id: u64) -> Option<usize> {
        self.documents.iter().position(|d| d.id() == id)
    }

    /// The index of the document being shown, if any.
    pub fn active(&self) -> Option<usize> {
        self.active.filter(|i| *i < self.documents.len())
    }

    pub fn active_document(&self) -> Option<&Document> {
        self.active().and_then(|i| self.documents.get(i))
    }

    pub fn active_document_mut(&mut self) -> Option<&mut Document> {
        let index = self.active()?;
        self.documents.get_mut(index)
    }

    /// Show the document at `index`, or the home view for `None`. An index past the end is ignored.
    pub fn set_active(&mut self, index: Option<usize>) {
        if index.is_none_or(|i| i < self.documents.len()) {
            self.active = index;
        }
    }

    pub fn has_document(&self) -> bool {
        self.active().is_some()
    }

    pub fn can_undo(&self) -> bool {
        self.active_document().is_some_and(|d| d.editor().can_undo())
    }

    pub fn can_redo(&self) -> bool {
        self.active_document().is_some_and(|d| d.editor().can_redo())
    }

    /// Whether any document has changes that are not saved.
    pub fn has_unsaved_changes(&self) -> bool {
        self.documents.iter().any(Document::is_dirty)
    }

    fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// A new empty document, shown. Returns its index.
    pub fn new_document(&mut self) -> usize {
        self.untitled += 1;
        let id = self.next_id();
        self.documents.push(Document::untitled(id, self.untitled));
        let index = self.documents.len() - 1;
        self.active = Some(index);
        index
    }

    /// The document that is the file at `path`, whose stamp is `stamp`: the same path, or (where the platform says which file a stamp is
    /// about) the same file reached by another path, such as through a link or by another spelling.
    fn position_of_file(&self, path: &std::path::Path, stamp: Option<&FileStamp>) -> Option<usize> {
        let file_id = stamp.and_then(|s| s.file_id);
        self.documents
            .iter()
            .position(|d| d.path() == Some(path) || file_id.is_some_and(|id| d.path().is_some() && d.stamp().and_then(|s| s.file_id) == Some(id)))
    }

    /// Open a file from its bytes and show it; a file that is already open (under this path, or another one to the same file) is just
    /// shown. Returns its index.
    pub fn open(&mut self, path: Option<std::path::PathBuf>, bytes: &[u8], stamp: Option<FileStamp>) -> Result<usize, OpenError> {
        if let Some(path) = &path
            && let Some(index) = self.position_of_file(path, stamp.as_ref())
        {
            self.active = Some(index);
            return Ok(index);
        }
        let id = self.next_id();
        self.documents.push(Document::from_bytes(id, path, bytes, stamp)?);
        let index = self.documents.len() - 1;
        self.active = Some(index);
        Ok(index)
    }

    /// Close the document at `index` (the caller has dealt with unsaved changes). The one to its left, or the home view, is shown next.
    pub fn close(&mut self, index: usize) -> Option<Document> {
        if index >= self.documents.len() {
            return None;
        }
        let closed = self.documents.remove(index);
        self.active = if self.documents.is_empty() {
            None
        } else {
            match self.active {
                Some(a) if a == index => Some(index.saturating_sub(1)), // the left neighbour, or the new first document
                Some(a) if a > index => Some(a - 1),
                other => other,
            }
        };
        Some(closed)
    }
}

/// Run `f`, turning a panic into an error message instead of unwinding into the caller. This is the last-resort guard AGENTS.md
/// requires around command execution: the user's documents stay as they were, and the app keeps running. It is a safety net, not a
/// substitute for returning errors.
pub fn guard<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|payload| panic_message(payload.as_ref()))
}

/// The text of a panic payload (`panic!("…")` carries a `&str` or a `String`).
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown error".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_session_has_no_document_and_nothing_to_undo() {
        let s = Session::new();
        assert!(!s.has_document() && s.active().is_none() && s.documents().is_empty());
        assert!(!s.can_undo() && !s.can_redo() && !s.has_unsaved_changes());
    }

    #[test]
    fn opening_showing_and_closing_documents() {
        let mut s = Session::new();
        let a = s.open(Some("/x/a.xml".into()), b"<a/>", None).expect("opens");
        let b = s.new_document();
        assert_eq!((a, b, s.active()), (0, 1, Some(1)));
        assert_eq!(s.documents().iter().map(Document::name).collect::<Vec<_>>(), ["a.xml", "Untitled-1"]);
        // Opening a file that is open shows it instead of opening it twice.
        assert_eq!(s.open(Some("/x/a.xml".into()), b"<changed/>", None), Ok(0));
        assert_eq!((s.documents().len(), s.active()), (2, Some(0)));
        assert_eq!(s.document(0).map(|d| d.editor().text()), Some("<a/>".to_owned()), "the open document is kept as it is");
        s.set_active(Some(9));
        assert_eq!(s.active(), Some(0), "an index past the end is ignored");
        s.set_active(None);
        assert!(!s.has_document());
        // Closing: the neighbour to the left is shown, indexes after it shift.
        s.set_active(Some(1));
        let c = s.new_document();
        assert_eq!(c, 2);
        assert!(s.close(1).is_some());
        assert_eq!((s.active(), s.documents().len()), (Some(1), 2), "the active one moved down with the list");
        assert!(s.close(1).is_some());
        assert_eq!(s.active(), Some(0));
        assert!(s.close(0).is_some() && s.active().is_none() && s.close(0).is_none());
    }

    #[test]
    fn a_file_reached_by_another_path_is_the_same_document() {
        let stamp = |id: Option<(u64, u64)>| Some(FileStamp { len: 4, modified: None, changed: None, file_id: id });
        let mut s = Session::new();
        let first = s.open(Some("/x/a.xml".into()), b"<a/>", stamp(Some((1, 77)))).expect("opens");
        // Another spelling (a link, a different folder name for the same one): the same file, the same document.
        assert_eq!(s.open(Some("/x/link/../a.xml".into()), b"<a/>", stamp(Some((1, 77)))), Ok(first));
        assert_eq!(s.documents().len(), 1);
        // A different file, or one the platform cannot identify, is a document of its own.
        assert_eq!(s.open(Some("/x/b.xml".into()), b"<b/>", stamp(Some((1, 78)))), Ok(1));
        assert_eq!(s.open(Some("/x/other/a.xml".into()), b"<c/>", stamp(None)), Ok(2));
        assert_eq!(s.documents().len(), 3);
        // An untitled document has no file, and never matches one.
        let untitled = s.new_document();
        assert_eq!(s.open(Some("/y/c.xml".into()), b"<c/>", stamp(Some((1, 77)))), Ok(first));
        assert_eq!(s.document(untitled).map(|d| d.path().is_none()), Some(true));
    }

    #[test]
    fn closing_the_first_tab_shows_the_next_one() {
        let mut s = Session::new();
        for _ in 0..3 {
            s.new_document();
        }
        s.set_active(Some(0));
        assert!(s.close(0).is_some());
        assert_eq!((s.active(), s.documents().len()), (Some(0), 2), "the old second tab is now the first and shown");
        s.set_active(Some(1));
        assert!(s.close(1).is_some());
        assert_eq!(s.active(), Some(0), "the left neighbour");
    }

    #[test]
    fn undo_and_unsaved_changes_follow_the_documents() {
        let mut s = Session::new();
        s.new_document();
        assert!(!s.can_undo() && !s.has_unsaved_changes());
        if let Some(d) = s.active_document_mut() {
            d.editor_mut().type_text("x");
        }
        assert!(s.can_undo() && !s.can_redo() && s.has_unsaved_changes());
        if let Some(d) = s.active_document_mut() {
            d.editor_mut().undo();
        }
        assert!(s.can_redo());
        assert_eq!(s.position_of(s.documents()[0].id()), Some(0));
        assert_eq!(s.position_of(999), None);
    }

    #[test]
    #[allow(clippy::panic)]
    fn guard_returns_the_value_when_nothing_goes_wrong() {
        assert_eq!(guard(|| 41 + 1), Ok(42));
    }

    #[test]
    #[allow(clippy::panic)]
    fn guard_turns_a_panic_into_an_error_message() {
        // Keep the panic's report out of the test output.
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let text: Result<(), String> = guard(|| panic!("boom"));
        let formatted: Result<(), String> = guard(|| panic!("{} {}", "formatted", 7));
        let other: Result<(), String> = guard(|| std::panic::panic_any(5_u32));
        std::panic::set_hook(hook);
        assert_eq!(text, Err("boom".to_owned()));
        assert_eq!(formatted, Err("formatted 7".to_owned()));
        assert_eq!(other, Err("unknown error".to_owned()));
    }
}
