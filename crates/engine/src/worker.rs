//! A worker thread for one kind of request: what the analysis and the validation of a document are run by, so that typing never waits
//! for them. A request replaces any older one for the same document that has not started, a panic is the caller's to turn into a
//! result (see `crate::guard`), and a result says which document it is for.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

/// What the worker calls when a result is ready (the app asks for a repaint).
type Callback = Arc<dyn Fn() + Send + Sync>;

/// What to do with a request.
type Run<P, R> = Arc<dyn Fn(P) -> R + Send + Sync>;

/// A request waiting to be run (only the native worker takes them: without a thread `submit` runs them at once).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
struct Job<P> {
    document: u64,
    payload: P,
}

/// The requests that have not started: at most one per document, because only the newest is worth doing.
struct Pending<P> {
    jobs: Vec<Job<P>>,
    /// The worker was dropped: the thread finishes what it has and ends.
    closed: bool,
}

impl<P> Default for Pending<P> {
    fn default() -> Self {
        Self { jobs: Vec::new(), closed: false }
    }
}

impl<P> Pending<P> {
    /// Add a request, replacing an older one for the same document.
    fn push(&mut self, job: Job<P>) {
        match self.jobs.iter_mut().find(|j| j.document == job.document) {
            Some(slot) => *slot = job,
            None => self.jobs.push(job),
        }
    }
}

struct Shared<P, R> {
    pending: Mutex<Pending<P>>,
    wake: Condvar,
    finished: Mutex<Vec<(u64, R)>>,
    on_done: Mutex<Option<Callback>>,
}

impl<P, R> Shared<P, R> {
    fn lock_pending(&self) -> MutexGuard<'_, Pending<P>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Publish a result and tell whoever is waiting for one. The callback runs outside every lock, so it may do anything (even set
    /// another callback).
    fn finish(&self, document: u64, result: R) {
        self.finished.lock().unwrap_or_else(PoisonError::into_inner).push((document, result));
        let callback = self.on_done.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(callback) = callback {
            callback();
        }
    }
}

/// Runs requests (a payload for a document) on a thread. `submit` returns at once; `poll` collects what has finished.
pub(crate) struct Worker<P: Send + 'static, R: Send + 'static> {
    shared: Arc<Shared<P, R>>,
    run: Run<P, R>,
    /// Whether a thread is running; without one (the web, or a thread that could not start) `submit` runs the request at once.
    threaded: bool,
}

impl<P: Send + 'static, R: Send + 'static> Drop for Worker<P, R> {
    fn drop(&mut self) {
        self.shared.lock_pending().closed = true;
        self.shared.wake.notify_all();
    }
}

impl<P: Send + 'static, R: Send + 'static> Worker<P, R> {
    fn with_shared(run: Run<P, R>, threaded: bool, shared: Arc<Shared<P, R>>) -> Self {
        Self { shared, run, threaded }
    }

    fn new_shared() -> Arc<Shared<P, R>> {
        Arc::new(Shared {
            pending: Mutex::new(Pending::default()),
            wake: Condvar::new(),
            finished: Mutex::new(Vec::new()),
            on_done: Mutex::new(None),
        })
    }

    /// A worker with a thread called `name` (on the web, one without).
    pub(crate) fn new(name: &str, run: impl Fn(P) -> R + Send + Sync + 'static) -> Self {
        let shared = Self::new_shared();
        let run: Run<P, R> = Arc::new(run);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (thread_shared, thread_run) = (shared.clone(), run.clone());
            let started = std::thread::Builder::new().name(name.into()).spawn(move || work(&thread_shared, &thread_run)).is_ok();
            Self::with_shared(run, started, shared)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = name;
            Self::with_shared(run, false, shared)
        }
    }

    /// A worker with no thread: `submit` runs the request in the caller's thread (what the web does, and handy for tests).
    pub(crate) fn inline(run: impl Fn(P) -> R + Send + Sync + 'static) -> Self {
        Self::with_shared(Arc::new(run), false, Self::new_shared())
    }

    pub(crate) fn threaded(&self) -> bool {
        self.threaded
    }

    /// Called whenever a result is ready.
    pub(crate) fn on_done(&self, notify: impl Fn() + Send + Sync + 'static) {
        *self.shared.on_done.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(notify));
    }

    /// Ask for a payload to be run for a document. A request that has not started is replaced by a newer one for the same document,
    /// so a slow one never leaves a queue behind it.
    pub(crate) fn submit(&self, document: u64, payload: P) {
        if self.threaded {
            self.shared.lock_pending().push(Job { document, payload });
            self.shared.wake.notify_one();
            return;
        }
        let result = (self.run)(payload);
        self.shared.finish(document, result);
    }

    /// Publish a result that was made without running a request (one that is refused at once).
    pub(crate) fn finish(&self, document: u64, result: R) {
        self.shared.finish(document, result);
    }

    /// The results that finished since the last call, oldest first.
    pub(crate) fn poll(&self) -> Vec<(u64, R)> {
        std::mem::take(&mut *self.shared.finished.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn work<P, R>(shared: &Shared<P, R>, run: &Run<P, R>) {
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
            let result = run(job.payload);
            shared.finish(job.document, result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_that_has_not_started_is_replaced_by_a_newer_one_for_the_same_document() {
        let mut pending: Pending<u64> = Pending::default();
        for version in 0..1000 {
            pending.push(Job { document: 1, payload: version });
        }
        pending.push(Job { document: 2, payload: 7 });
        pending.push(Job { document: 1, payload: 1000 });
        assert_eq!(
            pending.jobs.iter().map(|j| (j.document, j.payload)).collect::<Vec<_>>(),
            [(1, 1000), (2, 7)],
            "no queue builds up behind a slow request"
        );
    }

    #[test]
    fn requests_are_run_in_the_background_and_a_slow_one_does_not_leave_a_queue() {
        let worker = Worker::new("test", |n: u64| n * 2);
        let (tx, rx) = std::sync::mpsc::channel();
        worker.on_done(move || {
            let _ = tx.send(());
        });
        worker.submit(1, 1);
        worker.submit(1, 2);
        worker.submit(2, 5);
        let mut results: Vec<(u64, u64)> = Vec::new();
        while !(results.iter().any(|(d, r)| *d == 1 && *r == 4) && results.iter().any(|(d, r)| *d == 2 && *r == 10)) {
            rx.recv_timeout(std::time::Duration::from_secs(10)).expect("a result arrives");
            results.extend(worker.poll());
        }
        assert!(worker.poll().is_empty());
    }

    #[test]
    fn without_a_thread_the_result_is_there_at_once_and_the_callback_is_called() {
        let worker = Worker::inline(|n: u64| n + 1);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = calls.clone();
        worker.on_done(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        worker.submit(3, 9);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1, "whoever waits is told");
        assert_eq!(worker.poll(), [(3, 10)]);
        assert!(!worker.threaded());
    }

    #[test]
    fn a_callback_may_replace_itself_without_deadlock() {
        let worker = Arc::new(Worker::inline(|n: u64| n));
        let again = worker.clone();
        worker.on_done(move || again.on_done(|| {}));
        worker.submit(1, 1);
        assert_eq!(worker.poll().len(), 1);
    }
}
