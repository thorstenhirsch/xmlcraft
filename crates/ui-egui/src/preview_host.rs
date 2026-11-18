//! The browser view of the Preview tool window, and the system-browser copy used where there is none.
//!
//! **The view** is a locked-down native webview (`wry`: WKWebView on macOS, WebView2 on Windows), a child view of the window laid
//! over the panel. It shows the page from memory, served under the custom scheme of `xmlcraft_engine::preview` with that module's
//! policy: the Content-Security-Policy and `nosniff` on every answer, JavaScript off (the webview's own switch, and the policy's
//! sandbox), only the document and its anchors navigable, no new windows, no downloads, no permissions, no devtools, no link
//! previews, a private (non-persistent) data store. Linux has no view (WebKitGTK cannot be a child of a Wayland window, and the
//! build would need GTK), and neither has the web build; there, and when the view cannot be made, the tool offers the system browser.
//!
//! The view is above everything egui draws, so it is shown only where the panel is and while nothing of egui's is above that place.
//! [`Lifecycle`] decides each frame what to do with it, from where the panel wants it: made when the panel first shows a page, moved
//! and resized with the panel (in physical pixels, so the scale factor is followed), hidden while covered or while another tab is in
//! front, destroyed when the panel closes. Its decisions are pure and tested without a window; [`Host`] carries them out.
//!
//! **The copy for the system browser** ([`BrowserCopy`]) is the page written to a folder of its own under the temporary directory
//! (owner-only on Unix, a fresh name made with `create_dir`, so never a folder someone else made), replaced when the page changes and
//! removed when the app closes. It carries the policy as a `<meta>` (`xmlcraft_engine::preview::fallback_page`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use xmlcraft_engine::preview::{Allow, Prepared, fallback_page};

/// Where the view goes, in physical pixels of the window's content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Sizes beyond this are not real windows.
const MAX_PIXELS: f32 = 100_000.0;

impl Bounds {
    /// The pixels of `rect` (egui points) at `pixels_per_point`: none for a rect that is empty, not finite or absurd.
    pub fn of(rect: egui::Rect, pixels_per_point: f32) -> Option<Self> {
        if !rect.is_finite() || !pixels_per_point.is_finite() || pixels_per_point <= 0.0 {
            return None;
        }
        let scaled = |v: f32| (v * pixels_per_point).round();
        let (x, y, right, bottom) = (scaled(rect.min.x), scaled(rect.min.y), scaled(rect.max.x), scaled(rect.max.y));
        let (width, height) = (right - x, bottom - y);
        if width < 1.0 || height < 1.0 || [x.abs(), y.abs(), width, height].iter().any(|v| *v > MAX_PIXELS) {
            return None;
        }
        // (Each value is finite, whole and within ±MAX_PIXELS, so the casts are exact.)
        Some(Self { x: x as i32, y: y as i32, width: width as u32, height: height as u32 })
    }
}

/// One thing to do to the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    /// Make it, shown, at these bounds, loading the page.
    Create(Bounds),
    Move(Bounds),
    Show,
    Hide,
    Destroy,
    /// Load the page again (it changed).
    Reload,
    /// Give the keyboard back to the window (egui), from the view.
    ReturnFocus,
}

/// What the view is, and what to do with it next frame. Only [`Host`] acts on it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Lifecycle {
    live: bool,
    visible: bool,
    bounds: Option<Bounds>,
    /// The view could not be made: why. The tool uses the system browser until the panel is closed and opened again.
    failed: Option<String>,
    /// Make the view again (whether scripts run is fixed when it is made).
    remake: bool,
    /// The page changed.
    reload: bool,
}

