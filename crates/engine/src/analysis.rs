//! What is known about a document's text: its problems and its outline, from one parse.
//!
//! [`analyze`] is pure and total. [`Analyzer`] runs it on a worker thread so that typing never waits for it: a request replaces any
//! older one for the same document, a panic in the analysis becomes a failed result instead of taking the app down, and a result says
//! which version of the text it is about.

use std::ops::Range;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};

use xmlcraft_syntax::{Diagnostic, Limits, Severity, SyntaxKind, parse_with};
use xmlcraft_text::Buffer;

/// Documents larger than this are not analysed on every edit (large-file mode, M1.15, takes over).
pub const MAX_ANALYSED_BYTES: usize = 32 * 1024 * 1024;
/// The outline shows at most this many elements.
pub const MAX_OUTLINE_ITEMS: usize = 50_000;

/// One element in the outline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineItem {
    /// Nesting depth, 0 for the root element.
    pub depth: usize,
    /// The element's name as written (with any prefix).
    pub name: String,
    /// A short hint from its `id`, `name` or `key` attribute, if it has one.
    pub label: Option<String>,
    /// The element, start tag to end tag, as a byte range of the text.
    pub range: Range<usize>,
    /// The name in the start tag.
    pub name_range: Range<usize>,
}

/// The result of analysing one version of a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Analysis {
    /// The version of the text this is about (`Editor::version`).
    pub version: u64,
    pub diagnostics: Vec<Diagnostic>,
    pub outline: Vec<OutlineItem>,
    /// The outline stopped at [`MAX_OUTLINE_ITEMS`].
    pub outline_truncated: bool,
    /// The text was not analysed, and why.
    pub skipped: Option<Skipped>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skipped {
    /// Larger than [`MAX_ANALYSED_BYTES`].
    TooLarge,
    /// The analysis itself failed (a bug); the document is untouched.
    Failed,
}

impl Analysis {
    pub fn error_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.severity() == Severity::Error).count()
    }

    pub fn warning_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.severity() == Severity::Warning).count()
    }

    /// Analysed, and no errors.
    pub fn is_well_formed(&self) -> bool {
        self.skipped.is_none() && self.error_count() == 0
    }

    fn skipped(version: u64, why: Skipped) -> Self {
        Self { version, diagnostics: Vec::new(), outline: Vec::new(), outline_truncated: false, skipped: Some(why) }
    }
}

/// Analyse `text`, which is version `version` of a document.
pub fn analyze(text: &str, version: u64) -> Analysis {
    if text.len() > MAX_ANALYSED_BYTES {
        return Analysis::skipped(version, Skipped::TooLarge);
    }
    let parsed = parse_with(text, &Limits::default());
    let mut diagnostics = parsed.diagnostics().to_vec();
    diagnostics.sort_by_key(|d| (d.span.start, d.span.end));
    let (outline, outline_truncated) = outline_of(&parsed.syntax(), text);
    Analysis { version, diagnostics, outline, outline_truncated, skipped: None }
}

fn outline_of(root: &xmlcraft_syntax::SyntaxNode, text: &str) -> (Vec<OutlineItem>, bool) {
    use rowan::WalkEvent;
    let mut items: Vec<OutlineItem> = Vec::new();
    let mut depth = 0usize;
    let mut truncated = false;
    for event in root.preorder() {
        match event {
            WalkEvent::Enter(node) if node.kind() == SyntaxKind::Element => {
                if items.len() >= MAX_OUTLINE_ITEMS {
                    truncated = true;
                } else if let Some(item) = item_of(&node, text, depth) {
                    items.push(item);
                }
                depth += 1;
            }
            WalkEvent::Leave(node) if node.kind() == SyntaxKind::Element => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    (items, truncated)
}

fn item_of(node: &xmlcraft_syntax::SyntaxNode, text: &str, depth: usize) -> Option<OutlineItem> {
    let range = usize::from(node.text_range().start())..usize::from(node.text_range().end());
    let tag = node.children().find(|n| n.kind() == SyntaxKind::StartTag)?;
    let name_token = tag.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == SyntaxKind::Name)?;
    let name_range = usize::from(name_token.text_range().start())..usize::from(name_token.text_range().end());
    let mut label = None;
    for attribute in tag.children().filter(|n| n.kind() == SyntaxKind::Attribute) {
        let Some(name) = attribute.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == SyntaxKind::Name) else { continue };
        if !matches!(name.text(), "id" | "name" | "key") {
            continue;
        }
        if let Some(value) = attribute.children().find(|n| n.kind() == SyntaxKind::AttrValue) {
            let r = usize::from(value.text_range().start())..usize::from(value.text_range().end());
            let inner = text.get(r.start..r.end).unwrap_or_default().trim_matches(['"', '\'']);
            if !inner.is_empty() {
                label = Some(inner.chars().take(40).collect());
                break;
            }
        }
    }
    Some(OutlineItem { depth, name: name_token.text().to_owned(), label, range, name_range })
}

