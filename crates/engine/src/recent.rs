//! The recently opened files: newest first, no path twice, at most [`MAX_RECENT`].

use std::path::{Path, PathBuf};

/// How many files are remembered.
pub const MAX_RECENT: usize = 12;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recent {
    paths: Vec<PathBuf>,
}

impl Recent {
    pub fn new() -> Self {
        Self::default()
    }

    /// A list from paths given newest first (what was saved): a path that is there twice counts once, where it is first, and only the
    /// newest [`MAX_RECENT`] are kept.
    pub fn from_paths(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut recent = Self::new();
        for path in paths {
            if !recent.paths.contains(&path) && recent.paths.len() < MAX_RECENT {
                recent.paths.push(path);
            }
        }
        recent
    }

    /// `path` was opened or saved: it is now the newest.
    pub fn push(&mut self, path: PathBuf) {
        self.paths.retain(|other| *other != path);
        self.paths.insert(0, path);
        self.paths.truncate(MAX_RECENT);
    }

    /// Forget `path` (the file is gone, or the user does not want it listed).
    pub fn remove(&mut self, path: &Path) {
        self.paths.retain(|other| other != path);
    }

    pub fn clear(&mut self) {
        self.paths.clear();
    }

    /// The files, newest first.
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str) -> PathBuf {
        PathBuf::from(format!("/files/{name}.xml"))
    }

    #[test]
    fn the_newest_is_first_a_path_is_there_once_and_the_list_is_capped() {
        let mut recent = Recent::new();
        for name in ["a", "b", "c"] {
            recent.push(p(name));
        }
        assert_eq!(recent.paths(), [p("c"), p("b"), p("a")]);
        recent.push(p("a"));
        assert_eq!(recent.paths(), [p("a"), p("c"), p("b")], "opened again: moved to the front, not added");
        for i in 0..40 {
            recent.push(p(&format!("n{i}")));
        }
        assert_eq!(recent.paths().len(), MAX_RECENT);
        assert_eq!(recent.paths().first(), Some(&p("n39")));
        recent.remove(&p("n39"));
        assert_eq!(recent.paths().first(), Some(&p("n38")));
        recent.remove(&p("never-there"));
        recent.clear();
        assert!(recent.is_empty());
    }

    #[test]
    fn a_saved_list_is_read_back_without_repeats_and_cut_to_size() {
        let saved = [p("a"), p("b"), p("a"), p("c")].into_iter().chain((0..30).map(|i| p(&format!("n{i}"))));
        let recent = Recent::from_paths(saved);
        assert_eq!(recent.paths().len(), MAX_RECENT);
        assert_eq!(&recent.paths()[..3], [p("a"), p("b"), p("c")], "the order kept, the second `a` dropped");
        assert_eq!(Recent::from_paths(std::iter::empty()), Recent::new());
    }
}
