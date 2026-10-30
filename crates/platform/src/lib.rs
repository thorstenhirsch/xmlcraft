#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-platform: what only a native build can do.
//!
//! - **Layer:** L7. The native code is behind `cfg(not(target_arch = "wasm32"))`; on the web the same functions exist so that the crate
//!   compiles for it: those that return `io::Result` fail with `ErrorKind::Unsupported`, and `changed_on_disk` reports
//!   `DiskChange::Missing`. Callers say "not available" instead of failing to build.
//! - [`file_uri`]: the path a `file:` URI names.
//! - [`schema_files`]: the files the resolver reads (the external parts of DTDs and the documents of schemas).
//! - [`network`]: what the resolvers download with ([`network::Network`]: the client, the cache, the clock and the switch to work
//!   offline). [`http`] (the client, `ureq` over rustls) and [`disk_cache`] (the downloaded copies, kept between runs) are native only;
//!   on the web `Network::standard()` is a network that downloads nothing.
//! - [`files`]: reading a file with a size limit, writing it atomically (a crash or a full disk leaves the old file whole), and noticing
//!   that something else changed or removed it.

#[cfg(not(target_arch = "wasm32"))]
pub mod disk_cache;
pub mod file_uri;
pub mod files;
#[cfg(not(target_arch = "wasm32"))]
pub mod http;
pub mod network;
pub mod schema_files;
