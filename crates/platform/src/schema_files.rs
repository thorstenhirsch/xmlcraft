//! The files the resolver reads schema documents from: any file the person can read, up to the size limit of an opened file. The
//! schema documents a schema includes and imports are asked for by the resolver, which finds them among the open documents first.

use std::path::Path;
use std::sync::Arc;

use xmlcraft_resolve::{Buffers, Environment, FileAccess, FixedClock, Policy, Resolver, normalized};

use crate::files;

/// Reads files as they are on disk.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalFiles;

impl FileAccess for LocalFiles {
    fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        files::read_file(Path::new(path)).map(|loaded| loaded.bytes).map_err(|e| format!("{path}: {e}"))
    }

    /// The real path of the file (links followed, the case the system has for it), so that a file asked for under two names is one
    /// document; as it is spelled when there is no such file.
    fn identity(&self, path: &str) -> String {
        match std::fs::canonicalize(path) {
            Ok(real) => normalized(&real.to_string_lossy()),
            Err(_) => normalized(path),
        }
    }
}

/// The resolver the schema documents of a validation are found with: the documents that are open (`buffers`) first, then the user's
/// catalogs (none yet) and the schemas that ship with the app, then files. Nothing is downloaded: there is no fetcher, and the
/// policy is offline. (Without a cache the clock is not read.)
pub fn local_resolver(buffers: Arc<dyn Buffers>) -> Resolver {
    let env = Environment { buffers, files: Arc::new(LocalFiles), fetcher: None, cache: None, clock: Arc::new(FixedClock(0)) };
    Resolver::new(env, Policy { auto_download: false, offline: true, ..Policy::default() }, &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use xmlcraft_resolve::Request;

    #[test]
    fn a_file_is_read_and_a_missing_one_says_which() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("s.xsd");
        std::fs::write(&path, b"<x/>").expect("writes");
        assert_eq!(LocalFiles.read(path.to_str().expect("utf-8")), Ok(b"<x/>".to_vec()));
        let missing = dir.path().join("gone.xsd");
        let why = LocalFiles.read(missing.to_str().expect("utf-8")).expect_err("not there");
        assert!(why.contains("gone.xsd"), "{why}");
        // A folder is not a file.
        assert!(LocalFiles.read(dir.path().to_str().expect("utf-8")).is_err());
    }

    #[test]
    fn a_file_has_one_identity_whatever_it_is_called() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("Types.xsd");
        std::fs::write(&path, b"<x/>").expect("writes");
        let id = |p: &Path| LocalFiles.identity(p.to_str().expect("utf-8"));
        // The same file by a way round.
        std::fs::create_dir(dir.path().join("sub")).expect("dir");
        assert_eq!(id(&path), id(&dir.path().join("sub").join("..").join("Types.xsd")));
        // Another file is another document.
        let other = dir.path().join("Other.xsd");
        std::fs::write(&other, b"<x/>").expect("writes");
        assert_ne!(id(&path), id(&other));
        // A link is the file it leads to.
        #[cfg(unix)]
        {
            let link = dir.path().join("link.xsd");
            std::os::unix::fs::symlink(&path, &link).expect("a link");
            assert_eq!(id(&path), id(&link));
        }
        // On a file system that folds case the names that differ in case are one file; on one that does not, the other name is no file at all.
        let folded = dir.path().join("types.xsd");
        if folded.exists() {
            assert_eq!(id(&path), id(&folded), "this file system folds case");
        } else {
            assert_ne!(id(&path), id(&folded));
        }
        // A file that is not there is known by its spelling (and asking is no error).
        let missing = dir.path().join("gone.xsd");
        assert!(id(&missing).ends_with("gone.xsd"));
    }

    struct Open(Vec<(String, Arc<str>)>);

    impl Buffers for Open {
        fn text(&self, location: &str) -> Option<Arc<str>> {
            self.0.iter().find(|(l, _)| l == location).map(|(_, t)| t.clone())
        }
    }

    #[test]
    fn the_resolver_prefers_an_open_document_to_the_file_and_never_goes_to_the_network() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("s.xsd");
        std::fs::write(&path, b"<on-disk/>").expect("writes");
        let location = path.to_str().expect("utf-8").to_owned();
        let resolve = |open: Vec<(String, Arc<str>)>, uri: &str| {
            let resolver = local_resolver(Arc::new(Open(open)));
            let mut budget = xmlcraft_resolve::Budget::default();
            resolver
                .resolve(&Request::uri(uri, ""), &mut budget)
                .map(|found| String::from_utf8_lossy(&found.bytes).into_owned())
                .map_err(|f| f.message)
        };
        assert_eq!(resolve(Vec::new(), &location).as_deref(), Ok("<on-disk/>"));
        assert_eq!(resolve(vec![(location.clone(), Arc::from("<edited/>"))], &location).as_deref(), Ok("<edited/>"));
        let remote = resolve(Vec::new(), "https://example.invalid/s.xsd").expect_err("not downloaded");
        assert!(!remote.is_empty());
    }
}
