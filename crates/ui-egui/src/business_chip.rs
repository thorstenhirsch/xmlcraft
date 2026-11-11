//! The chip in the header that says what a business document is (an e-invoice, an XBRL instance): worked out from the text once it has
//! stood still for a moment, so that typing does not read the whole document on every key ([`Settled`], which the command bar uses too).

use std::time::{Duration, Instant};

use xmlcraft_engine::business::{BusinessChip, chip_for};

use crate::XmlCraftApp;

/// The longest text the chip (and the command bar) reads while the window is in use.
pub(crate) const MAX_BYTES: usize = 2 * 1024 * 1024;
/// How long the text stands still before it is read again.
const QUIET: Duration = Duration::from_millis(400);

/// What is known of the chip of a document.
pub(crate) type ChipState = Settled<Option<BusinessChip>>;

/// Something worked out from the text of a document once the text has stood still for a moment (the chip, what the command bar
/// offers): the value for the version it was made from, and the version last seen and since when.
pub(crate) struct Settled<T> {
    done: Option<(u64, T)>,
    seen: Option<(u64, Instant)>,
}

impl<T> Default for Settled<T> {
    fn default() -> Self {
        Self { done: None, seen: None }
    }
}

impl<T: Clone> Settled<T> {
    /// The value for the text at `version`. The first look works it out at once (with `make`); a newer text is worked out once it has
    /// been left alone for a moment, and until then the value of the older text stays.
    pub(crate) fn get(&mut self, ctx: &egui::Context, version: u64, make: impl FnOnce() -> T) -> T {
        if let Some((v, value)) = &self.done
            && *v == version
        {
            return value.clone();
        }
        let now = Instant::now();
        let quiet = match self.seen {
            Some((v, at)) if v == version => now.saturating_duration_since(at) >= QUIET,
            _ => {
                self.seen = Some((version, now));
                false
            }
        };
        match &self.done {
            Some((_, value)) if !quiet => {
                ctx.request_repaint_after(QUIET);
                value.clone()
            }
            _ => {
                let value = make();
                self.done = Some((version, value.clone()));
                value
            }
        }
    }
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
        view.business.get(ctx, version, || if len <= MAX_BYTES { chip_for(&doc.editor().text()) } else { None })
    }
}