type Finished = Arc<Mutex<Vec<(u64, Analysis)>>>;

/// What the worker calls when an analysis is ready (the app asks for a repaint), once one is set.
type Notify = Arc<Mutex<Option<Box<dyn Fn() + Send>>>>;

/// Runs [`analyze`] on a worker thread. `submit` returns at once; `poll` collects what has finished.
pub struct Analyzer {
    #[cfg(not(target_arch = "wasm32"))]
    jobs: Sender<Job>,
    finished: Finished,
    on_done: Notify,
}

#[cfg(not(target_arch = "wasm32"))]
struct Job {
    document: u64,
    version: u64,
    text: Buffer,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl Analyzer {
    pub fn new() -> Self {
        let finished: Finished = Arc::new(Mutex::new(Vec::new()));
        let on_done: Notify = Arc::new(Mutex::new(None));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (jobs, queue) = channel::<Job>();
            let (done, notify) = (finished.clone(), on_done.clone());
            // If the thread cannot start, `submit` falls back to analysing at once.
            let _ = std::thread::Builder::new().name("analysis".into()).spawn(move || worker(queue, done, notify));
            Self { jobs, finished, on_done }
        }
        #[cfg(target_arch = "wasm32")]
        {
            Self { finished, on_done }
        }
    }

    /// Called from the worker whenever a result is ready (the app asks for a repaint).
    pub fn on_done(&self, notify: impl Fn() + Send + 'static) {
        *self.on_done.lock().unwrap_or_else(PoisonError::into_inner) = Some(Box::new(notify));
    }

    /// Ask for `text` (version `version` of the document with this id) to be analysed. Newer requests for the same document supersede
    /// older ones that have not started.
    pub fn submit(&self, document: u64, version: u64, text: &Buffer) {
        #[cfg(not(target_arch = "wasm32"))]
        if self.jobs.send(Job { document, version, text: text.clone() }).is_ok() {
            return;
        }
        // No worker: do it here.
        let analysis = guarded(&text.text(), version);
        self.finished.lock().unwrap_or_else(PoisonError::into_inner).push((document, analysis));
    }