impl Lifecycle {
    /// What to do this frame. `open`: the panel is open (in front or not); `wanted`: where it shows the page now (none: no page, another
    /// tab is in front, or something of egui's is above it); `pressed`: a pointer button went down on egui (the view is not where egui
    /// sees the pointer, so the keyboard goes back to egui).
    pub fn frame(&mut self, open: bool, wanted: Option<Bounds>, pressed: bool) -> Vec<Op> {
        let mut ops = Vec::new();
        if !open {
            if self.live {
                ops.push(Op::Destroy);
            }
            *self = Self::default();
            return ops;
        }
        if self.failed.is_some() {
            return ops;
        }
        if self.remake && self.live {
            ops.push(Op::Destroy);
            self.live = false;
            self.visible = false;
            self.bounds = None;
        }
        self.remake = false;
        match wanted {
            Some(bounds) if !self.live => {
                ops.push(Op::Create(bounds));
                (self.live, self.visible, self.bounds, self.reload) = (true, true, Some(bounds), false);
            }
            Some(bounds) => {
                if self.bounds != Some(bounds) {
                    ops.push(Op::Move(bounds));
                    self.bounds = Some(bounds);
                }
                if !self.visible {
                    ops.push(Op::Show);
                    self.visible = true;
                }
            }
            None if self.live && self.visible => {
                ops.extend([Op::Hide, Op::ReturnFocus]);
                self.visible = false;
            }
            None => {}
        }
        if std::mem::take(&mut self.reload) && self.live {
            ops.push(Op::Reload);
        }
        if pressed && self.live && self.visible && !ops.contains(&Op::ReturnFocus) {
            ops.push(Op::ReturnFocus);
        }
        ops
    }

    /// The page changed: a live view loads it again (one made later loads it anyway).
    pub fn page_changed(&mut self) {
        self.reload = self.live;
    }

    /// Make the view again at the next frame (scripts were allowed or forbidden).
    pub fn remake(&mut self) {
        self.remake = self.live;
    }

    /// The view could not be made, or an operation on it failed: there is none now.
    pub fn fail(&mut self, why: String) {
        *self = Self { failed: Some(why), ..Self::default() };
    }

    pub fn failure(&self) -> Option<&str> {
        self.failed.as_deref()
    }

    pub fn is_live(&self) -> bool {
        self.live
    }

    pub fn is_shown(&self) -> bool {
        self.live && self.visible
    }
}

/// What the view's protocol serves: the page and what its document allows.
#[derive(Debug, Default)]
pub(crate) struct Current {
    pub page: Option<Arc<Prepared>>,
    pub allow: Allow,
}

/// Shared between the app and the view's protocol handler (which may run on another thread).
type Shared = Arc<Mutex<Current>>;

/// The page and the permissions, whatever a panicking holder left (a poisoned lock still holds the last values set).
fn read(shared: &Shared) -> (Option<Arc<Prepared>>, Allow) {
    match shared.lock() {
        Ok(current) => (current.page.clone(), current.allow),
        Err(poisoned) => {
            let current = poisoned.into_inner();
            (current.page.clone(), current.allow)
        }
    }
}

fn write(shared: &Shared, page: Option<Arc<Prepared>>, allow: Allow) {
    let mut current = match shared.lock() {
        Ok(current) => current,
        Err(poisoned) => poisoned.into_inner(),
    };
    current.page = page;
    current.allow = allow;
}

/// The browser view: [`Lifecycle`]'s decisions carried out on the native webview, where there is one.
#[derive(Default)]
pub(crate) struct Host {
    shared: Shared,
    lifecycle: Lifecycle,
    /// Whether the view now live was made with scripts on.
    made_with_scripts: bool,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    view: Option<native::View>,
}

impl Host {
    /// Whether this build has a browser view at all.
    pub const EMBEDDED: bool = cfg!(any(target_os = "macos", target_os = "windows"));

    /// Show `page` with what its document allows, from the next frame.
    pub fn set_page(&mut self, page: Option<Arc<Prepared>>, allow: Allow) {
        write(&self.shared, page, allow);
        if self.lifecycle.is_live() && allow.scripts != self.made_with_scripts {
            self.lifecycle.remake();
        } else {
            self.lifecycle.page_changed();
        }
    }

    /// Carry out this frame's decisions (see [`Lifecycle::frame`]).
    pub fn sync(&mut self, frame: &eframe::Frame, open: bool, wanted: Option<Bounds>, pressed: bool) {
        if !Self::EMBEDDED {
            return;
        }
        for op in self.lifecycle.frame(open, wanted, pressed) {
            if let Err(why) = self.run(frame, op) {
                log::warn!("the browser view failed, the system browser is used instead: {why}");
                self.drop_view();
                self.lifecycle.fail(why);
                break;
            }
        }
    }

