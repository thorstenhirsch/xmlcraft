//! The cache of downloaded resources: what is kept, when a copy is current, and a cache in memory.

use std::collections::HashMap;
use std::sync::Mutex;

/// One downloaded resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheEntry {
    /// The URL that was asked for (the key).
    pub url: String,
    /// Where it came from after redirects.
    pub final_url: String,
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// When it was fetched or last found current, in seconds since 1970-01-01 UTC.
    pub fetched_at: i64,
}

impl CacheEntry {
    /// Whether the copy is current without asking: fetched less than `fresh_for` seconds before `now`. A copy from the future (the
    /// clock was set back) is not.
    pub fn is_fresh(&self, now: i64, fresh_for: i64) -> bool {
        now.checked_sub(self.fetched_at).is_some_and(|age| (0..fresh_for).contains(&age))
    }
}

/// Where downloaded resources are kept between runs. Reading and writing must not panic or block for long; a cache that cannot store
/// something just forgets it.
pub trait ResourceCache: Send + Sync {
    fn get(&self, url: &str) -> Option<CacheEntry>;
    fn put(&self, entry: CacheEntry);
    /// How much is kept (a cache that cannot tell says nothing is).
    fn stats(&self) -> CacheStats {
        CacheStats::default()
    }
    /// Forget everything; how many copies there were. A cache that cannot is left as it is.
    fn clear(&self) -> usize {
        0
    }
}

/// What a cache holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub entries: usize,
    pub bytes: u64,
}

/// A cache that lives as long as the program.
#[derive(Default)]
pub struct MemoryCache {
    entries: Mutex<HashMap<String, CacheEntry>>,
}

impl MemoryCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().map_or_else(|poisoned| poisoned.into_inner().len(), |e| e.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl ResourceCache for MemoryCache {
    fn get(&self, url: &str) -> Option<CacheEntry> {
        let entries = self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.get(url).cloned()
    }

    fn put(&self, entry: CacheEntry) {
        let mut entries = self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.insert(entry.url.clone(), entry);
    }

    fn stats(&self) -> CacheStats {
        let entries = self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        CacheStats { entries: entries.len(), bytes: entries.values().map(|e| e.body.len() as u64).fold(0, u64::saturating_add) }
    }

    fn clear(&self) -> usize {
        let mut entries = self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = entries.len();
        entries.clear();
        before
    }
}
