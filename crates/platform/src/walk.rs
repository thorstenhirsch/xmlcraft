//! Listing the files under a folder, for a search over many files: the folder's tree is walked breadth first, symbolic links are not
//! followed (a link could lead out of the folder or round in a circle), the folders that hold what is not written by hand (names that
//! start with a dot, `node_modules`, `target`) are not entered, and the walk is bounded in files, depth and folders.
//!
//! On the web there are no folders: the walk finds nothing.

use std::path::{Path, PathBuf};

/// What a walk found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Walked {
    /// The files, folder by folder, each folder's files sorted by name.
    pub files: Vec<PathBuf>,
    /// The walk stopped at a limit (files, depth or folders): there is more than is listed.
    pub truncated: bool,
    /// Folders that could not be read.
    pub unreadable: usize,
}

/// The deepest folder entered, and the most folders and files visited.
pub const MAX_DEPTH: usize = 32;
pub const MAX_FOLDERS: usize = 20_000;

/// Folder names (besides those that start with a dot) that are not entered.
#[cfg(not(target_arch = "wasm32"))]
const SKIPPED: &[&str] = &["node_modules", "target"];

/// The files under `root` for which `include` (given the file's name) says yes, at most `max_files`.
#[cfg(not(target_arch = "wasm32"))]
pub fn files(root: &Path, include: &dyn Fn(&str) -> bool, max_files: usize) -> Walked {
    use std::collections::VecDeque;
    use std::fs;

    let mut walked = Walked::default();
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    queue.push_back((root.to_path_buf(), 0));
    let mut folders = 0usize;
    while let Some((folder, depth)) = queue.pop_front() {
        folders += 1;
        if folders > MAX_FOLDERS {
            walked.truncated = true;
            break;
        }
        let Ok(entries) = fs::read_dir(&folder) else {
            walked.unreadable += 1;
            continue;
        };
        let mut here: Vec<(String, PathBuf, bool)> = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            let Ok(kind) = entry.file_type() else { continue };
            // (A link is neither followed nor listed.)
            if kind.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            here.push((name, entry.path(), kind.is_dir()));
        }
        here.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, path, is_dir) in here {
            if is_dir {
                if name.starts_with('.') || SKIPPED.contains(&name.as_str()) {
                    continue;
                }
                if depth + 1 > MAX_DEPTH {
                    walked.truncated = true;
                } else {
                    queue.push_back((path, depth + 1));
                }
            } else if include(&name) {
                if walked.files.len() >= max_files {
                    walked.truncated = true;
                    return walked;
                }
                walked.files.push(path);
            }
        }
    }
    walked
}

#[cfg(target_arch = "wasm32")]
pub fn files(_root: &Path, _include: &dyn Fn(&str) -> bool, _max_files: usize) -> Walked {
    Walked::default()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::fs;

    use super::*;

    fn touch(dir: &Path, name: &str) {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        fs::write(path, "x").expect("writes");
    }

    fn names(root: &Path, walked: &Walked) -> Vec<String> {
        walked.files.iter().map(|p| p.strip_prefix(root).expect("inside").to_string_lossy().replace('\\', "/")).collect()
    }

    #[test]
    fn files_are_listed_folder_by_folder_without_hidden_build_or_linked_folders() {
        let dir = tempfile::tempdir().expect("dir");
        let root = dir.path();
        for f in ["b.xml", "a.xml", "sub/c.xml", "sub/deep/d.xml", ".git/config.xml", "node_modules/m.xml", "target/t.xml", "note.txt"] {
            touch(root, f);
        }
        let all = files(root, &|_| true, 100);
        assert_eq!(names(root, &all), ["a.xml", "b.xml", "note.txt", "sub/c.xml", "sub/deep/d.xml"]);
        assert!(!all.truncated && all.unreadable == 0);
        let xml = files(root, &|n| n.ends_with(".xml"), 100);
        assert_eq!(names(root, &xml), ["a.xml", "b.xml", "sub/c.xml", "sub/deep/d.xml"]);
        // A link is not followed, even to a folder outside (or a loop).
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().expect("outside");
            touch(outside.path(), "secret.xml");
            std::os::unix::fs::symlink(outside.path(), root.join("link")).expect("link");
            std::os::unix::fs::symlink(root, root.join("loop")).expect("loop");
            assert_eq!(names(root, &files(root, &|n| n.ends_with(".xml"), 100)), ["a.xml", "b.xml", "sub/c.xml", "sub/deep/d.xml"]);
        }
    }

    #[test]
    fn the_walk_stops_at_its_limits_and_says_so() {
        let dir = tempfile::tempdir().expect("dir");
        for i in 0..10 {
            touch(dir.path(), &format!("f{i}.xml"));
        }
        let few = files(dir.path(), &|_| true, 4);
        assert_eq!((few.files.len(), few.truncated), (4, true));
        let missing = files(&dir.path().join("nothing"), &|_| true, 4);
        assert_eq!((missing.files.len(), missing.unreadable), (0, 1));
        // Folders nested past the depth are not entered.
        let mut deep = dir.path().to_path_buf();
        for _ in 0..(MAX_DEPTH + 3) {
            deep.push("d");
        }
        fs::create_dir_all(&deep).expect("dirs");
        fs::write(deep.join("far.xml"), "x").expect("writes");
        let walked = files(dir.path(), &|n| n == "far.xml", 10);
        assert!(walked.files.is_empty() && walked.truncated, "{walked:?}");
    }
}
