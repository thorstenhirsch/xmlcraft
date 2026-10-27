#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-platform: what only a native build can do.
//!
//! - **Layer:** L7. The native code is behind `cfg(not(target_arch = "wasm32"))`; on the web the same functions exist and fail with
//!   `ErrorKind::Unsupported`, so the crate compiles for it and callers say "not available" instead of failing to build.
//! - [`files`]: reading a file with a size limit, writing it atomically (a crash or a full disk leaves the old file whole), and noticing
//!   that something else changed or removed it.

pub mod files;