    /// Destroy the view now (the app is closing).
    pub fn close(&mut self) {
        self.drop_view();
        self.lifecycle = Lifecycle::default();
    }

    /// Why there is no view although the build has one.
    pub fn failure(&self) -> Option<&str> {
        self.lifecycle.failure()
    }

    /// Whether the view is shown now.
    pub fn is_shown(&self) -> bool {
        self.lifecycle.is_shown()
    }

    fn drop_view(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            // (Dropping removes the native view from the window.)
            let _ = xmlcraft_engine::guard(|| drop(self.view.take()));
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn run(&mut self, frame: &eframe::Frame, op: Op) -> Result<(), String> {
        if let Op::Create(bounds) = op {
            let scripts = read(&self.shared).1.scripts;
            let shared = self.shared.clone();
            let view = xmlcraft_engine::guard(|| native::create(frame, shared, bounds, scripts)).map_err(|p| format!("it stopped: {p}"))??;
            self.view = Some(view);
            self.made_with_scripts = scripts;
            return Ok(());
        }
        if op == Op::Destroy {
            self.drop_view();
            return Ok(());
        }
        let Some(view) = &self.view else { return Ok(()) };
        xmlcraft_engine::guard(|| native::apply(view, op)).map_err(|p| format!("it stopped: {p}"))?
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn run(&mut self, _frame: &eframe::Frame, _op: Op) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native {
    use std::borrow::Cow;

    use wry::dpi::{PhysicalPosition, PhysicalSize};
    use wry::http::{Request, Response};
    use wry::{NewWindowResponse, PermissionResponse, WebViewBuilder};
    use xmlcraft_engine::preview::{DOCUMENT_URL, SCHEME, allows_navigation, serve};

    use super::{Bounds, Op, Shared, read};

    pub(super) struct View {
        webview: wry::WebView,
        /// WebView2 keeps its profile here (a folder the app can write; the default is beside the program).
        #[cfg(target_os = "windows")]
        _context: wry::WebContext,
    }

    fn rect(b: Bounds) -> wry::Rect {
        wry::Rect { position: PhysicalPosition::new(b.x, b.y).into(), size: PhysicalSize::new(b.width, b.height).into() }
    }

    /// The protocol's answer: the page at the document's path, else 404, always with the policy.
    fn respond(shared: &Shared, request: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
        let (page, allow) = read(shared);
        let path = if request.method() == wry::http::Method::GET { request.uri().path() } else { "" };
        let served = serve(path, page.as_deref(), allow);
        let mut response = Response::builder().status(served.status);
        for (name, value) in &served.headers {
            response = response.header(*name, value.as_str());
        }
        response.body(Cow::Owned(served.body)).unwrap_or_else(|_| {
            let mut refused = Response::new(Cow::Borrowed(&[][..]));
            *refused.status_mut() = wry::http::StatusCode::INTERNAL_SERVER_ERROR;
            refused
        })
    }

    pub(super) fn create(frame: &eframe::Frame, shared: Shared, bounds: Bounds, scripts: bool) -> Result<View, String> {
        #[cfg(target_os = "windows")]
        let mut context = {
            let base = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
            wry::WebContext::new(Some(base.join("XmlCraft").join("WebView2")))
        };
        #[cfg(target_os = "windows")]
        let builder = WebViewBuilder::new_with_web_context(&mut context);
        #[cfg(not(target_os = "windows"))]
        let builder = WebViewBuilder::new();
        let mut builder = builder
            .with_bounds(rect(bounds))
            .with_visible(true)
            .with_focused(false)
            .with_devtools(false)
            .with_incognito(true)
            .with_clipboard(false)
            .with_autoplay(false)
            .with_hotkeys_zoom(false)
            .with_back_forward_navigation_gestures(false)
            .with_custom_protocol(SCHEME.to_owned(), move |_, request| respond(&shared, &request))
            .with_navigation_handler(|url| allows_navigation(&url))
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .with_permission_handler(|_| PermissionResponse::Deny)
            .with_url(DOCUMENT_URL);
        if !scripts {
            builder = builder.with_javascript_disabled();
        }
        #[cfg(target_os = "macos")]
        {
            use wry::WebViewBuilderExtDarwin;
            builder = builder.with_allow_link_preview(false);
        }
        #[cfg(target_os = "windows")]
        {
            use wry::WebViewBuilderExtWindows;
            builder = builder.with_https_scheme(true).with_browser_accelerator_keys(false).with_default_context_menus(false);
        }
        let webview = builder.build_as_child(frame).map_err(|e| e.to_string())?;
        Ok(View {
            webview,
            #[cfg(target_os = "windows")]
            _context: context,
        })
    }

    pub(super) fn apply(view: &View, op: Op) -> Result<(), String> {
        let webview = &view.webview;
        let done = match op {
            Op::Move(bounds) => webview.set_bounds(rect(bounds)),
            Op::Show => webview.set_visible(true),
            Op::Hide => webview.set_visible(false),
            Op::Reload => webview.reload(),
            Op::ReturnFocus => webview.focus_parent(),
            // (Made and destroyed by the host.)
            Op::Create(_) | Op::Destroy => Ok(()),
        };
        done.map_err(|e| e.to_string())
    }
}

/// The page written for the system browser.
#[derive(Debug, Default)]
pub(crate) struct BrowserCopy {
    /// The folder the copies are written to, once made (under [`Self::base`]).
    dir: Option<PathBuf>,
    /// The copy there is now: of which document, where.
    file: Option<(u64, PathBuf)>,
    /// Where the folder is made; the temporary directory unless a test says otherwise.
    base: Option<PathBuf>,
}

/// How many fresh folder names are tried.
const FOLDER_ATTEMPTS: u32 = 8;

/// A file name for the copy of `name` (document `doc`): the stem of its last part in letters, digits, `-` and `_` (at most 40), the id,
/// `.html`.
pub(crate) fn copy_name(name: &str, doc: u64) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = last.rsplit_once('.').map_or(last, |(stem, _)| stem);
    let safe: String = stem.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).take(40).collect();
    let safe = if safe.is_empty() { "page".to_owned() } else { safe };
    format!("{safe}-{doc}.html")
}

impl BrowserCopy {
    /// Copies made under `base` instead of the temporary directory (tests).
    #[cfg(test)]
    pub fn under(base: PathBuf) -> Self {
        Self { dir: None, file: None, base: Some(base) }
    }

