//! Running an XQuery on a worker thread: what the Query tool's XQuery tab waits for, so that a long query never stops the window.
//! The run itself is [`xmlcraft_xquery::evaluate`].

use xmlcraft_xquery::{Options, Outcome, QueryError};

use crate::worker::Worker;

/// One request: a query, and the text of the document that is its context item (if any).
#[derive(Clone, Debug)]
pub struct XQueryJob {
    /// Told back in the outcome, so that the caller can tell a result it still waits for from one it gave up on.
    pub serial: u64,
    pub query: String,
    pub context: Option<String>,
    pub options: Options,
}

/// What came of a request.
#[derive(Clone, Debug)]
pub struct XQueryOutcome {
    pub serial: u64,
    pub result: Result<Outcome, QueryError>,
}

fn run(job: XQueryJob) -> XQueryOutcome {
    let serial = job.serial;
    let result = match crate::guard(|| xmlcraft_xquery::evaluate(&job.query, job.context.as_deref(), &job.options)) {
        Ok(done) => done,
        Err(panic) => {
            log::error!("a query failed: {panic}");
            Err(QueryError {
                phase: xmlcraft_xquery::Phase::Run,
                code: String::new(),
                message: "the query failed (a bug); nothing was changed".to_owned(),
                span: None,
            })
        }
    };
    XQueryOutcome { serial, result }
}

/// Runs queries on a worker thread.
pub struct XQueryRunner {
    worker: Worker<XQueryJob, XQueryOutcome>,
}

impl Default for XQueryRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl XQueryRunner {
    pub fn new() -> Self {
        Self { worker: Worker::new("xquery", run) }
    }

    /// A runner with no thread: `submit` runs at once.
    pub fn inline() -> Self {
        Self { worker: Worker::inline(run) }
    }

    pub fn on_done(&self, notify: impl Fn() + Send + Sync + 'static) {
        self.worker.on_done(notify);
    }

    pub fn submit(&self, job: XQueryJob) {
        self.worker.submit(0, job);
    }

    pub fn poll(&self) -> Vec<XQueryOutcome> {
        self.worker.poll().into_iter().map(|(_, outcome)| outcome).collect()
    }
}
