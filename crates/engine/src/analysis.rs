//! What is known about a document's text: its problems and its outline, from one parse.
//!
//! [`analyze`] is pure and total. [`Analyzer`] runs it on a worker thread so that typing never waits for it: a request replaces any
//! older one for the same document, a panic in the analysis becomes a failed result instead of taking the app down, and a result says
//! which version of the text it is about.

use std::ops::Range;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

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

/// What the analyzer calls when a result is ready (the app asks for a repaint).
type Callback = Arc<dyn Fn() + Send + Sync>;

/// A text waiting to be analysed (only the native worker takes them: on the web `submit` analyses at once).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
struct Job {
    document: u64,
    version: u64,
    text: Buffer,
}

/// The requests that have not started: at most one per document, because only the newest is worth doing.
#[derive(Default)]
struct Pending {
    jobs: Vec<Job>,
    /// The analyzer was dropped: the worker finishes what it has and ends.
    closed: bool,
}

impl Pending {
    /// Add a request, replacing an older one for the same document.
    fn push(&mut self, job: Job) {
        match self.jobs.iter_mut().find(|j| j.document == job.document) {
            Some(slot) => *slot = job,
            None => self.jobs.push(job),
        }
    }
}

#[derive(Default)]
struct Shared {
    pending: Mutex<Pending>,
    wake: Condvar,
    finished: Mutex<Vec<(u64, Analysis)>>,
    on_done: Mutex<Option<Callback>>,
}

impl Shared {
    fn lock_pending(&self) -> MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Publish a result and tell whoever is waiting for one. The callback runs outside every lock, so it may do anything (even set
    /// another callback).
    fn finish(&self, document: u64, analysis: Analysis) {
        self.finished.lock().unwrap_or_else(PoisonError::into_inner).push((document, analysis));
        let callback = self.on_done.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(callback) = callback {
            callback();
        }
    }
}

/// The largest text analysed in the caller's own thread. On the web there is no other thread and a frame must not wait long, so it
/// is small there; a native analyzer whose worker could not start has the same limit as the worker.
#[cfg(target_arch = "wasm32")]
const INLINE_LIMIT: usize = 1024 * 1024;
#[cfg(not(target_arch = "wasm32"))]
const INLINE_LIMIT: usize = MAX_ANALYSED_BYTES;

/// Runs [`analyze`] on a worker thread. `submit` returns at once; `poll` collects what has finished.
pub struct Analyzer {
    shared: Arc<Shared>,
    /// Whether a worker thread is running; without one (the web, or a thread that could not start) `submit` analyses at once.
    threaded: bool,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Analyzer {
    fn drop(&mut self) {
        self.shared.lock_pending().closed = true;
        self.shared.wake.notify_all();
    }
}

impl Analyzer {
    pub fn new() -> Self {
        let shared = Arc::new(Shared::default());
        #[cfg(not(target_arch = "wasm32"))]
        {
            let worker_shared = shared.clone();
            let started = std::thread::Builder::new().name("analysis".into()).spawn(move || worker(&worker_shared)).is_ok();
            Self { shared, threaded: started }
        }
        #[cfg(target_arch = "wasm32")]
        {
            Self { shared, threaded: false }
        }
    }

    /// An analyzer with no thread: `submit` analyses in the caller's thread (what the web does, and handy for tests).
    pub fn inline() -> Self {
        Self { shared: Arc::new(Shared::default()), threaded: false }
    }

    /// Called whenever a result is ready (the app asks for a repaint).
    pub fn on_done(&self, notify: impl Fn() + Send + Sync + 'static) {
        *self.shared.on_done.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(notify));
    }

    /// Ask for `text` (version `version` of the document with this id) to be analysed. A request that has not started is replaced by a
    /// newer one for the same document, so a slow analysis never leaves a queue behind it.
    pub fn submit(&self, document: u64, version: u64, text: &Buffer) {
        if self.threaded {
            self.shared.lock_pending().push(Job { document, version, text: text.clone() });
            self.shared.wake.notify_one();
            return;
        }
        let analysis = if text.len() > INLINE_LIMIT { Analysis::skipped(version, Skipped::TooLarge) } else { analyze_guarded(&text.text(), version) };
        self.shared.finish(document, analysis);
    }

    /// The analyses that finished since the last call, oldest first.
    pub fn poll(&self) -> Vec<(u64, Analysis)> {
        std::mem::take(&mut *self.shared.finished.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// [`analyze`], with a panic turned into a failed result (the last-resort guard: a bug in the analysis must not take the app down).
pub fn analyze_guarded(text: &str, version: u64) -> Analysis {
    crate::guard(|| analyze(text, version)).unwrap_or_else(|message| {
        log::error!("analysis failed: {message}");
        Analysis::skipped(version, Skipped::Failed)
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn worker(shared: &Shared) {
    loop {
        let jobs = {
            let mut pending = shared.lock_pending();
            while pending.jobs.is_empty() && !pending.closed {
                pending = shared.wake.wait(pending).unwrap_or_else(PoisonError::into_inner);
            }
            if pending.jobs.is_empty() {
                return; // closed, and nothing left to do
            }
            std::mem::take(&mut pending.jobs)
        };
        for job in jobs {
            let analysis = analyze_guarded(&job.text.text(), job.version);
            shared.finish(job.document, analysis);
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

    #[test]
    fn a_request_that_has_not_started_is_replaced_by_a_newer_one_for_the_same_document() {
        let mut pending = Pending::default();
        let job = |document, version| Job { document, version, text: Buffer::from_text("<a/>") };
        for version in 0..1000 {
            pending.push(job(1, version));
        }
        pending.push(job(2, 7));
        pending.push(job(1, 1000));
        assert_eq!(
            pending.jobs.iter().map(|j| (j.document, j.version)).collect::<Vec<_>>(),
            [(1, 1000), (2, 7)],
            "no queue builds up behind a slow analysis"
        );
    }

    #[test]
    fn without_a_thread_the_result_is_there_at_once_and_the_callback_is_called() {
        let analyzer = Analyzer::inline();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = calls.clone();
        analyzer.on_done(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        analyzer.submit(3, 9, &Buffer::from_text("<a>"));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1, "whoever waits is told");
        let results = analyzer.poll();
        assert_eq!((results.len(), results[0].0, results[0].1.version, results[0].1.is_well_formed()), (1, 3, 9, false));
    }

    #[test]
    fn a_callback_may_replace_itself_without_deadlock() {
        let analyzer = Arc::new(Analyzer::inline());
        let again = analyzer.clone();
        analyzer.on_done(move || again.on_done(|| {}));
        analyzer.submit(1, 1, &Buffer::from_text("<a/>"));
        assert_eq!(analyzer.poll().len(), 1);
    }
}