    /// The folder of the copies, made the first time: a fresh name (`create_dir` fails if it exists, so it is never a folder someone
    /// else made), owner-only on Unix.
    fn folder(&mut self) -> std::io::Result<PathBuf> {
        if let Some(dir) = &self.dir {
            return Ok(dir.clone());
        }
        let base = self.base.clone().unwrap_or_else(std::env::temp_dir);
        let mut last = std::io::Error::other("no folder name was free");
        for attempt in 0..FOLDER_ATTEMPTS {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
            let dir = base.join(format!("xmlcraft-preview-{}-{nanos:09}-{attempt}", std::process::id()));
            #[cfg_attr(not(unix), allow(unused_mut))] // only Unix sets the mode
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&dir) {
                Ok(()) => {
                    self.dir = Some(dir.clone());
                    return Ok(dir);
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// Write the page of document `doc` (called `name`), replacing the copy of an earlier one. Returns where it is.
    pub fn write(&mut self, doc: u64, name: &str, page: &Prepared, allow: Allow) -> Result<PathBuf, String> {
        if cfg!(target_arch = "wasm32") {
            return Err("the web build writes no files".to_owned());
        }
        let dir = self.folder().map_err(|e| format!("the temporary folder cannot be made: {e}"))?;
        let path = dir.join(copy_name(name, doc));
        xmlcraft_platform::files::write_file(&path, fallback_page(page, allow).as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some((_, old)) = self.file.take().filter(|(_, old)| *old != path) {
            let _ = std::fs::remove_file(old);
        }
        self.file = Some((doc, path.clone()));
        Ok(path)
    }

    /// Write the page again when there is a copy of document `doc` (its page changed): the browser shows it when reloaded.
    pub fn refresh(&mut self, doc: u64, name: &str, page: &Prepared, allow: Allow) {
        if self.file.as_ref().is_some_and(|(d, _)| *d == doc)
            && let Err(why) = self.write(doc, name, page, allow)
        {
            log::warn!("the copy for the system browser was not updated: {why}");
        }
    }

    /// Where the copy is now.
    pub fn path(&self) -> Option<&Path> {
        self.file.as_ref().map(|(_, p)| p.as_path())
    }

    /// Remove the copies and their folder (the app is closing).
    pub fn remove(&mut self) {
        self.file = None;
        if let Some(dir) = self.dir.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

impl Drop for BrowserCopy {
    fn drop(&mut self) {
        self.remove();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xmlcraft_engine::preview::{HTML_MEDIA_TYPE, Shown};

    const B: Bounds = Bounds { x: 10, y: 20, width: 300, height: 200 };

    #[test]
    fn the_pixels_of_a_rect_follow_the_scale_factor_and_nonsense_has_none() {
        let rect = egui::Rect::from_min_size(egui::pos2(5.0, 10.0), egui::vec2(150.0, 100.0));
        assert_eq!(Bounds::of(rect, 2.0), Some(B));
        assert_eq!(Bounds::of(rect, 1.0), Some(Bounds { x: 5, y: 10, width: 150, height: 100 }));
        assert_eq!(Bounds::of(rect, 1.25), Some(Bounds { x: 6, y: 13, width: 188, height: 125 }), "rounded at the edges");
        for ppp in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(Bounds::of(rect, ppp), None, "{ppp}");
        }
        assert_eq!(Bounds::of(egui::Rect::NOTHING, 1.0), None);
        assert_eq!(Bounds::of(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(0.2, 50.0)), 1.0), None, "less than a pixel");
        assert_eq!(Bounds::of(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1e9, 50.0)), 1.0), None);
        assert_eq!(Bounds::of(egui::Rect::EVERYTHING, 1.0), None);
    }

    #[test]
    fn the_view_is_made_when_the_panel_first_shows_a_page_and_destroyed_when_it_closes() {
        let mut life = Lifecycle::default();
        assert!(life.frame(true, None, false).is_empty(), "no page yet: nothing is made");
        assert_eq!(life.frame(true, Some(B), false), [Op::Create(B)]);
        assert!(life.is_shown());
        assert!(life.frame(true, Some(B), false).is_empty(), "nothing changed, nothing done");
        assert_eq!(life.frame(false, None, false), [Op::Destroy]);
        assert!(!life.is_live());
        assert!(life.frame(false, None, false).is_empty(), "destroyed once");
        assert_eq!(life.frame(true, Some(B), false), [Op::Create(B)], "made again when the panel opens again");
    }

    #[test]
    fn the_view_follows_the_panel_and_hides_while_covered_or_behind_another_tab() {
        let mut life = Lifecycle::default();
        life.frame(true, Some(B), false);
        let wider = Bounds { width: 500, ..B };
        assert_eq!(life.frame(true, Some(wider), false), [Op::Move(wider)]);
        assert_eq!(life.frame(true, None, false), [Op::Hide, Op::ReturnFocus], "covered: hidden, and the keyboard back to egui");
        assert!(life.is_live() && !life.is_shown(), "hidden, not destroyed");
        assert!(life.frame(true, None, false).is_empty());
        assert_eq!(life.frame(true, Some(wider), false), [Op::Show]);
        assert_eq!(life.frame(true, Some(B), false), [Op::Move(B)]);
    }

    #[test]
    fn a_new_page_reloads_a_live_view_and_scripts_remake_it() {
        let mut life = Lifecycle::default();
        life.page_changed();
        assert_eq!(life.frame(true, Some(B), false), [Op::Create(B)], "a view made later loads the page anyway");
        life.page_changed();
        assert_eq!(life.frame(true, Some(B), false), [Op::Reload]);
        life.remake();
        assert_eq!(life.frame(true, Some(B), false), [Op::Destroy, Op::Create(B)]);
        life.remake();
        assert_eq!(life.frame(true, None, false), [Op::Destroy], "remade only where it is wanted");
        assert_eq!(life.frame(true, Some(B), false), [Op::Create(B)]);
    }

    #[test]
    fn a_click_on_egui_gives_the_keyboard_back_and_a_failure_stops_until_the_panel_reopens() {
        let mut life = Lifecycle::default();
        life.frame(true, Some(B), false);
        assert_eq!(life.frame(true, Some(B), true), [Op::ReturnFocus]);
        life.fail("no handle".into());
        assert_eq!(life.failure(), Some("no handle"));
        assert!(life.frame(true, Some(B), true).is_empty(), "no view after a failure");
        assert!(life.frame(false, None, false).is_empty());
        assert_eq!(life.failure(), None, "closing the panel forgets the failure");
        assert_eq!(life.frame(true, Some(B), false), [Op::Create(B)]);
    }

    #[test]
    fn the_host_serves_the_page_it_was_given_and_remakes_for_scripts() {
        let mut host = Host::default();
        let page = Arc::new(Prepared { body: "<p>x".into(), media_type: HTML_MEDIA_TYPE, shown: Shown::Html, title: None, warnings: Vec::new() });
        host.set_page(Some(page.clone()), Allow::default());
        let (served, allow) = read(&host.shared);
        assert_eq!((served.as_deref(), allow), (Some(page.as_ref()), Allow::default()));
        // A view that was made without scripts is made again when they are allowed.
        host.lifecycle.frame(true, Some(B), false);
        host.set_page(Some(page), Allow { scripts: true, remote_images: false });
        assert_eq!(host.lifecycle.frame(true, Some(B), false), [Op::Destroy, Op::Create(B)]);
    }

    #[test]
    fn a_poisoned_lock_still_gives_the_last_page() {
        let shared: Shared = Arc::default();
        write(&shared, None, Allow { remote_images: true, scripts: false });
        let clone = shared.clone();
        let _ = std::thread::spawn(move || {
            let _held = clone.lock().expect("locks");
            panic!("poisons the lock");
        })
        .join();
        assert!(shared.is_poisoned());
        assert_eq!(read(&shared).1, Allow { remote_images: true, scripts: false });
        write(&shared, None, Allow::default());
        assert_eq!(read(&shared).1, Allow::default());
    }

    #[test]
    fn the_copy_names_are_safe_file_names() {
        assert_eq!(copy_name("index.html", 3), "index-3.html");
        assert_eq!(copy_name("../../etc/passwd", 1), "passwd-1.html");
        assert_eq!(copy_name("a\\b.c.html", 4), "bc-4.html");
        assert_eq!(copy_name("über ä.xml", 2), "ber-2.html");
        assert_eq!(copy_name("", 9), "page-9.html");
        assert_eq!(copy_name(&"x".repeat(100), 1).len(), 40 + "-1.html".len());
    }

    #[test]
    fn the_copy_is_written_owner_only_with_the_policy_replaced_and_removed() {
        let base = tempfile::tempdir().expect("dir");
        let mut copy = BrowserCopy::under(base.path().to_path_buf());
        let page =
            Prepared { body: "<!DOCTYPE html><p>one".into(), media_type: HTML_MEDIA_TYPE, shown: Shown::Html, title: None, warnings: Vec::new() };
        let path = copy.write(7, "a.html", &page, Allow::default()).expect("writes");
        let written = std::fs::read_to_string(&path).expect("reads");
        assert!(written.starts_with("<!DOCTYPE html><meta http-equiv=\"Content-Security-Policy\""), "{written}");
        let dir = path.parent().expect("in its folder").to_path_buf();
        assert!(dir.starts_with(base.path()) && dir.file_name().is_some_and(|n| n.to_string_lossy().starts_with("xmlcraft-preview-")));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&dir).expect("folder").permissions().mode() & 0o777, 0o700, "owner-only");
        }
        // The page changed: the copy of that document is written again; another document's change leaves it.
        let changed = Prepared { body: "<p>two".into(), ..page.clone() };
        copy.refresh(8, "b.html", &changed, Allow::default());
        assert!(std::fs::read_to_string(&path).expect("reads").contains("one"));
        copy.refresh(7, "a.html", &changed, Allow::default());
        assert!(std::fs::read_to_string(&path).expect("reads").contains("two"));
        // Another document's copy replaces it.
        let other = copy.write(8, "b.html", &page, Allow::default()).expect("writes");
        assert!(!path.exists() && other.exists() && copy.path() == Some(other.as_path()));
        copy.remove();
        assert!(!dir.exists(), "the folder is gone");
        assert_eq!(copy.path(), None);
    }
}
