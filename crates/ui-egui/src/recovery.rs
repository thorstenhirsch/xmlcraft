//! Autosave recovery: the text of every document with unsaved changes is kept on disk (see `xmlcraft_platform::recovery`) a few seconds
//! after it changes, and removed when the document is saved, closed or the app ends normally. What is left from a session that did not
//! end normally (a crash, a power cut, a kill) is offered at the next start: Recover opens each as a document with its edits, unsaved,
//! and Discard deletes them. Nothing here touches the documents' own files.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use xmlcraft_platform::recovery::{self, Snapshot};

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::i18n::{fmt, tl};

/// How long after a change (at the next look) a snapshot is taken.
const INTERVAL: Duration = Duration::from_secs(5);
/// A text larger than this is not kept (a person was not typing it).
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// What the app knows of recovery.
pub(crate) struct Recovery {
    dir: Option<PathBuf>,
    /// Names this session's snapshots (the time it started, in hex): the snapshots of an earlier session are not overwritten.
    run: String,
    interval: Duration,
    last_look: Option<Instant>,
    /// The version of each document that was written as its snapshot.
    taken: HashMap<u64, u64>,
    /// What an earlier session left, waiting for the person to say.
    found: Vec<Snapshot>,
}

impl Default for Recovery {
    fn default() -> Self {
        let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Self { dir: None, run: format!("{started:x}"), interval: INTERVAL, last_look: None, taken: HashMap::new(), found: Vec::new() }
    }
}

impl Recovery {
    fn key(&self, id: u64) -> String {
        format!("{}-{id}", self.run)
    }
}

/// Run `job` on a thread (the files are small, but a disk may be slow): without threads, on the web, there is nothing to do.
fn in_background(job: impl FnOnce() + Send + 'static) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = std::thread::Builder::new().name("recovery".into()).spawn(job);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = job;
    }
}

impl XmlCraftApp {
    /// Keep recoverable text in `dir` (none: not at all), and offer what an earlier session left there.
    pub fn set_recovery_dir(&mut self, dir: Option<PathBuf>) {
        self.recovery.found = dir.as_deref().map(recovery::list).unwrap_or_default();
        self.recovery.dir = dir;
        self.offer_recovery();
    }

    /// How long a change waits before it is written (tests make it short).
    pub fn set_recovery_interval(&mut self, interval: Duration) {
        self.recovery.interval = interval;
    }

    /// How many documents an earlier session left that the person has not decided about.
    pub fn recoverable(&self) -> usize {
        self.recovery.found.len()
    }

    fn offer_recovery(&mut self) {
        if self.recovery.found.is_empty() || self.dialog.is_some() {
            return;
        }
        let names: Vec<String> = self.recovery.found.iter().take(8).map(|s| s.name.clone()).collect();
        self.dialog = Some(Dialog::Recover { count: self.recovery.found.len(), names: names.join(", ") });
    }

    /// Write a snapshot of each document with changes that were not written yet, and drop the snapshot of one that is saved. Called
    /// every frame; it looks at most once an interval.
    pub(crate) fn recovery_tick(&mut self, ctx: &egui::Context) {
        let Some(dir) = self.recovery.dir.clone() else { return };
        // (What an earlier session left is asked about first, when nothing else is being asked.)
        if !self.recovery.found.is_empty() {
            self.offer_recovery();
        }
        let now = Instant::now();
        if self.recovery.last_look.is_some_and(|t| now.saturating_duration_since(t) < self.recovery.interval) {
            ctx.request_repaint_after(self.recovery.interval);
            return;
        }
        self.recovery.last_look = Some(now);
        let mut write: Vec<Snapshot> = Vec::new();
        let mut clean: Vec<u64> = Vec::new();
        for doc in self.session.documents() {
            let (id, version) = (doc.id(), doc.editor().version());
            if !doc.is_dirty() {
                if self.recovery.taken.contains_key(&id) {
                    clean.push(id);
                }
                continue;
            }
            if self.recovery.taken.get(&id) == Some(&version) || doc.editor().buffer().len() > MAX_BYTES {
                continue;
            }
            self.recovery.taken.insert(id, version);
            write.push(Snapshot {
                key: self.recovery.key(id),
                name: doc.name().to_owned(),
                path: doc.path().map(|p| p.to_string_lossy().into_owned()),
                text: doc.editor().text(),
            });
        }
        for id in &clean {
            self.recovery.taken.remove(id);
        }
        let removed: Vec<String> = clean.iter().map(|id| self.recovery.key(*id)).collect();
        if !write.is_empty() || !removed.is_empty() {
            in_background(move || {
                for snapshot in &write {
                    if let Err(e) = recovery::write(&dir, snapshot) {
                        log::warn!("couldn't keep the unsaved text of {}: {e}", snapshot.name);
                    }
                }
                for key in &removed {
                    recovery::remove(&dir, key);
                }
            });
        }
        ctx.request_repaint_after(self.recovery.interval);
    }

    /// A document was closed: what was kept of it is not needed.
    pub(crate) fn recovery_forget(&mut self, id: u64) {
        if self.recovery.taken.remove(&id).is_some()
            && let Some(dir) = self.recovery.dir.clone()
        {
            let key = self.recovery.key(id);
            in_background(move || recovery::remove(&dir, &key));
        }
    }

    /// The app is ending the way it should: nothing is left to recover from this session.
    pub(crate) fn recovery_finish(&mut self) {
        if let Some(dir) = &self.recovery.dir {
            recovery::remove_with_prefix(dir, &self.recovery.run);
        }
        self.recovery.taken.clear();
    }

    /// Open what an earlier session left, with its edits and unsaved, and delete the snapshots.
    pub(crate) fn recover_found(&mut self) {
        let found = std::mem::take(&mut self.recovery.found);
        let mut recovered = 0usize;
        for snapshot in &found {
            let mut opened = false;
            if let Some(path) = snapshot.path.as_deref().map(std::path::Path::new).filter(|p| p.is_file()) {
                // The file it belonged to, with the unsaved text on top: the saved text is what undo goes back to.
                if self.open_path(path)
                    && let Some(doc) = self.session.active_document_mut()
                {
                    if doc.editor().text() == snapshot.text {
                        // Nothing was unsaved after all.
                        opened = true;
                    } else {
                        let editor = doc.editor_mut();
                        editor.select_all();
                        editor.type_text(&snapshot.text);
                        opened = true;
                        recovered += 1;
                    }
                }
            }
            if !opened {
                self.open_generated(&snapshot.name, &snapshot.text);
                recovered += 1;
            }
        }
        if let Some(dir) = self.recovery.dir.clone() {
            for s in &found {
                recovery::remove(&dir, &s.key);
            }
        }
        self.status = fmt(tl!("Recovered {count} documents with unsaved changes"), &[("count", &recovered.to_string())]);
    }

    /// Delete what an earlier session left.
    pub(crate) fn discard_found(&mut self) {
        let found = std::mem::take(&mut self.recovery.found);
        if let Some(dir) = self.recovery.dir.clone() {
            for s in &found {
                recovery::remove(&dir, &s.key);
            }
        }
    }
}
