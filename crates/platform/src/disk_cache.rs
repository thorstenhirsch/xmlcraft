//! The downloaded grammars, kept between runs: the resolver's [`ResourceCache`] on disk.
//!
//! One file per URL, named by the SHA-256 of the URL, in a folder of the operating system's cache location (or the one named by the
//! environment variable `XMLCRAFT_CACHE_DIR`). A file is [`MAGIC`], a line of JSON saying where the copy came from and what the
//! server said about it, and the body. Whatever is in the folder is untrusted: a file that is too large, cut short, not in this form,
//! or for another URL than the one asked for is a miss, never an error, and a write that fails is forgotten (the next run downloads
//! again). When the folder grows past [`DEFAULT_MAX_BYTES`] the copies fetched longest ago are removed.

use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use xmlcraft_resolve::{CacheEntry, MAX_DOWNLOAD_BYTES, ResourceCache};

/// What a cache file starts with.
const MAGIC: &[u8] = b"XMLCRAFT-CACHE 1\n";
/// The suffix of a cache file; nothing else in the folder is read or removed.
const SUFFIX: &str = ".entry";
/// The most the folder keeps (the copies fetched longest ago go first).
pub const DEFAULT_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// The most a cache file may take: a body of the largest size a download may have, and a line of header.
const MAX_FILE_BYTES: u64 = MAX_DOWNLOAD_BYTES as u64 + 64 * 1024;

/// The line of JSON after [`MAGIC`].
#[derive(Serialize, Deserialize)]
struct Header {
    url: String,
    final_url: String,
    etag: Option<String>,
    last_modified: Option<String>,
    fetched_at: i64,
}

/// Which operating system's rules [`cache_dir_for`] follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Mac,
    Windows,
    /// Linux and the other Unixes (the XDG rules).
    Other,
}

/// The folder downloaded grammars are kept in, given the environment: `XMLCRAFT_CACHE_DIR` when it names one; else
/// `~/Library/Caches/XmlCraft/schemas` on macOS, `%LOCALAPPDATA%\XmlCraft\Cache\schemas` on Windows and
/// `$XDG_CACHE_HOME/xmlcraft/schemas` (`~/.cache/xmlcraft/schemas`) elsewhere. `None` when there is no home to put it in.
pub fn cache_dir_for(os: Os, var: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |name: &str| var(name).filter(|value| !value.is_empty()).map(PathBuf::from);
    if let Some(dir) = set("XMLCRAFT_CACHE_DIR") {
        return Some(dir);
    }
    match os {
        Os::Mac => set("HOME").map(|home| home.join("Library").join("Caches").join("XmlCraft").join("schemas")),
        Os::Windows => set("LOCALAPPDATA").map(|base| base.join("XmlCraft").join("Cache").join("schemas")),
        Os::Other => set("XDG_CACHE_HOME")
            .filter(|dir| dir.is_absolute())
            .or_else(|| set("HOME").map(|home| home.join(".cache")))
            .map(|base| base.join("xmlcraft").join("schemas")),
    }
}

/// [`cache_dir_for`] for this operating system and the real environment.
pub fn default_cache_dir() -> Option<PathBuf> {
    let os = if cfg!(target_os = "macos") {
        Os::Mac
    } else if cfg!(windows) {
        Os::Windows
    } else {
        Os::Other
    };
    cache_dir_for(os, &|name| std::env::var_os(name))
}

/// A cache in a folder.
#[derive(Clone, Debug)]
pub struct DiskCache {
    dir: PathBuf,
    max_bytes: u64,
}

