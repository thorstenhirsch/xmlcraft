//! Quick fixes in the window: the Fix button of a problem in the Problems panel, and Edit ▸ Quick Fix, which puts right the problem at
//! the caret. The fixes themselves are `xmlcraft_engine::quickfix`; this works out the ones for the text as it is, once per version.

use std::collections::BTreeMap;

use xmlcraft_engine::quickfix::{QuickFix, quick_fixes};

use crate::XmlCraftApp;
use crate::i18n::{fmt, tl};

/// The fixes of the problems of one version of a text (and of one validation of it), by where the problem starts.
pub(crate) struct FixCache {
    key: (u64, Option<u64>),
    /// Keyed by where the problem starts and its code: two problems can start at the same place.
    by_problem: BTreeMap<(usize, &'static str), Vec<QuickFix>>,
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
        let mut by_problem: BTreeMap<(usize, &'static str), Vec<QuickFix>> = BTreeMap::new();
        let own = analysis.diagnostics.iter().take(MAX_PROBLEMS).map(|d| (d, analysis.diagnostics.as_slice()));
        let validity = validation.into_iter().flat_map(|v| v.problems.iter()).filter(|p| p.file.is_none()).map(|p| (&p.diagnostic, &[][..]));
        for (diagnostic, all) in own.chain(validity.take(MAX_PROBLEMS)) {
            let fixes = quick_fixes(&text, diagnostic, all);
            if !fixes.is_empty() {
                by_problem.entry((diagnostic.span.start, diagnostic.code.key())).or_default().extend(fixes);
            }
        }
        if let Some(view) = self.views.get_mut(&id) {
            view.fixes = Some(FixCache { key, by_problem });
        }
    }

    /// The fixes worked out for the text as it is now: none when the problems are those of older text (a fix made for that would change
    /// the wrong place).
    fn current_fixes(&self) -> Option<&FixCache> {
        let doc = self.session.active_document()?;
        let view = self.views.get(&doc.id())?;
        let analysis = view.analysis.as_ref().filter(|a| a.version == doc.editor().version())?;
        let cache = view.fixes.as_ref()?;
        (cache.key == (analysis.version, view.current_validation().map(|v| v.version))).then_some(cache)
    }

    /// The fixes of the problem with this code that starts at `start` in the document being shown.
    pub(crate) fn fixes_of_problem(&self, start: usize, code: &'static str) -> &[QuickFix] {
        self.current_fixes().and_then(|c| c.by_problem.get(&(start, code))).map_or(&[], Vec::as_slice)
    }

    /// The fixes for what is wrong where the caret is: the problems whose span holds it; when there is none there, those that start on
    /// the caret's line. The first problem's first.
    pub fn quick_fixes_at_caret(&mut self) -> Vec<QuickFix> {
        self.ensure_fixes();
        let Some(doc) = self.session.active_document() else { return Vec::new() };
        let editor = doc.editor();
        let caret = editor.primary().head();
        let Some(cache) = self.current_fixes() else { return Vec::new() };
        let here = self.problems_at(caret);
        let chosen: Vec<(usize, &'static str)> = if here.is_empty() {
            let buffer = editor.buffer();
            let line = buffer.line_of(caret.min(buffer.len()));
            cache.by_problem.keys().filter(|(start, _)| buffer.line_of((*start).min(buffer.len())) == line).copied().collect()
        } else {
            here
        };
        chosen.iter().filter_map(|key| cache.by_problem.get(key)).flatten().cloned().collect()
    }

    /// The problems whose span holds `offset`, as where they start and their codes.
    fn problems_at(&self, offset: usize) -> Vec<(usize, &'static str)> {
        let Some(analysis) = self.active_analysis() else { return Vec::new() };
        let holds = |span: &std::ops::Range<usize>| span.start <= offset && offset < span.end.max(span.start.saturating_add(1));
        let own = analysis.diagnostics.iter().filter(|d| holds(&d.span)).map(|d| (d.span.start, d.code.key()));
        let validity = self
            .active_validation()
            .into_iter()
            .flat_map(|v| v.problems.iter())
            .filter(|p| p.file.is_none() && holds(&p.diagnostic.span))
            .map(|p| (p.diagnostic.span.start, p.diagnostic.code.key()));
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
