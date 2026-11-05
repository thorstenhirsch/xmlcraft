#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! Comparison and merging for XmlCraft.

mod compare;
mod lines;
mod merge;
mod node;
mod seq;

pub use compare::{Change, ChangeKind, Family, compare_documents, compare_mixed, compare_trees};
pub use lines::{Hunk, HunkKind, LineDiff, LineOptions, Whitespace, diff_lines, diff_words, split_lines};
pub use merge::{Choice, Conflict, Merge3, Region, merge3};
pub use node::{DiffError, Node, NodeKind, TreeOptions};
pub use seq::{DEFAULT_BUDGET, matches as common_subsequence};