impl DiskCache {
    /// A cache in `dir` (made when something is first stored), keeping up to [`DEFAULT_MAX_BYTES`].
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), max_bytes: DEFAULT_MAX_BYTES }
    }

    /// A cache that keeps up to `max_bytes` (what is stored last is kept even if it alone is larger).
    #[must_use]
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path_of(&self, url: &str) -> PathBuf {
        let digest = Sha256::digest(url.as_bytes());
        let mut name = String::with_capacity(digest.len() * 2 + SUFFIX.len());
        for byte in digest {
            name.push_str(&format!("{byte:02x}"));
        }
        name.push_str(SUFFIX);
        self.dir.join(name)
    }

    /// The cache files, with the time each was written and its size.
    fn files(&self) -> Vec<(PathBuf, std::time::SystemTime, u64)> {
        let Ok(entries) = fs::read_dir(&self.dir) else { return Vec::new() };
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_str().is_some_and(|name| name.ends_with(SUFFIX)))
            .filter_map(|entry| {
                let meta = entry.metadata().ok().filter(fs::Metadata::is_file)?;
                Some((entry.path(), meta.modified().ok()?, meta.len()))
            })
            .collect()
    }

    /// How many copies are kept.
    pub fn len(&self) -> usize {
        self.files().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Remove every copy; how many there were (those that could not be removed are not counted).
    pub fn clear(&self) -> usize {
        self.files().into_iter().filter(|(path, ..)| fs::remove_file(path).is_ok()).count()
    }

    /// Remove the copies written longest ago until what is left fits, keeping `keep`.
    fn trim(&self, keep: &Path) {
        let mut files = self.files();
        let mut total: u64 = files.iter().map(|(.., len)| *len).sum();
        if total <= self.max_bytes {
            return;
        }
        files.sort_by_key(|(_, modified, _)| *modified);
        for (path, _, len) in files {
            if total <= self.max_bytes {
                break;
            }
            if path != keep && fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
            }
        }
    }
}

fn encode(entry: &CacheEntry) -> Option<Vec<u8>> {
    let header = Header {
        url: entry.url.clone(),
        final_url: entry.final_url.clone(),
        etag: entry.etag.clone(),
        last_modified: entry.last_modified.clone(),
        fetched_at: entry.fetched_at,
    };
    // JSON on one line: a newline inside a string is written `\n`.
    let line = serde_json::to_vec(&header).ok()?;
    let mut bytes = Vec::with_capacity(MAGIC.len() + line.len() + 1 + entry.body.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&line);
    bytes.push(b'\n');
    bytes.extend_from_slice(&entry.body);
    Some(bytes)
}

fn decode(bytes: &[u8], url: &str) -> Option<CacheEntry> {
    let rest = bytes.strip_prefix(MAGIC)?;
    let end = rest.iter().position(|b| *b == b'\n')?;
    let header: Header = serde_json::from_slice(rest.get(..end)?).ok()?;
    // A file for another URL (a name that was made up to look like this one) is not this URL's copy.
    if header.url != url {
        return None;
    }
    let body = rest.get(end + 1..)?.to_vec();
    Some(CacheEntry {
        url: header.url,
        final_url: header.final_url,
        body,
        etag: header.etag,
        last_modified: header.last_modified,
        fetched_at: header.fetched_at,
    })
}

