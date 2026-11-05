//! Evaluating an XPath expression on a worker thread: what the XPath window waits for, so that an expression that takes long (or a large
//! document) never stops the window. The evaluation itself is [`xmlcraft_query::evaluate`].

use xmlcraft_query::{Evaluated, Options, QueryError, evaluate};
use xmlcraft_syntax::parse;
use xmlcraft_xsd::dom;

use crate::worker::Worker;

/// The prefixes the root element of `text` declares, with their namespaces: the ones an expression may use without declaring them. The
/// default namespace has no prefix and is not among them (an expression names such elements `*:name`).
pub fn root_namespaces(text: &str) -> Vec<(String, String)> {
    let tree = dom::build(&parse(text).syntax());
    let Some(root) = tree.root() else { return Vec::new() };
    let mut found: Vec<(String, String)> = tree.in_scope(root).into_iter().filter_map(|(prefix, uri)| prefix.map(|p| (p, uri))).collect();
    found.sort();
    found
}

/// One request: an expression and the text of the document it is evaluated on.
#[derive(Clone, Debug)]
pub struct XPathJob {
    /// Told back in the outcome, so that the caller can tell a result it still waits for from one it gave up on.
    pub serial: u64,
    pub text: String,
    pub expression: String,
    pub options: Options,
}

/// What came of a request.
#[derive(Clone, Debug)]
pub struct XPathOutcome {
    pub serial: u64,
    pub result: Result<Evaluated, QueryError>,
}

/// Evaluates expressions on a worker thread.
pub struct XPathRunner {
    worker: Worker<XPathJob, XPathOutcome>,
}

fn run(job: XPathJob) -> XPathOutcome {
    let serial = job.serial;
    // (The engine returns errors; the guard is for the case that it does not.)
    let result = match crate::guard(|| evaluate(&job.text, &job.expression, &job.options)) {
        Ok(done) => done,
        Err(panic) => {
            log::error!("evaluating an XPath expression failed: {panic}");
            Err(QueryError {
                phase: xmlcraft_query::Phase::Run,
                code: String::new(),
                message: "the evaluation failed (a bug); nothing was changed".to_owned(),
                span: None,
            })
        }
    };
    XPathOutcome { serial, result }
}

impl XPathRunner {
    pub fn new() -> Self {
        Self { worker: Worker::new("xpath evaluation", run) }
    }

    /// A runner with no thread: `submit` evaluates at once.
    pub fn inline() -> Self {
        Self { worker: Worker::inline(run) }
    }

    pub fn on_done(&self, notify: impl Fn() + Send + Sync + 'static) {
        self.worker.on_done(notify);
    }

    pub fn submit(&self, job: XPathJob) {
        self.worker.submit(0, job);
    }

    pub fn poll(&self) -> Vec<XPathOutcome> {
        self.worker.poll().into_iter().map(|(_, outcome)| outcome).collect()
    }
}

impl Default for XPathRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(serial: u64, expression: &str) -> XPathJob {
        XPathJob { serial, text: "<r><i n=\"1\"/><i n=\"2\"/></r>".to_owned(), expression: expression.to_owned(), options: Options::default() }
    }

    #[test]
    fn an_expression_is_evaluated_and_the_outcome_says_which_request_it_is_for() {
        let runner = XPathRunner::inline();
        runner.submit(job(7, "count(//i)"));
        runner.submit(job(8, "//i[@n = 2]"));
        let outcomes = runner.poll();
        assert_eq!(outcomes.iter().map(|o| o.serial).collect::<Vec<_>>(), [7, 8]);
        let first = outcomes[0].result.as_ref().expect("evaluates");
        assert_eq!(first.items[0].text, "2");
        let second = outcomes[1].result.as_ref().expect("evaluates");
        assert_eq!(second.count, 1);
        assert!(second.items[0].span.is_some(), "a node knows its place in the text");
    }

    #[test]
    fn the_prefixes_of_the_root_element_are_there_to_use_and_the_default_namespace_is_not_one() {
        let text = "<r xmlns=\"urn:d\" xmlns:a=\"urn:a\" xmlns:b=\"urn:b\"><a:i/></r>";
        assert_eq!(root_namespaces(text), [("a".to_owned(), "urn:a".to_owned()), ("b".to_owned(), "urn:b".to_owned())]);
        assert!(root_namespaces("not xml").is_empty());
        let mut job = job(1, "count(//a:i)");
        job.text = text.to_owned();
        job.options.namespaces = root_namespaces(text);
        let runner = XPathRunner::inline();
        runner.submit(job);
        assert_eq!(runner.poll()[0].result.as_ref().expect("evaluates").items[0].text, "1");
    }

    #[test]
    fn a_node_with_a_prefix_knows_its_place_in_the_text() {
        let text = "<order xmlns:p=\"urn:p\"><p:item n=\"1\">Tea</p:item><p:item n=\"2\">Milk</p:item></order>";
        let mut j = job(1, "//p:item[@n = 2]");
        j.text = text.to_owned();
        j.options.namespaces = root_namespaces(text);
        let runner = XPathRunner::inline();
        runner.submit(j);
        let done = runner.poll().remove(0).result.expect("evaluates");
        let span = done.items[0].span.clone().expect("a place");
        assert_eq!(&text[span], "<p:item n=\"2\">Milk</p:item>");
    }

    #[test]
    fn an_error_comes_back_as_an_outcome_not_a_panic() {
        let runner = XPathRunner::inline();
        runner.submit(job(1, "//i["));
        let outcomes = runner.poll();
        let error = outcomes[0].result.as_ref().expect_err("does not compile");
        assert_eq!(error.code, "XPST0003");
    }

    #[test]
    fn a_threaded_runner_delivers_and_asks_for_a_repaint() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let runner = XPathRunner::new();
        let wakes = Arc::new(AtomicUsize::new(0));
        runner.on_done({
            let wakes = wakes.clone();
            move || {
                wakes.fetch_add(1, Ordering::SeqCst);
            }
        });
        runner.submit(job(3, "string(//i[1]/@n)"));
        let started = std::time::Instant::now();
        let mut got = Vec::new();
        while got.is_empty() {
            assert!(started.elapsed() < std::time::Duration::from_secs(10), "no outcome");
            got = runner.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(got[0].serial, 3);
        assert!(wakes.load(Ordering::SeqCst) >= 1);
    }
}
