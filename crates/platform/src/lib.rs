#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-platform: what only a native build can do.
//!
//! - **Layer:** L7. Everything here is behind `cfg(not(target_arch = "wasm32"))`, so the crate compiles for the web and is empty there.
//! - [`files`]: reading a file with a size limit, writing it atomically (a crash or a full disk leaves the old file whole), and noticing
//!   that something else changed or removed it.

#[cfg(not(target_arch = "wasm32"))]
pub mod files;