impl ResourceCache for DiskCache {
    fn get(&self, url: &str) -> Option<CacheEntry> {
        let file = fs::File::open(self.path_of(url)).ok()?;
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        let mut bytes = Vec::new();
        // One byte more than may be, so that a file that is too large is noticed without reading all of it.
        file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return None;
        }
        decode(&bytes, url)
    }

    fn put(&self, entry: CacheEntry) {
        if entry.body.len() > MAX_DOWNLOAD_BYTES {
            return;
        }
        let Some(bytes) = encode(&entry) else { return };
        let path = self.path_of(&entry.url);
        let stored = fs::create_dir_all(&self.dir).and_then(|()| craft_atomic::atomic_write(&path, &bytes));
        match stored {
            Ok(()) => self.trim(&path),
            Err(e) => log::debug!("{}: the copy is not kept: {e}", entry.url),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(url: &str, body: &[u8], fetched_at: i64) -> CacheEntry {
        CacheEntry {
            url: url.to_owned(),
            final_url: format!("{url}#final"),
            body: body.to_vec(),
            etag: Some("\"abc\"".to_owned()),
            last_modified: Some("Tue, 15 Nov 1994 12:45:26 GMT".to_owned()),
            fetched_at,
        }
    }

    fn cache() -> (tempfile::TempDir, DiskCache) {
        let dir = tempfile::tempdir().expect("temp dir");
        let cache = DiskCache::new(dir.path().join("schemas"));
        (dir, cache)
    }

    #[test]
    fn a_copy_comes_back_as_it_was_stored_after_a_restart() {
        let (_dir, cache) = cache();
        let stored = entry("http://example.com/a.xsd", b"<xs:schema/>\n\xff\x00 any bytes", 1234);
        assert_eq!(cache.get(&stored.url), None);
        cache.put(stored.clone());
        assert_eq!(cache.get(&stored.url), Some(stored.clone()));
        // Another instance over the same folder is a new run.
        assert_eq!(DiskCache::new(cache.dir()).get(&stored.url), Some(stored.clone()));
        // Storing again replaces it.
        let renewed = CacheEntry { fetched_at: 9999, ..stored.clone() };
        cache.put(renewed.clone());
        assert_eq!(cache.get(&stored.url), Some(renewed));
        assert_eq!(cache.len(), 1);
        // A copy with nothing the server said about it, and an empty body, are copies too.
        let bare = CacheEntry { etag: None, last_modified: None, ..entry("http://example.com/empty.xsd", b"", 1) };
        cache.put(bare.clone());
        assert_eq!(cache.get(&bare.url), Some(bare));
    }

    #[test]
    fn urls_that_differ_are_different_copies() {
        let (_dir, cache) = cache();
        for (url, body) in [("http://example.com/a.xsd", "a"), ("https://example.com/a.xsd", "b"), ("http://example.com/A.xsd", "c")] {
            cache.put(entry(url, body.as_bytes(), 1));
        }
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.get("https://example.com/a.xsd").map(|e| e.body), Some(b"b".to_vec()));
        assert_eq!(cache.get("http://example.com/A.xsd").map(|e| e.body), Some(b"c".to_vec()));
    }

    #[test]
    fn what_is_in_the_folder_is_never_trusted() {
        let (_dir, cache) = cache();
        let url = "http://example.com/a.xsd";
        cache.put(entry(url, b"body", 1));
        let path = cache.path_of(url);
        let good = fs::read(&path).expect("the file");
        let corrupted: Vec<Vec<u8>> = vec![
            Vec::new(),
            b"garbage".to_vec(),
            MAGIC.to_vec(),
            [MAGIC, b"{not json}\nbody"].concat(),
            [MAGIC, b"{\"url\":\"http://example.com/a.xsd\"}\nbody"].concat(), // the header lacks what it needs
            good[..good.len() / 2].to_vec(),                                   // cut short
            [MAGIC, b"{\"url\":\"http://example.com/other.xsd\",\"final_url\":\"\",\"etag\":null,\"last_modified\":null,\"fetched_at\":1}\nx"]
                .concat(),
            vec![0xff; 100],
        ];
        for bytes in corrupted {
            fs::write(&path, &bytes).expect("writes");
            assert_eq!(cache.get(url), None, "{bytes:?}");
        }
        // A file larger than any copy may be is not read in, though it is a good file otherwise.
        let huge = encode(&entry(url, &vec![b'x'; usize::try_from(MAX_FILE_BYTES).expect("fits")], 1)).expect("encodes");
        fs::write(&path, &huge).expect("writes");
        assert_eq!(cache.get(url), None);
        // One that fits is read.
        let fits = encode(&entry(url, &vec![b'x'; MAX_DOWNLOAD_BYTES], 1)).expect("encodes");
        fs::write(&path, &fits).expect("writes");
        assert_eq!(cache.get(url).map(|e| e.body.len()), Some(MAX_DOWNLOAD_BYTES));
        // A folder where a file should be.
        fs::remove_file(&path).expect("removes");
        fs::create_dir(&path).expect("dir");
        assert_eq!(cache.get(url), None);
        // And a good file is good again once it is stored again.
        fs::remove_dir(&path).expect("removes");
        cache.put(entry(url, b"body", 1));
        assert!(cache.get(url).is_some());
    }

    #[test]
    fn a_cache_that_cannot_store_forgets() {
        let dir = tempfile::tempdir().expect("temp dir");
        // The folder is under a file, so it cannot be made.
        let file = dir.path().join("file");
        fs::write(&file, b"x").expect("writes");
        let cache = DiskCache::new(file.join("schemas"));
        cache.put(entry("http://example.com/a.xsd", b"body", 1));
        assert_eq!(cache.get("http://example.com/a.xsd"), None);
        assert!(cache.is_empty());
    }

    #[test]
    fn the_oldest_copies_go_when_the_folder_grows_too_large() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cache = DiskCache::new(dir.path().join("schemas")).with_max_bytes(2500);
        let body = vec![b'x'; 1000];
        for (i, name) in ["a", "b", "c"].into_iter().enumerate() {
            cache.put(entry(&format!("http://example.com/{name}.xsd"), &body, 1));
            // The files must differ in age.
            let path = cache.path_of(&format!("http://example.com/{name}.xsd"));
            let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000 + i as u64);
            fs::File::options().write(true).open(&path).expect("opens").set_modified(when).expect("sets the time");
        }
        // Three copies of about 1.1 kB do not fit in 2.5 kB: the oldest is gone.
        assert_eq!(cache.len(), 2);
        assert!(cache.get("http://example.com/a.xsd").is_none());
        assert!(cache.get("http://example.com/b.xsd").is_some());
        assert!(cache.get("http://example.com/c.xsd").is_some());
        // What is stored last stays even when it alone is too large.
        let tiny = DiskCache::new(dir.path().join("schemas")).with_max_bytes(10);
        tiny.put(entry("http://example.com/d.xsd", &body, 1));
        assert!(tiny.get("http://example.com/d.xsd").is_some());
        assert_eq!(tiny.len(), 1);
    }

    #[test]
    fn a_body_larger_than_a_download_may_be_is_not_kept() {
        let (_dir, cache) = cache();
        cache.put(entry("http://example.com/big.xsd", &vec![b'x'; MAX_DOWNLOAD_BYTES + 1], 1));
        assert!(cache.is_empty());
    }

    #[test]
    fn clearing_removes_the_copies_and_nothing_else() {
        let (_dir, cache) = cache();
        cache.put(entry("http://example.com/a.xsd", b"a", 1));
        cache.put(entry("http://example.com/b.xsd", b"b", 1));
        let other = cache.dir().join("notes.txt");
        fs::write(&other, b"mine").expect("writes");
        assert_eq!(cache.clear(), 2);
        assert!(cache.is_empty());
        assert_eq!(fs::read(&other).expect("still there"), b"mine");
        // Clearing a cache that was never used is fine.
        assert_eq!(DiskCache::new(cache.dir().join("never")).clear(), 0);
    }

    #[test]
    fn the_folder_follows_each_system_s_rules() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| OsString::from(*v))
        };
        let dir = |os, pairs| cache_dir_for(os, &env(pairs));
        assert_eq!(dir(Os::Mac, &[("HOME", "/Users/a")]), Some(PathBuf::from("/Users/a/Library/Caches/XmlCraft/schemas")));
        assert_eq!(
            dir(Os::Windows, &[("LOCALAPPDATA", "C:\\Users\\a\\AppData\\Local")]),
            Some(PathBuf::from("C:\\Users\\a\\AppData\\Local").join("XmlCraft").join("Cache").join("schemas"))
        );
        assert_eq!(dir(Os::Other, &[("HOME", "/home/a")]), Some(PathBuf::from("/home/a/.cache/xmlcraft/schemas")));
        assert_eq!(dir(Os::Other, &[("HOME", "/home/a"), ("XDG_CACHE_HOME", "/var/c")]), Some(PathBuf::from("/var/c/xmlcraft/schemas")));
        // XDG says a relative path is to be ignored.
        assert_eq!(dir(Os::Other, &[("HOME", "/home/a"), ("XDG_CACHE_HOME", "rel")]), Some(PathBuf::from("/home/a/.cache/xmlcraft/schemas")));
        // The variable that names the folder wins everywhere; an empty one is not set.
        for os in [Os::Mac, Os::Windows, Os::Other] {
            assert_eq!(dir(os, &[("XMLCRAFT_CACHE_DIR", "/x/y")]), Some(PathBuf::from("/x/y")));
            assert_eq!(dir(os, &[("XMLCRAFT_CACHE_DIR", "")]), None);
        }
        // No home, no folder.
        assert_eq!(dir(Os::Mac, &[]), None);
        assert_eq!(dir(Os::Windows, &[("HOME", "/h")]), None);
        assert_eq!(dir(Os::Other, &[]), None);
    }
}
