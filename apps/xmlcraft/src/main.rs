#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! XmlCraft desktop app: window, renderer, logging and command line. The application itself is `xmlcraft-ui-egui`.

// A release build on Windows is a GUI program: no console window opens beside it. (`xmlcraft-cli` stays a console program.)
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod logging;

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use xmlcraft_ui_egui::XmlCraftApp;

/// Wayland app id and macOS bundle identifier suffix; matches packaging/linux/io.github.thorstenhirsch.xmlcraft.desktop.
const APP_ID: &str = "io.github.thorstenhirsch.xmlcraft";
/// The name eframe stores the window state and preferences under.
const APP_NAME: &str = "XmlCraft";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// On macOS the icon sits on Apple's grid (the Dock draws it as is); elsewhere the full-bleed tile.
#[cfg(target_os = "macos")]
const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/app-icon/xmlcraft-1024.png");
#[cfg(not(target_os = "macos"))]
const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/io.github.thorstenhirsch.xmlcraft.png");

const USAGE: &str = "XmlCraft, an open-source XML editor.

Usage: xmlcraft [OPTIONS] [--] [FILE...]

Options:
  --version        Print the version and exit
  -h, --help       Print this help and exit
  --control FILE   Start the opt-in control channel: listens on loopback only and writes its port and a random token to FILE
                   (owner-only), so `xmlcraft-cli ui --control FILE ...` can see and drive this window. Off unless given.

Environment:
  XMLCRAFT_RENDERER=wgpu|gl   Choose the renderer (default: wgpu, then OpenGL if wgpu cannot start)
  RUST_LOG=...                Log filter, e.g. `debug` or `warn,xmlcraft_syntax=trace`

Files named on the command line are opened, each in its own tab. Put `--` before a file whose name starts with a hyphen.";

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Cli {
    Version,
    Help,
    Fail(String),
    Run { files: Vec<String>, control: Option<PathBuf> },
}

/// Read the command line (without the program name). Everything after `--` is a file, so a name that starts with a hyphen can be opened.
fn parse_args(args: impl IntoIterator<Item = String>) -> Cli {
    let mut files = Vec::new();
    let mut control = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--version" => return Cli::Version,
            "-h" | "--help" => return Cli::Help,
            "--control" => match args.next() {
                Some(file) => control = Some(PathBuf::from(file)),
                None => return Cli::Fail("--control needs a file name".to_owned()),
            },
            "--" => {
                files.extend(args.by_ref());
                break;
            }
            flag if flag.starts_with('-') => log::warn!("ignoring the unknown option {flag}"),
            _ => files.push(arg),
        }
    }
    Cli::Run { files, control }
}

