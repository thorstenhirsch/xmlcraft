//! XBRL instances in the validator: after the DTD or schema check, F8 lists what the checks of `xmlcraft-xbrl` find in an XBRL 2.1
//! instance or an Inline XBRL document, each at its fact or context.

use std::ops::Range;

use xmlcraft_syntax::{Code, Diagnostic};
use xmlcraft_xbrl::{Level, check, is_xbrl, read};

use crate::validation::ValidityProblem;

/// Whether `text` is an XBRL instance or an Inline XBRL document that can be read.
pub fn recognised(text: &str) -> bool {
    is_xbrl(text) && read(text).is_ok()
}

/// What the XBRL checks find in `text`, when it is an XBRL document (nothing otherwise, and nothing for text that cannot be read).
pub fn problems(text: &str) -> Vec<ValidityProblem> {
    if !is_xbrl(text) {
        return Vec::new();
    }
    let Ok(instance) = read(text) else { return Vec::new() };
    check(&instance)
        .into_iter()
        .map(|f| {
            let code = match f.level {
                Level::Error => Code::XbrlError,
                Level::Warning => Code::XbrlWarning,
            };
            // A finding about the whole document (no schemaRef) is shown at the start of the root element.
            let span: Range<usize> = if f.span.is_empty() { 0..1 } else { f.span.start..f.span.start.saturating_add(1) };
            ValidityProblem { diagnostic: Diagnostic::new(code, span, vec![f.message]), file: None }
        })
        .collect()
}
