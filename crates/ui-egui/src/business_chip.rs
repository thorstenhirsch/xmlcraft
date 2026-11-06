//! The chip in the header that says what a business document is (an e-invoice, an XBRL instance): worked out from the text once it has
//! stood still for a moment, so that typing does not read the whole document on every key.

use std::time::{Duration, Instant};

use xmlcraft_engine::business::{BusinessChip, chip_for};

use crate::XmlCraftApp;

/// The longest text the chip reads while the window is in use.
const MAX_BYTES: usize = 2 * 1024 * 1024;
/// How long the text stands still before it is read again.
const QUIET: Duration = Duration::from_millis(400);

/// What is known of the chip of a document.
#[derive(Default)]
pub(crate) struct ChipState {
    /// The version of the text the chip was made from, and the chip (none: the text is not a business document).
    done: Option<(u64, Option<BusinessChip>)>,
    /// The version last seen and since when.
    seen: Option<(u64, Instant)>,
}

impl XmlCraftApp {
    /// The chip of the document being shown, if it is a business document. A newer text is read once it has been left alone for a
    /// moment; until then the chip of the older text stays (and the first look reads at once).
    pub(crate) fn business_chip(&mut self, ctx: &egui::Context) -> Option<BusinessChip> {
        let doc = self.session.active_document()?;
        if !doc.language().is_xml() {
            return None;
        }
        let (id, version, len) = (doc.id(), doc.editor().version(), doc.editor().buffer().len());
        let view = self.views.get_mut(&id)?;
        let state = &mut view.business;
        if state.done.as_ref().is_some_and(|(v, _)| *v == version) {
            return state.done.as_ref().and_then(|(_, chip)| chip.clone());
        }
        let now = Instant::now();
        let first = state.done.is_none();
        let quiet = match state.seen {
            Some((v, at)) if v == version => now.saturating_duration_since(at) >= QUIET,
            _ => {
                state.seen = Some((version, now));
                false
            }
        };
        if first || quiet {
            let chip = if len <= MAX_BYTES { chip_for(&doc.editor().text()) } else { None };
            state.done = Some((version, chip.clone()));
            return chip;
        }
        ctx.request_repaint_after(QUIET);
        state.done.as_ref().and_then(|(_, chip)| chip.clone())
    }
}