fn main() -> eframe::Result {
    // First, so the panic hook and every start-up warning are recorded.
    logging::install();
    // Last-resort guard (AGENTS.md §5): commands, edits, opens and saves catch panics and report them; this hook logs every
    // panic, caught or not, with a backtrace when RUST_BACKTRACE is set.
    std::panic::set_hook(Box::new(|info| {
        let trace = std::backtrace::Backtrace::capture();
        let report = if trace.status() == std::backtrace::BacktraceStatus::Captured {
            format!("internal error: {info}\n{trace}")
        } else {
            format!("internal error: {info}")
        };
        if log::log_enabled!(log::Level::Error) {
            log::error!("{report}");
        } else {
            // `eprintln!` panics on a broken stderr pipe, and a panic inside the panic hook aborts.
            let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("xmlcraft: {report}\n"));
        }
    }));
    let (files, control) = match parse_args(std::env::args().skip(1)) {
        Cli::Version => return print_and_exit(&format!("xmlcraft {VERSION}")),
        Cli::Help => return print_and_exit(USAGE),
        Cli::Fail(message) => return print_and_fail(&message),
        Cli::Run { files, control } => (files, control),
    };
    let choice = renderer_choice(std::env::var("XMLCRAFT_RENDERER").ok().as_deref());
    // Set once the app is created, which is after the renderer has started.
    let started = Rc::new(Cell::new(false));
    let first = if choice == RendererChoice::Gl { eframe::Renderer::Glow } else { eframe::Renderer::Wgpu };
    let result = eframe::run_native(APP_NAME, native_options(first), app_creator(Rc::clone(&started), control.clone(), files.clone()));
    let result = match result {
        // Only a renderer that couldn't start: without a window or display at all, OpenGL can't help either, and winit's own
        // error says more.
        Err(e @ eframe::Error::Wgpu(_)) if retry_with_gl(choice, started.get()) => {
            // Old or unusual GPUs and drivers can't give wgpu a device; OpenGL usually still works there, so that's better than
            // quitting.
            log::error!("the GPU renderer (wgpu) didn't start: {e}. Starting with OpenGL instead; set XMLCRAFT_RENDERER=gl to skip wgpu.");
            eframe::run_native(APP_NAME, native_options(eframe::Renderer::Glow), app_creator(started, control.clone(), files))
        }
        other => other,
    };
    // The control file names a port nobody listens on any more. (On macOS the process usually ends before this: the app removes it
    // itself when the window closes.)
    if let Some(file) = &control {
        let _ = std::fs::remove_file(file);
    }
    result
}

fn print_and_fail(message: &str) -> eframe::Result {
    let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("xmlcraft: {message}\n"));
    std::process::exit(2)
}

fn print_and_exit(text: &str) -> eframe::Result {
    // `println!` panics on a broken stdout pipe; a script piping `--help` into `head` must not crash it.
    let _ = std::io::Write::write_fmt(&mut std::io::stdout(), format_args!("{text}\n"));
    Ok(())
}

/// Which renderer to start with, from `XMLCRAFT_RENDERER`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RendererChoice {
    /// wgpu, then OpenGL if wgpu can't start (unset, or anything unrecognised).
    Auto,
    /// wgpu only (`wgpu`): a failure is reported, not worked around.
    Wgpu,
    /// OpenGL only (`gl`, `opengl` or `glow`): for drivers where wgpu starts but misbehaves.
    Gl,
}

fn renderer_choice(value: Option<&str>) -> RendererChoice {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("wgpu") => RendererChoice::Wgpu,
        Some("gl" | "opengl" | "glow") => RendererChoice::Gl,
        _ => RendererChoice::Auto,
    }
}

/// Whether a failed run should be retried with OpenGL: only when the renderer was left to choose, and it failed before the app was
/// created (so while starting the renderer, not later).
fn retry_with_gl(choice: RendererChoice, app_started: bool) -> bool {
    choice == RendererChoice::Auto && !app_started
}

/// The window and renderer settings for one run.
fn native_options(renderer: eframe::Renderer) -> eframe::NativeOptions {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_inner_size([1440.0, 920.0])
        .with_min_inner_size([820.0, 520.0])
        .with_drag_and_drop(true)
        .with_app_id(APP_ID);
    // Dock, taskbar, Alt-Tab and launcher icon when running unbundled.
    match eframe::icon_data::from_png_bytes(APP_ICON_PNG) {
        Ok(icon) => viewport = viewport.with_icon(icon),
        Err(e) => log::warn!("app icon: {e}"),
    }
    if cfg!(target_os = "macos") {
        // The tab strip is the title bar: AppKit draws the traffic lights over it.
        viewport = viewport.with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false);
    }
    // Set explicitly, so the renderer never depends on which one eframe defaults to.
    eframe::NativeOptions { viewport, renderer, ..Default::default() }
}

