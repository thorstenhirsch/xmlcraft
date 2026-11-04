//! Quick fixes in the window: the Fix button of a problem in the Problems panel, and Edit ▸ Quick Fix, which puts right the problem at
//! the caret. The fixes themselves are `xmlcraft_engine::quickfix`; this works out the ones for the text as it is, once per version.

use std::collections::BTreeMap;

use xmlcraft_engine::quickfix::{QuickFix, quick_fixes};

use crate::XmlCraftApp;
use crate::i18n::{fmt, tl};

/// The fixes of the problems of one version of a text (and of one validation of it), by where the problem starts.
pub(crate) struct FixCache {
    key: (u64, Option<u64>),
    by_start: BTreeMap<usize, Vec<QuickFix>>,
}

/// The most problems that get their fixes worked out (a text with thousands of them is not helped by thousands of buttons).
const MAX_PROBLEMS: usize = 2_000;

impl XmlCraftApp {
    /// Work out the fixes of the problems of the document being shown, when that has not been done for its text as it is. Nothing when the
    /// problems are those of older text: a fix made for it would change the wrong place.
    pub(crate) fn ensure_fixes(&mut self) {
        let Some(doc) = self.session.active_document() else { return };
        let (id, version) = (doc.id(), doc.editor().version());
        let Some(view) = self.views.get(&id) else { return };
        let Some(analysis) = view.analysis.as_ref().filter(|a| a.version == version) else { return };
        let validation = view.current_validation();
        let key = (analysis.version, validation.map(|v| v.version));
        if view.fixes.as_ref().is_some_and(|c| c.key == key) {
            return;
        }
        let text = doc.editor().text();
        let mut by_start: BTreeMap<usize, Vec<QuickFix>> = BTreeMap::new();
        let own = analysis.diagnostics.iter().take(MAX_PROBLEMS).map(|d| (d, analysis.diagnostics.as_slice()));
        let validity = validation.into_iter().flat_map(|v| v.problems.iter()).filter(|p| p.file.is_none()).map(|p| (&p.diagnostic, &[][..]));
        for (diagnostic, all) in own.chain(validity.take(MAX_PROBLEMS)) {
            let fixes = quick_fixes(&text, diagnostic, all);
            if !fixes.is_empty() {
                by_start.entry(diagnostic.span.start).or_default().extend(fixes);
            }
        }
        if let Some(view) = self.views.get_mut(&id) {
            view.fixes = Some(FixCache { key, by_start });
        }
    }

    /// The fixes of the problems that start at `start` in the document being shown.
    pub(crate) fn fixes_starting_at(&self, start: usize) -> &[QuickFix] {
        let Some(id) = self.session.active_document().map(|d| d.id()) else { return &[] };
        self.views.get(&id).and_then(|v| v.fixes.as_ref()).and_then(|c| c.by_start.get(&start)).map_or(&[], Vec::as_slice)
    }

    /// The fixes for what is wrong where the caret is (a problem whose span holds it, or the one on the caret's line when there is none
    /// there), the first problem's first.
    pub fn quick_fixes_at_caret(&mut self) -> Vec<QuickFix> {
        self.ensure_fixes();
        let Some(doc) = self.session.active_document() else { return Vec::new() };
        let caret = doc.editor().primary().head();
        let Some(cache) = self.views.get(&doc.id()).and_then(|v| v.fixes.as_ref()) else { return Vec::new() };
        let holds = |start: usize| self.problem_spans_at(caret).contains(&start);
        cache.by_start.iter().filter(|(start, _)| holds(**start)).flat_map(|(_, f)| f.iter().cloned()).collect()
    }

    /// Where the problems whose span holds `offset` start.
    fn problem_spans_at(&self, offset: usize) -> Vec<usize> {
        let Some(analysis) = self.active_analysis() else { return Vec::new() };
        let holds = |span: &std::ops::Range<usize>| span.start <= offset && offset < span.end.max(span.start.saturating_add(1));
        let own = analysis.diagnostics.iter().filter(|d| holds(&d.span)).map(|d| d.span.start);
        let validity = self
            .active_validation()
            .into_iter()
            .flat_map(|v| v.problems.iter())
            .filter(|p| p.file.is_none() && holds(&p.diagnostic.span))
            .map(|p| p.diagnostic.span.start);
        own.chain(validity).collect()
    }

    /// Put `fix` in the text as one undoable step. Whether it was.
    pub fn apply_quick_fix(&mut self, fix: &QuickFix) -> bool {
        let Some(doc) = self.session.active_document_mut() else { return false };
        let editor = doc.editor_mut();
        let done = editor.apply_edits(&fix.edits);
        if done {
            if let Some(caret) = fix.caret.filter(|c| *c <= editor.buffer().len()) {
                editor.set_selection(caret, caret);
            }
            self.focus_editor = true;
            self.status = fmt(tl!("Fixed: {what}"), &[("what", &fix.title)]);
        }
        done
    }

    /// Edit ▸ Quick Fix: the first fix for what is wrong at the caret.
    pub(crate) fn quick_fix_at_caret(&mut self) {
        match self.quick_fixes_at_caret().into_iter().next() {
            Some(fix) => {
                self.apply_quick_fix(&fix);
            }
            None => self.status = tl!("There is nothing to fix at the caret.").to_owned(),
        }
    }
}
