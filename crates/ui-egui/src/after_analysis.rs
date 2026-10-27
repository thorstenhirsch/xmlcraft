//! Commands that need the analysis of the text as it is now: the jump to the matching tag, folding.
//!
//! The window never waits for a parse: a command asked for when the text has changed since the last analysis waits for the new one,
//! and is dropped if the text changes again meanwhile (its caret is not where it was asked in).

use xmlcraft_engine::analysis::Skipped;
use xmlcraft_engine::folding::{self, FoldAction};

use crate::XmlCraftApp;
use crate::documents::DocView;
use crate::i18n::{fmt, tl};
use crate::theme::ThemeKind;

/// What to do once the analysis of the text is here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Then {
    /// Go to the other tag of the element the caret is at.
    GoToMatchingTag,
    /// Fold or unfold as `FoldAction` says.
    Fold(FoldAction),
    /// Fold what can be folded into this line (0-based): its fold mark was clicked.
    FoldLine(usize),
}

/// A command that is waiting for the analysis of one version of one document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Waiting {
    pub document: u64,
    pub version: u64,
    pub then: Then,
}

impl XmlCraftApp {
    /// Do `then` for the document being shown, as soon as the analysis of its text as it is now is here (at once, if it is).
    pub(crate) fn when_analysed(&mut self, then: Then) {
        let kind = self.applied.unwrap_or(ThemeKind::Light);
        let Some(doc) = self.session.active_document() else { return };
        let (document, version) = (doc.id(), doc.editor().version());
        let view = self.views.entry(document).or_insert_with(|| DocView::new(kind));
        if view.requested() != Some(version) {
            view.set_requested(version);
            self.analyzer.submit(document, version, doc.editor().buffer());
        }
        self.waiting = Some(Waiting { document, version, then });
        self.finish_waiting();
    }

    /// Do the command that was waiting, if the analysis of its text is here.
    pub(crate) fn finish_waiting(&mut self) {
        let Some(Waiting { document, version, then }) = self.waiting else { return };
        let shown = self.session.active_document().map(xmlcraft_engine::Document::id);
        let Some(doc) = self.session.position_of(document).and_then(|i| self.session.document_mut(i)) else {
            self.waiting = None; // closed meanwhile
            return;
        };
        if doc.editor().version() != version {
            self.waiting = None; // the text changed again: the caret is not where it was asked in
            return;
        }
        if shown != Some(document) {
            self.waiting = None; // another tab is shown now: what is done would be done in that one
            return;
        }
        let Some(analysis) = self.views.get(&document).and_then(|v| v.analysis.as_ref()).filter(|a| a.version == version) else { return };
        self.waiting = None;
        let name = doc.name().to_owned();
        if let Some(why) = analysis.skipped {
            self.status = match why {
                Skipped::TooLarge => fmt(tl!("{name} is too large for this"), &[("name", &name)]),
                Skipped::Failed => fmt(tl!("Analysing {name} failed. This is a bug; the document is unchanged."), &[("name", &name)]),
            };
            return;
        }
        match then {
            Then::GoToMatchingTag => {
                let head = doc.editor().primary().head();
                match analysis.matching_tag(head) {
                    Some(found) => {
                        let at = found.jump_target(head);
                        self.jump_to(at..at);
                    }
                    None => self.status = tl!("No matching tag here").to_owned(),
                }
            }
            Then::Fold(action) => {
                let changed = folding::apply(doc.editor_mut(), &analysis.folds, action);
                if !changed {
                    self.status = match action {
                        FoldAction::Fold => tl!("Nothing to fold here").to_owned(),
                        FoldAction::Unfold => tl!("Nothing is folded here").to_owned(),
                        FoldAction::Level(level) => fmt(tl!("Nothing to fold at level {level}"), &[("level", &level.to_string())]),
                        FoldAction::FoldAll | FoldAction::UnfoldAll => String::new(),
                    };
                }
            }
            Then::FoldLine(line) => {
                if let Some(range) = analysis.folds.iter().find(|r| r.first_line == line) {
                    doc.editor_mut().fold_lines(range.first_line, range.last_line);
                }
            }
        }
    }
}