fn app_creator<'a>(started: Rc<Cell<bool>>, control: Option<PathBuf>, files: Vec<String>) -> eframe::AppCreator<'a> {
    Box::new(move |cc| {
        started.set(true);
        // winit's Quit (⌘Q) ends the process without asking about unsaved changes. This menu's Quit closes the window instead, so
        // the app's close handling asks, as for the close button.
        #[cfg(target_os = "macos")]
        if let Err(e) = craft_menu::mac::install_app_menu(&cc.egui_ctx, APP_NAME) {
            log::warn!("couldn't install the app menu, so ⌘Q quits without asking to save: {e}");
        }
        let mut app = XmlCraftApp::new(cc);
        app.open_paths(files.iter().map(Path::new));
        if let Some(file) = &control {
            open_control_channel(&mut app, &cc.egui_ctx, file)?;
            app.set_control_file(file.clone());
        }
        Ok(Box::new(app))
    })
}

/// Start the opt-in control channel: listen on loopback, then write the port and token to `file` (owner-only). The token is never
/// logged.
fn open_control_channel(app: &mut XmlCraftApp, ctx: &egui::Context, file: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = app.attach_control(ctx);
    let endpoint = xmlcraft_automation::control::serve(move |method: &str, params| client.send(method, params))?;
    xmlcraft_automation::control::write_endpoint_file(file, &endpoint)?;
    log::info!("control channel on 127.0.0.1:{} (token in {})", endpoint.port, file.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Cli {
        parse_args(list.iter().map(|a| (*a).to_owned()))
    }

    #[test]
    fn the_command_line() {
        let run = |files: &[&str], control: Option<&str>| Cli::Run {
            files: files.iter().map(|f| (*f).to_owned()).collect(),
            control: control.map(PathBuf::from),
        };
        assert_eq!(args(&[]), run(&[], None));
        assert_eq!(args(&["a.xml", "b.xml"]), run(&["a.xml", "b.xml"], None));
        assert_eq!(args(&["--control", "c.json", "a.xml"]), run(&["a.xml"], Some("c.json")));
        assert_eq!(args(&["--version", "a.xml"]), Cli::Version);
        assert_eq!(args(&["a.xml", "-h"]), Cli::Help);
        assert!(matches!(args(&["--control"]), Cli::Fail(_)));
        // An unknown option is ignored; after `--` even a name that starts with a hyphen is a file, and so is `--version`.
        assert_eq!(args(&["--bogus", "a.xml"]), run(&["a.xml"], None));
        assert_eq!(args(&["--", "-odd.xml", "--version", "--"]), run(&["-odd.xml", "--version", "--"], None));
        assert_eq!(args(&["a.xml", "--", "b.xml"]), run(&["a.xml", "b.xml"], None));
    }

    #[test]
    fn the_renderer_follows_the_environment() {
        assert_eq!(renderer_choice(None), RendererChoice::Auto);
        assert_eq!(renderer_choice(Some("")), RendererChoice::Auto);
        assert_eq!(renderer_choice(Some("vulkan")), RendererChoice::Auto);
        assert_eq!(renderer_choice(Some(" WGPU ")), RendererChoice::Wgpu);
        for gl in ["gl", "OpenGL", "glow"] {
            assert_eq!(renderer_choice(Some(gl)), RendererChoice::Gl, "{gl}");
        }
    }

    #[test]
    fn opengl_is_only_a_fallback_when_nobody_chose_and_the_app_never_started() {
        assert!(retry_with_gl(RendererChoice::Auto, false));
        assert!(!retry_with_gl(RendererChoice::Auto, true), "a failure after start-up is not a renderer problem");
        assert!(!retry_with_gl(RendererChoice::Wgpu, false), "an explicit choice is respected");
        assert!(!retry_with_gl(RendererChoice::Gl, false));
    }

    #[test]
    fn the_app_icon_decodes() {
        assert!(eframe::icon_data::from_png_bytes(APP_ICON_PNG).is_ok());
    }

    #[test]
    fn the_app_id_names_the_desktop_entry() {
        assert_eq!(APP_ID, "io.github.thorstenhirsch.xmlcraft");
        assert!(!VERSION.is_empty());
    }
}
