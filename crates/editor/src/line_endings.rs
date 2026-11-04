//! Converting the line endings of a text.

use xmlcraft_text::Edit;
use xmlcraft_text::eol::Eol;

use crate::Editor;

impl Editor {
    /// Write every line break of the text as `eol` (a lone `\r` counts as a line break here, as in the XML specification): one undoable
    /// step that changes only the breaks that are something else. The setting for new breaks follows. Returns how many breaks were
    /// changed (none when the text has no break that differs; the setting still follows).
    pub fn convert_line_endings(&mut self, eol: Eol) -> usize {
        let text = self.text();
        let bytes = text.as_bytes();
        let want = eol.as_str();
        let mut edits = Vec::new();
        let mut i = 0;
        while let Some(&b) = bytes.get(i) {
            let end = match (b, bytes.get(i + 1)) {
                (b'\r', Some(b'\n')) => i + 2,
                (b'\r' | b'\n', _) => i + 1,
                _ => {
                    i += 1;
                    continue;
                }
            };
            if text.get(i..end) != Some(want) {
                edits.push(Edit::replace(i..end, want));
            }
            i = end;
        }
        let changed = edits.len();
        if changed > 0 && !self.apply_edits(&edits) {
            return 0;
        }
        let mut settings = self.settings().clone();
        settings.eol = eol;
        self.set_settings(settings);
        changed
    }
}

#[cfg(test)]
mod eol_tests {
    use super::*;
    use xmlcraft_text::eol::LineEndings;

    #[test]
    fn only_the_breaks_that_differ_are_changed_in_one_undoable_step() {
        let mut e = Editor::new("<a>\r\n  <b/>\n  <c/>\r<d/>\n</a>");
        assert_eq!(e.convert_line_endings(Eol::Lf), 2, "the CRLF and the lone CR");
        assert_eq!(e.text(), "<a>\n  <b/>\n  <c/>\n<d/>\n</a>");
        assert_eq!(e.settings().eol, Eol::Lf);
        assert!(e.undo());
        assert_eq!(e.text(), "<a>\r\n  <b/>\n  <c/>\r<d/>\n</a>", "one step brings every break back");
        assert_eq!(e.convert_line_endings(Eol::CrLf), 3);
        assert_eq!(LineEndings::count(&e.text()), LineEndings { lf: 0, crlf: 4, cr: 0 });
    }

    #[test]
    fn a_text_that_has_the_ending_already_or_no_breaks_is_not_touched() {
        let mut e = Editor::new("<a>\n<b/>\n</a>");
        let version = e.version();
        assert_eq!(e.convert_line_endings(Eol::Lf), 0);
        assert_eq!(e.version(), version, "no edit, no new version");
        let mut e = Editor::new("<a/>");
        assert_eq!(e.convert_line_endings(Eol::CrLf), 0);
        assert_eq!(e.settings().eol, Eol::CrLf, "new breaks are written the new way");
        e.set_selection(4, 4);
        e.insert_newline();
        assert!(e.text().contains("\r\n"));
    }

    #[test]
    fn breaks_next_to_multibyte_text_and_at_the_ends_are_found() {
        let mut e = Editor::new("\r\nä\r\n\r\nö\r");
        assert_eq!(e.convert_line_endings(Eol::Lf), 4);
        assert_eq!(e.text(), "\nä\n\nö\n");
    }
}
