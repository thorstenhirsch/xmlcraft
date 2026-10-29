//! Native file pickers that never block the frame.
//!
//! A blocking dialog on macOS runs a nested event loop inside winit's handler; an event that arrives while it spins re-enters the
//! handler and the app aborts. `rfd::AsyncFileDialog` returns at once instead: the panel is shown when the future is created (on the main
//! thread, here), a worker thread waits for the answer, and the app uses it on a later frame ([`XmlCraftApp::process_picks`]).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::XmlCraftApp;
use crate::documents::AfterClose;

/// What a chosen path is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))] // only the native panels ask
pub(crate) enum PickFor {
    Open,
    /// A schema or a DTD for the Assign Schema dialog.
    SchemaFile,
    /// Save the document with this id under the chosen name; afterwards do `then`.
    SaveAs {
        id: u64,
        then: Option<AfterClose>,
    },
}

struct Done {
    what: PickFor,
    /// The chosen paths; empty when the picker was cancelled.
    paths: Vec<PathBuf>,
}

#[derive(Default)]
pub(crate) struct Pickers {
    /// A picker is showing or its answer is waiting for a frame; a second request is refused until then.
    showing: Arc<AtomicBool>,
    done: Arc<Mutex<Vec<Done>>>,
}

impl std::fmt::Debug for Pickers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pickers").field("showing", &self.showing.load(Ordering::SeqCst)).finish_non_exhaustive()
    }
}

