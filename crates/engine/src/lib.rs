#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! The engine: what every frontend (the egui app, the CLI, the MCP server, the control channel) drives. It owns the open
//! documents and the command registry, and knows nothing about egui.

pub mod commands;

/// The open documents. Documents arrive with the editor milestone (M1); until then a session is empty.
#[derive(Debug, Default)]
pub struct Session {
    documents: Vec<Document>,
    active: Option<usize>,
}

/// One open document. Filled in by the editor milestone.
#[derive(Debug)]
pub struct Document {
    pub name: String,
    pub dirty: bool,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn documents(&self) -> &[Document] {
        &self.documents
    }

    /// The index of the document being shown, if any.
    pub fn active(&self) -> Option<usize> {
        self.active.filter(|i| *i < self.documents.len())
    }

    pub fn has_document(&self) -> bool {
        self.active().is_some()
    }

    pub fn can_undo(&self) -> bool {
        false
    }

    pub fn can_redo(&self) -> bool {
        false
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
        assert!(!s.can_undo() && !s.can_redo());
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
