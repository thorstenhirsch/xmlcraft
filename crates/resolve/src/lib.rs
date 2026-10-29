#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-resolve: finding the text a reference names.
//!
//! - **Layer:** L3, no UI and no I/O (files, the network and the clock are traits the caller implements), so it compiles for the web
//!   too.
//! - [`Resolver::resolve`] takes a [`Request`] (a public and/or system identifier of a DTD part, or a URI or namespace of a schema, and
//!   the location of the document that has it) and looks, in order, in the open documents, the user's catalogs, the bundled catalogs,
//!   and at the location itself: a bundled file, a file, or an `http`/`https` URL through the cache and then (when the policy allows)
//!   the network. See the module `resolver` for the details; every place that was looked in is kept in the [`Trail`].
//! - Budgets ([`Budget`]) bound one load: 256 resources and 64 MiB in all, 10 MiB for a download.

mod budget;
mod cache;
mod env;
mod location;
mod resolver;
mod trail;

pub use budget::{Budget, MAX_DOWNLOAD_BYTES, MAX_RESOURCES, MAX_TOTAL_BYTES};
pub use cache::{CacheEntry, MemoryCache, ResourceCache};
pub use env::{Buffers, Clock, Conditions, FetchError, FetchLimits, FetchOutcome, Fetched, Fetcher, FileAccess, FixedClock, NoBuffers, NoFiles};
pub use location::{file_path, is_path, join, normalized, scheme_of};
pub use resolver::{Environment, Failure, Kind, Origin, Policy, Request, Resolved, Resolver};
pub use trail::{Outcome, Stage, Step, Trail};