    /// The analyses that finished since the last call, oldest first.
    pub fn poll(&self) -> Vec<(u64, Analysis)> {
        std::mem::take(&mut *self.finished.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// [`analyze`], with a panic turned into a failed result.
fn guarded(text: &str, version: u64) -> Analysis {
    crate::guard(|| analyze(text, version)).unwrap_or_else(|message| {
        log::error!("analysis failed: {message}");
        Analysis::skipped(version, Skipped::Failed)
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn worker(queue: Receiver<Job>, finished: Finished, notify: Notify) {
    while let Ok(first) = queue.recv() {
        // Everything that arrived while the last one ran: only the newest per document is worth doing.
        let mut latest: Vec<Job> = vec![first];
        while let Ok(job) = queue.try_recv() {
            match latest.iter_mut().find(|j| j.document == job.document) {
                Some(slot) => *slot = job,
                None => latest.push(job),
            }
        }
        for job in latest {
            let analysis = guarded(&job.text.text(), job.version);
            finished.lock().unwrap_or_else(PoisonError::into_inner).push((job.document, analysis));
            if let Some(notify) = notify.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                notify();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn a_well_formed_document_has_no_problems_and_an_outline() {
        let text = "<?xml version=\"1.0\"?>\n<library>\n  <book id=\"b1\" title=\"x\"><title>A</title></book>\n  <book name=\"two\"/>\n</library>";
        let a = analyze(text, 7);
        assert_eq!((a.version, a.is_well_formed(), a.error_count(), a.skipped), (7, true, 0, None));
        let names: Vec<(usize, &str, Option<&str>)> = a.outline.iter().map(|i| (i.depth, i.name.as_str(), i.label.as_deref())).collect();
        assert_eq!(names, [(0, "library", None), (1, "book", Some("b1")), (2, "title", None), (1, "book", Some("two"))]);
        let book = &a.outline[1];
        assert_eq!(&text[book.range.clone()], "<book id=\"b1\" title=\"x\"><title>A</title></book>");
        assert_eq!(&text[book.name_range.clone()], "book");
    }

    #[test]
    fn problems_are_sorted_by_position_and_counted() {
        let a = analyze("<a><b></a>\n<c/>", 1);
        assert!(!a.is_well_formed());
        let starts: Vec<usize> = a.diagnostics.iter().map(|d| d.span.start).collect();
        assert!(starts.windows(2).all(|w| w[0] <= w[1]), "{starts:?}");
        assert!(a.error_count() >= 2, "an unclosed element and a second root");
        let warned = analyze("<?xml version=\"1.1\"?><a/>", 1);
        assert_eq!((warned.error_count(), warned.warning_count(), warned.is_well_formed()), (0, 1, true));
    }

    #[test]
    fn a_broken_document_still_has_an_outline() {
        let a = analyze("<a><b><c></b></a>", 1);
        assert_eq!(a.outline.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
    }

    #[test]
    fn deep_and_huge_documents_stay_bounded() {
        let deep = "<a>".repeat(100_000);
        let a = analyze(&deep, 1);
        // The syntax tree stops nesting at 1024 levels (deeper elements are checked but not nodes), so the outline does too.
        assert_eq!((a.outline.len(), a.outline.last().map(|i| i.depth)), (1024, Some(1023)));
        assert!(a.diagnostics.iter().any(|d| d.code == xmlcraft_syntax::Code::NestingTooDeep));
        let wide = format!("<r>{}</r>", "<x/>".repeat(MAX_OUTLINE_ITEMS + 10));
        let a = analyze(&wide, 1);
        assert_eq!((a.outline.len(), a.outline_truncated), (MAX_OUTLINE_ITEMS, true));
        assert!(a.outline.iter().all(|i| i.depth <= 1));
        let big = " ".repeat(MAX_ANALYSED_BYTES + 1);
        assert_eq!(analyze(&big, 1).skipped, Some(Skipped::TooLarge));
    }

    proptest! {
        /// Whatever the text, the outline's ranges are inside it and nest properly.
        #[test]
        fn outline_ranges_are_inside_the_text(pieces in proptest::collection::vec(proptest::sample::select(&["<a>", "</a>", "<b x=\"1\">", "</b>", "<c/>", "text", "&", "<!--", "-->", "<![CDATA[", "]]>", "\n", "é"][..]), 0..40)) {
            let text = pieces.concat();
            let a = analyze(&text, 1);
            for item in &a.outline {
                prop_assert!(item.range.start <= item.range.end && item.range.end <= text.len());
                prop_assert!(text.is_char_boundary(item.range.start) && text.is_char_boundary(item.range.end));
                prop_assert!(item.name_range.start >= item.range.start && item.name_range.end <= item.range.end);
                prop_assert_eq!(&text[item.name_range.clone()], item.name.as_str());
            }
            for d in &a.diagnostics {
                prop_assert!(d.span.end <= text.len());
            }
        }
    }

    #[test]
    fn the_worker_analyses_in_the_background_and_keeps_the_newest() {
        let analyzer = Analyzer::new();
        let (tx, rx) = std::sync::mpsc::channel();
        analyzer.on_done(move || {
            let _ = tx.send(());
        });
        let buffer = Buffer::from_text("<a><b></a>");
        analyzer.submit(1, 1, &buffer);
        analyzer.submit(1, 2, &Buffer::from_text("<a/>"));
        analyzer.submit(2, 5, &buffer);
        let mut results: Vec<(u64, Analysis)> = Vec::new();
        let have_everything =
            |results: &[(u64, Analysis)]| results.iter().any(|(d, a)| *d == 1 && a.version == 2) && results.iter().any(|(d, _)| *d == 2);
        while !have_everything(&results) {
            rx.recv_timeout(std::time::Duration::from_secs(10)).expect("a result arrives");
            results.extend(analyzer.poll());
        }
        let newest_of_one = results.iter().filter(|(d, _)| *d == 1).map(|(_, a)| a.version).max();
        assert_eq!(newest_of_one, Some(2));
        let two = results.iter().find(|(d, _)| *d == 2).map(|(_, a)| a).expect("document two");
        assert_eq!((two.version, two.is_well_formed()), (5, false));
        assert!(analyzer.poll().is_empty());
    }
}
