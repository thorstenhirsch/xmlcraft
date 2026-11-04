//! What a click in the status bar does: write the line breaks another way, indent new lines another way, or write the file in another
//! encoding. None of them touches text the user did not ask to change: only the line breaks that differ are rewritten (one undo step),
//! indentation is a setting for what is typed from now on, and an encoding is how the text is written when it is saved.

use crate::XmlCraftApp;
use crate::chrome::StatusChoice;
use crate::i18n::{fmt, tl};

impl XmlCraftApp {
    pub(crate) fn apply_status_choice(&mut self, choice: StatusChoice) {
        let Some(doc) = self.session.active_document_mut() else { return };
        match choice {
            StatusChoice::LineEnding(eol) => {
                let changed = doc.editor_mut().convert_line_endings(eol);
                self.status = if changed == 0 {
                    fmt(tl!("Line breaks are written as {eol} from now on"), &[("eol", eol.label())])
                } else {
                    fmt(tl!("Converted {n} line breaks to {eol}"), &[("n", &changed.to_string()), ("eol", eol.label())])
                };
            }
            StatusChoice::Indent { tabs, size } => {
                let mut settings = doc.editor().settings().clone();
                settings.use_tabs = tabs;
                settings.indent_size = size;
                doc.editor_mut().set_settings(settings);
                self.status = if tabs {
                    tl!("New lines are indented with tabs").to_owned()
                } else {
                    fmt(tl!("New lines are indented with {n} spaces"), &[("n", &size.to_string())])
                };
            }
            StatusChoice::Encoding(encoding, bom) => {
                if doc.set_encoding(encoding, bom) {
                    let name = if bom { format!("{} BOM", encoding.name()) } else { encoding.name().to_owned() };
                    self.status = fmt(tl!("The file will be saved as {name}"), &[("name", &name)]);
                }
            }
        }
    }
}