/// Clears the flag if the worker ends without queueing an answer (it panicked).
#[cfg(not(target_arch = "wasm32"))]
struct Claim {
    flag: Arc<AtomicBool>,
    ctx: egui::Context,
    handed_over: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl Claim {
    /// The answer is queued: the frame that uses it clears the flag.
    fn hand_over(mut self) {
        self.handed_over = true;
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Claim {
    fn drop(&mut self) {
        if !self.handed_over {
            self.flag.store(false, Ordering::SeqCst);
            self.ctx.request_repaint();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl XmlCraftApp {
    /// Show the Open panel. Returns `false` when another panel is already showing.
    pub(crate) fn pick_open(&mut self, ctx: &egui::Context) -> bool {
        // No extension filter: opening by drag and drop or from the command line takes any file, and so should the panel (XML lives in
        // files with many other extensions, and a chooser that enforces a filter would hide them).
        let dialog = rfd::AsyncFileDialog::new().set_title(crate::i18n::tl!("Open"));
        self.ask(ctx, PickFor::Open, Ask::Open(dialog))
    }

    /// Show the panel that chooses the schema or the DTD to assign to a document.
    pub(crate) fn pick_schema_file(&mut self, ctx: &egui::Context) -> bool {
        let dialog = rfd::AsyncFileDialog::new()
            .set_title(crate::i18n::tl!("Choose a schema or a DTD"))
            .add_filter(crate::i18n::tl!("Schemas and DTDs"), &["xsd", "dtd", "ent", "mod", "xml"]);
        self.ask(ctx, PickFor::SchemaFile, Ask::Open(dialog))
    }

    /// Show the Save As panel for the document with this id.
    pub(crate) fn pick_save_as(&mut self, ctx: &egui::Context, id: u64, then: Option<AfterClose>) -> bool {
        let name = self.session.position_of(id).and_then(|i| self.session.document(i)).map(|d| d.name().to_owned()).unwrap_or_default();
        let mut dialog = rfd::AsyncFileDialog::new().set_title(crate::i18n::tl!("Save As")).set_file_name(if name.contains('.') {
            name
        } else {
            format!("{name}.xml")
        });
        if let Some(dir) =
            self.session.position_of(id).and_then(|i| self.session.document(i)).and_then(|d| d.path()).and_then(std::path::Path::parent)
        {
            dialog = dialog.set_directory(dir);
        }
        self.ask(ctx, PickFor::SaveAs { id, then }, Ask::Save(dialog))
    }

    fn ask(&mut self, ctx: &egui::Context, what: PickFor, ask: Ask) -> bool {
        if self.pickers.showing.swap(true, Ordering::SeqCst) {
            return false;
        }
        // Tests and automation answer instead of a native panel; the answer still arrives on a later frame, like a real one.
        if let Some(paths) = self.pick_override.take() {
            self.pickers.done.lock().unwrap_or_else(PoisonError::into_inner).push(Done { what, paths });
            ctx.request_repaint();
            return true;
        }
        // On macOS creating the future shows the panel, so it must happen here, on the main thread.
        let one = |h: Option<rfd::FileHandle>| h.map(|h| h.path().to_path_buf()).into_iter().collect::<Vec<_>>();
        let started = match ask {
            Ask::Open(dialog) => {
                let pick = dialog.pick_files();
                self.spawn_waiter(ctx, what, async move { pick.await.unwrap_or_default().into_iter().map(|h| h.path().to_path_buf()).collect() })
            }
            Ask::Save(dialog) => {
                let pick = dialog.save_file();
                self.spawn_waiter(ctx, what, async move { one(pick.await) })
            }
        };
        if !started {
            self.pickers.showing.store(false, Ordering::SeqCst);
            self.status = crate::i18n::tl!("Couldn't show the file picker. Please try again.").to_owned();
        }
        started
    }

    fn spawn_waiter(&self, ctx: &egui::Context, what: PickFor, pick: impl std::future::Future<Output = Vec<PathBuf>> + Send + 'static) -> bool {
        let claim = Claim { flag: self.pickers.showing.clone(), ctx: ctx.clone(), handed_over: false };
        let done = self.pickers.done.clone();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("file-picker".into())
            .spawn(move || {
                let paths = pollster::block_on(pick);
                done.lock().unwrap_or_else(PoisonError::into_inner).push(Done { what, paths });
                claim.hand_over();
                ctx.request_repaint();
            })
            .is_ok()
    }
}

#[cfg(target_arch = "wasm32")]
impl XmlCraftApp {
    /// The web build has no native file panels.
    pub(crate) fn pick_open(&mut self, _ctx: &egui::Context) -> bool {
        self.status = crate::i18n::tl!("Opening files is not available in the browser yet").to_owned();
        false
    }

    pub(crate) fn pick_schema_file(&mut self, _ctx: &egui::Context) -> bool {
        self.status = crate::i18n::tl!("Opening files is not available in the browser yet").to_owned();
        false
    }

    pub(crate) fn pick_save_as(&mut self, _ctx: &egui::Context, _id: u64, _then: Option<AfterClose>) -> bool {
        self.status = crate::i18n::tl!("Saving files is not available in the browser yet").to_owned();
        false
    }
}

impl XmlCraftApp {
    /// Use the answers that arrived since the last frame. Called every frame.
    pub(crate) fn process_picks(&mut self, ctx: &egui::Context) {
        let done = std::mem::take(&mut *self.pickers.done.lock().unwrap_or_else(PoisonError::into_inner));
        for Done { what, paths } in done {
            self.pickers.showing.store(false, Ordering::SeqCst);
            match (what, paths.is_empty()) {
                (PickFor::Open, false) => self.open_paths(paths.iter().map(PathBuf::as_path)),
                (PickFor::SchemaFile, false) => {
                    if let Some(path) = paths.first() {
                        self.picked_schema_file(path);
                    }
                }
                (PickFor::SaveAs { id, then }, false) => {
                    if let Some(path) = paths.into_iter().next() {
                        self.save_document_as(ctx, id, path, then);
                    }
                }
                // Cancelled: a quit that was waiting for this answer is off.
                (PickFor::SaveAs { .. }, true) => self.quitting = false,
                (PickFor::Open | PickFor::SchemaFile, true) => {}
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
enum Ask {
    Open(rfd::AsyncFileDialog),
    Save(rfd::AsyncFileDialog),
}
