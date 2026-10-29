//! What the resolver needs from the outside world. It does no I/O itself: the app, the headless tools and the tests each bring their own.

use std::sync::Arc;

/// The text of documents that are open (and maybe unsaved), by location. An open schema being edited is what a document that uses it
/// is validated against.
pub trait Buffers: Send + Sync {
    fn text(&self, location: &str) -> Option<Arc<str>>;
}

/// Reading files. `path` is a plain path (a `file:` URI is already turned into one). Where files may be read from, and how large they
/// may be, is the implementation's business: the answer is the bytes or the reason there are none.
pub trait FileAccess: Send + Sync {
    fn read(&self, path: &str) -> Result<Vec<u8>, String>;

    /// What the file at `path` is, so that two paths of one file are told to be one: its spelling made plain, unless the implementation
    /// can tell better (a file system that folds case, a link, is a place only the system knows the real name of).
    fn identity(&self, path: &str) -> String {
        crate::location::normalized(path)
    }
}

/// What the cache remembers about a copy, to ask the server whether it is still current.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Conditions {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// What a fetch may take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FetchLimits {
    /// The most bytes a body may have; a longer one is an error ([`FetchError::TooLarge`]).
    pub max_bytes: usize,
}

/// A downloaded resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub body: Vec<u8>,
    /// Where it came from after redirects: what relative references inside it are resolved against.
    pub final_url: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchOutcome {
    Fresh(Fetched),
    /// The copy the conditions describe is still current.
    NotModified,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// The connection failed, timed out, or the answer was not understood.
    Network(String),
    /// The server answered with this status.
    Status(u16),
    /// The body is longer than the limit.
    TooLarge,
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Network(why) => write!(f, "{why}"),
            FetchError::Status(code) => write!(f, "the server answered {code}"),
            FetchError::TooLarge => write!(f, "the resource is larger than the limit"),
        }
    }
}

/// Downloading. Only `http` and `https` URLs are asked for.
pub trait Fetcher: Send + Sync {
    fn fetch(&self, url: &str, conditions: &Conditions, limits: &FetchLimits) -> Result<FetchOutcome, FetchError>;
}

/// The time, in seconds since 1970-01-01 UTC (the crate reads no clock itself, so that it compiles for the web).
pub trait Clock: Send + Sync {
    fn now(&self) -> i64;
}

/// A clock that stands still: for tests.
pub struct FixedClock(pub i64);

impl Clock for FixedClock {
    fn now(&self) -> i64 {
        self.0
    }
}

/// No open documents.
pub struct NoBuffers;

impl Buffers for NoBuffers {
    fn text(&self, _: &str) -> Option<Arc<str>> {
        None
    }
}

/// No files can be read.
pub struct NoFiles;

impl FileAccess for NoFiles {
    fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        Err(format!("{path}: files are not read here"))
    }
}
