//! The UI control channel (egui half): lets an agent see and drive the running app.
//!
//! **Opt-in only.** Nothing here runs unless the app is started with `--control <file>` (or a test attaches a [`ControlClient`]).
//! The transport (loopback only, a random token in an owner-only file, deadlines and limits) is `xmlcraft-automation`'s `control`
//! module; this module is what the requests mean.
//!
//! How it works:
//! - [`ControlPlugin`], an egui plugin, keeps a copy of the AccessKit tree from every frame's output (the widget tree with ids,
//!   roles, labels, rects and states) and injects queued input events (AccessKit click actions, pointer events, keys, text) at the
//!   start of later frames.
//! - Requests reach the app through a channel; `XmlCraftApp` answers them at the start of a frame, so they see and change exactly
//!   what the user would.
//!
//! Methods (JSON in, JSON out):
//! - `ui.state`: version, theme, tabs, panes, palette, status and the window's size in logical points.
//! - `ui.inspect {query?, role?, focused?, limit?}`: widgets in tree order with `id` (a string), `role`, `label`, `value`, `rect`
//!   (points), `enabled`, `toggled`, `selected`, `clickable`, `focusable`, `focused`, `depth`.
//! - `ui.click {id}` | `{label}` | `{x, y, button?}`: click a widget (by its AccessKit action) or a point.
//! - `ui.focus {id}` | `{label}`: focus one enabled widget through its AccessKit action.
//! - `ui.type {text}`, `ui.key {key, modifiers?}`, `ui.move {x, y}`: keyboard and pointer input.
//! - `ui.command {id}`: run a registry command (as the menu would). `ui.commands` lists them.
//! - `ui.resize {width, height}`: request a native window size (320–8192 logical points per dimension).
//! - `ui.quit {}`: normal native exit, or `quitting: false, needs_confirmation: true` when a document has unsaved changes.
//! - `ui.screenshot {region?}`: PNG of the window (base64), optionally cropped to a rect.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use egui::accesskit::{self, Action, NodeId};
use serde_json::{Value, json};

pub type Reply = Result<Value, String>;

/// Seconds to wait for frames (input to be handled, a screenshot to be taken) before reporting that the window is not being
/// drawn. While a window is hidden, minimized or fully covered, eframe keeps calling `logic` but runs no egui pass, so input and
/// screenshots wait.
const NOT_DRAWN_TIMEOUT: f64 = 5.0;

/// Wall-clock seconds (egui's own clock stops while the window is not drawn).
fn now_secs(ctx: &egui::Context) -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = ctx;
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
    }
    #[cfg(target_arch = "wasm32")]
    {
        ctx.input(|i| i.time)
    }
}

fn not_drawn(what: &str) -> String {
    format!(
        "{what} did not happen within {NOT_DRAWN_TIMEOUT:.0} s: the window is not being drawn (it may be hidden, minimized or covered); bring it to the front and retry"
    )
}

/// One request from a client, answered on `reply`.
pub struct ControlRequest {
    pub method: String,
    pub params: Value,
    pub reply: Sender<Reply>,
}

/// Sends requests to the app (from the transport thread or a test).
#[derive(Clone)]
pub struct ControlClient {
    tx: Sender<ControlRequest>,
    ctx: egui::Context,
}

impl ControlClient {
    /// Queue a request; the reply arrives on the returned receiver once the app has handled it.
    pub fn send(&self, method: &str, params: Value) -> Receiver<Reply> {
        let (reply, rx) = channel();
        let req = ControlRequest { method: method.into(), params, reply };
        if let Err(e) = self.tx.send(req) {
            let _ = e.0.reply.send(Err("the app has closed".into()));
        }
        self.ctx.request_repaint();
        rx
    }
}

/// State shared between the plugin (frame hooks) and the app.
#[derive(Default)]
struct Shared {
    nodes: HashMap<NodeId, accesskit::Node>,
    root: Option<NodeId>,
    focus: Option<NodeId>,
    /// Event batches to inject, one batch per frame.
    inject: VecDeque<Vec<egui::Event>>,
    /// Real egui passes so far (counted in `output_hook`, which `logic`-only calls never reach).
    passes: u64,
}

/// The egui plugin half of the control channel.
pub struct ControlPlugin {
    shared: Arc<Mutex<Shared>>,
}

impl egui::Plugin for ControlPlugin {
    fn debug_name(&self) -> &'static str {
        "xmlcraft-control"
    }

    fn input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        let Ok(mut s) = self.shared.lock() else { return };
        if let Some(batch) = s.inject.pop_front() {
            input.events.extend(batch);
        }
        if !s.inject.is_empty() {
            ctx.request_repaint();
        }
    }

    fn output_hook(&mut self, _ctx: &egui::Context, output: &mut egui::FullOutput) {
        let Ok(mut s) = self.shared.lock() else { return };
        s.passes += 1;
        let Some(update) = &output.platform_output.accesskit_update else { return };
        s.nodes = update.nodes.iter().cloned().collect();
        s.root = update.tree.as_ref().map(|t| t.root);
        s.focus = Some(update.focus);
    }
}

enum Pending {
    /// Answer once this many egui passes have run (the injected input has been handled by then).
    Frames {
        until_pass: u64,
        since: f64,
        reply: Sender<Reply>,
        value: Value,
    },
    Screenshot {
        tag: u64,
        region: Option<egui::Rect>,
        reply: Sender<Reply>,
        since: f64,
    },
}

/// The app half: the request queue and requests waiting for later frames.
pub struct Control {
    rx: Receiver<ControlRequest>,
    shared: Arc<Mutex<Shared>>,
    pending: Vec<Pending>,
    next_tag: u64,
}

/// Install the control channel on `ctx`: returns the app half and a client.
pub fn attach(ctx: &egui::Context) -> (Control, ControlClient) {
    let shared = Arc::new(Mutex::new(Shared::default()));
    ctx.add_plugin(ControlPlugin { shared: shared.clone() });
    ctx.enable_accesskit();
    let (tx, rx) = channel();
    (Control { rx, shared, pending: Vec::new(), next_tag: 1 }, ControlClient { tx, ctx: ctx.clone() })
}

/// What a request needs from the app (implemented by `XmlCraftApp`).
pub(crate) trait Host {
    fn state(&self) -> Value;
    fn command(&mut self, id: &str) -> Reply;
    fn commands(&self) -> Value;
    /// Quit if nothing would be lost; false when the user must confirm first.
    fn quit(&mut self) -> bool;
    /// Open the file at `path` in a new tab.
    fn open(&mut self, path: &str) -> Reply;
    /// The text of the open document at `index` (the one being shown when `None`).
    fn document_text(&self, index: Option<usize>) -> Reply;
}

enum Handled {
    Now(Reply),
    /// Reply with the value after this many frames.
    AfterFrames(u32, Value),
    Screenshot(Option<egui::Rect>),
}

impl Control {
    /// Handle new requests and finish waiting ones. Call at the start of every frame.
    pub(crate) fn tick(&mut self, ctx: &egui::Context, host: &mut impl Host) {
        self.finish_pending(ctx);
        while let Ok(req) = self.rx.try_recv() {
            match self.handle(ctx, host, &req.method, &req.params) {
                Handled::Now(r) => {
                    let _ = req.reply.send(r);
                }
                Handled::AfterFrames(frames, value) => {
                    let until_pass = self.passes() + u64::from(frames);
                    self.pending.push(Pending::Frames { until_pass, since: now_secs(ctx), reply: req.reply, value });
                }
                Handled::Screenshot(region) => {
                    let tag = self.next_tag;
                    self.next_tag += 1;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(tag)));
                    self.pending.push(Pending::Screenshot { tag, region, reply: req.reply, since: now_secs(ctx) });
                }
            }
        }
        if !self.pending.is_empty() {
            ctx.request_repaint();
        }
    }

    fn finish_pending(&mut self, ctx: &egui::Context) {
        let shots: Vec<(u64, Arc<egui::ColorImage>)> = ctx.input(|i| {
            i.raw
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Screenshot { user_data, image, .. } => {
                        user_data.data.as_ref().and_then(|d| d.downcast_ref::<u64>()).map(|tag| (*tag, image.clone()))
                    }
                    _ => None,
                })
                .collect()
        });
        let ppp = ctx.pixels_per_point();
        let now = now_secs(ctx);
        let passes = self.passes();
        let mut keep = Vec::new();
        for p in self.pending.drain(..) {
            match p {
                Pending::Frames { until_pass, reply, value, .. } if passes >= until_pass => {
                    let _ = reply.send(Ok(value));
                }
                Pending::Frames { since, reply, .. } if now - since > NOT_DRAWN_TIMEOUT => {
                    // The agent is told it failed, so it must not happen later either.
                    if let Ok(mut sh) = self.shared.lock() {
                        sh.inject.clear();
                    }
                    let _ = reply.send(Err(not_drawn("handling the input")));
                }
                p @ Pending::Frames { .. } => keep.push(p),
                Pending::Screenshot { tag, region, reply, since } => match shots.iter().find(|(t, _)| *t == tag) {
                    Some((_, image)) => {
                        let _ = reply.send(screenshot_png(image, region, ppp));
                    }
                    // The platform skips painting windows that are hidden, minimized or fully covered, so no screenshot ever
                    // arrives: say so instead of hanging.
                    None if now - since > NOT_DRAWN_TIMEOUT => {
                        let _ = reply.send(Err(not_drawn("the screenshot")));
                    }
                    None => keep.push(Pending::Screenshot { tag, region, reply, since }),
                },
            }
        }
        self.pending = keep;
    }

    fn passes(&self) -> u64 {
        self.shared.lock().map(|s| s.passes).unwrap_or(0)
    }

    /// Queue event batches, one per frame; the number of frames to wait before the last one has been handled.
    fn inject(&self, batches: Vec<Vec<egui::Event>>) -> u32 {
        let n = u32::try_from(batches.len()).unwrap_or(u32::MAX);
        if let Ok(mut s) = self.shared.lock() {
            s.inject.extend(batches);
        }
        // The last batch is handled during the frame it is injected into; answer one frame later.
        n.saturating_add(1)
    }

    fn handle(&mut self, ctx: &egui::Context, host: &mut impl Host, method: &str, p: &Value) -> Handled {
        let str_param = |k: &str| p.get(k).and_then(Value::as_str).ok_or_else(|| format!("{method}: missing string parameter {k}"));
        let r: Result<Handled, String> = (|| match method {
            "ui.state" => {
                only_params(p, method, &[])?;
                let mut state = host.state();
                let size = ctx.input(|i| i.viewport_rect().size());
                state["window"] = json!({ "width": size.x, "height": size.y, "pixels_per_point": ctx.pixels_per_point() });
                Ok(Handled::Now(Ok(state)))
            }
            "ui.commands" => Ok(Handled::Now(Ok(host.commands()))),
            "ui.open" => {
                only_params(p, method, &["path"])?;
                let opened = host.open(str_param("path")?)?;
                Ok(Handled::AfterFrames(2, opened))
            }
            "ui.document_text" => {
                only_params(p, method, &["index"])?;
                let index = p.get("index").and_then(Value::as_u64).map(|i| i as usize);
                Ok(Handled::Now(host.document_text(index)))
            }
            "ui.command" => {
                only_params(p, method, &["id"])?;
                Ok(Handled::Now(host.command(str_param("id")?)))
            }
            "ui.inspect" => {
                only_params(p, method, &["query", "role", "focused", "limit"])?;
                Ok(Handled::Now(Ok(self.inspect(p))))
            }
            "ui.click" => self.click(p),
            "ui.focus" => self.focus(p),
            "ui.resize" => {
                only_params(p, method, &["width", "height"])?;
                let dimension = |key: &str| -> Result<f32, String> {
                    p.get(key)
                        .and_then(Value::as_f64)
                        .filter(|n| n.is_finite() && (320.0..=8192.0).contains(n))
                        .map(|n| n as f32)
                        .ok_or_else(|| format!("ui.resize: {key} must be between 320 and 8192 logical points"))
                };
                let size = egui::vec2(dimension("width")?, dimension("height")?);
                if cfg!(target_arch = "wasm32") {
                    return Err("ui.resize needs a native window".into());
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                ctx.request_repaint();
                Ok(Handled::AfterFrames(2, json!({ "requested": [size.x, size.y] })))
            }
            "ui.quit" => {
                only_params(p, method, &[])?;
                if cfg!(target_arch = "wasm32") {
                    return Err("ui.quit needs a native window".into());
                }
                let quitting = host.quit();
                if quitting {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Ok(Handled::Now(Ok(json!({ "quitting": quitting, "needs_confirmation": !quitting }))))
            }
            "ui.move" => {
                only_params(p, method, &["x", "y"])?;
                let point = |key: &str| -> Result<f32, String> {
                    p.get(key)
                        .and_then(Value::as_f64)
                        .map(|n| n as f32)
                        .filter(|n| n.is_finite())
                        .ok_or_else(|| format!("ui.move: {key} must be a finite coordinate in points"))
                };
                let pos = egui::pos2(point("x")?, point("y")?);
                Ok(Handled::AfterFrames(self.inject(vec![vec![egui::Event::PointerMoved(pos)]]), json!({ "moved": [pos.x, pos.y] })))
            }
            "ui.type" => {
                only_params(p, method, &["text"])?;
                let text = str_param("text")?.to_string();
                Ok(Handled::AfterFrames(self.inject(vec![vec![egui::Event::Text(text)]]), json!({ "typed": true })))
            }
            "ui.key" => {
                only_params(p, method, &["key", "modifiers"])?;
                let name = str_param("key")?;
                let key = egui::Key::from_name(name)
                    .ok_or_else(|| format!("ui.key: unknown key {name:?} (egui key names: A, Enter, Escape, ArrowDown, F5, …)"))?;
                let modifiers = modifiers(p.get("modifiers"))?;
                let ev = |pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
                Ok(Handled::AfterFrames(self.inject(vec![vec![ev(true)], vec![ev(false)]]), json!({ "key": name })))
            }
            "ui.screenshot" => {
                only_params(p, method, &["region"])?;
                let region = match p.get("region") {
                    None | Some(Value::Null) => None,
                    Some(r) => Some(rect_param(r)?),
                };
                ctx.request_repaint();
                Ok(Handled::Screenshot(region))
            }
            other => Err(format!(
                "unknown method {other:?} (ui.state, ui.inspect, ui.click, ui.focus, ui.resize, ui.quit, ui.move, ui.type, ui.key, ui.command, ui.commands, ui.screenshot)"
            )),
        })();
        r.unwrap_or_else(|e| Handled::Now(Err(e)))
    }

    fn inspect(&self, p: &Value) -> Value {
        let query = p.get("query").and_then(Value::as_str).map(str::to_lowercase);
        let role = p.get("role").and_then(Value::as_str).map(str::to_lowercase);
        let focused = p.get("focused").and_then(Value::as_bool);
        let limit = p.get("limit").and_then(Value::as_u64).map_or(500, |n| usize::try_from(n).unwrap_or(usize::MAX));
        let Ok(s) = self.shared.lock() else { return json!({ "widgets": [] }) };
        let mut out = Vec::new();
        let mut total = 0usize;
        let mut stack: Vec<(NodeId, usize)> = s.root.map(|r| vec![(r, 0)]).unwrap_or_default();
        while let Some((id, depth)) = stack.pop() {
            let Some(node) = s.nodes.get(&id) else { continue };
            for c in node.children().iter().rev() {
                stack.push((*c, depth + 1));
            }
            let w = widget(id, node, depth, s.focus == Some(id));
            let text =
                format!("{} {} {}", w["label"].as_str().unwrap_or(""), w["value"].as_str().unwrap_or(""), w["description"].as_str().unwrap_or(""))
                    .to_lowercase();
            let role_ok = role.as_ref().is_none_or(|r| w["role"].as_str().is_some_and(|x| x.to_lowercase() == *r));
            let query_ok = query.as_ref().is_none_or(|q| text.contains(q.as_str()));
            let focus_ok = focused.is_none_or(|want| (s.focus == Some(id)) == want);
            if role_ok && query_ok && focus_ok && id != s.root.unwrap_or(id) {
                total += 1;
                if out.len() < limit {
                    out.push(w);
                }
            }
        }
        json!({ "widgets": out, "count": total, "truncated": total > out.len() })
    }

    fn click(&mut self, p: &Value) -> Result<Handled, String> {
        only_params(p, "ui.click", &["id", "label", "x", "y", "button"])?;
        if let (Some(x), Some(y)) = (p.get("x").and_then(Value::as_f64), p.get("y").and_then(Value::as_f64)) {
            let point = |key: &str, n: f64| {
                Some(n as f32).filter(|n| n.is_finite()).ok_or_else(|| format!("ui.click: {key} must be a finite coordinate in points"))
            };
            let pos = egui::pos2(point("x", x)?, point("y", y)?);
            let which = pointer_button(p, "ui.click")?;
            let button = |pressed| egui::Event::PointerButton { pos, button: which, pressed, modifiers: egui::Modifiers::NONE };
            // Keep hover/layout warmup, then make an instantaneous click even on slow frames.
            let frames = self.inject(vec![vec![egui::Event::PointerMoved(pos)], vec![button(true), button(false)]]);
            return Ok(Handled::AfterFrames(frames, json!({ "clicked": [x, y] })));
        }
        let s = self.shared.lock().map_err(|_| "control state poisoned")?;
        let id = match (p.get("id"), p.get("label").and_then(Value::as_str)) {
            (Some(id), _) => {
                let id: u64 =
                    id.as_u64().or_else(|| id.as_str().and_then(|s| s.parse().ok())).ok_or("ui.click: id must be a widget id from ui.inspect")?;
                let id = NodeId(id);
                if !s.nodes.contains_key(&id) {
                    return Err(format!("ui.click: no widget with id {} on screen (run ui.inspect again)", id.0));
                }
                id
            }
            (None, Some(label)) => {
                let want = label.to_lowercase();
                let matches: Vec<NodeId> = s
                    .nodes
                    .iter()
                    .filter(|(_, n)| n.supports_action(Action::Click) && !n.is_disabled() && n.label().is_some_and(|l| l.to_lowercase() == want))
                    .map(|(id, _)| *id)
                    .collect();
                match matches.as_slice() {
                    [one] => *one,
                    [] => return Err(format!("ui.click: no enabled clickable widget labelled {label:?} (try ui.inspect with query)")),
                    many => return Err(format!("ui.click: {} widgets are labelled {label:?}; click one by id", many.len())),
                }
            }
            (None, None) => return Err("ui.click: pass id, label, or x and y".into()),
        };
        let node = s.nodes.get(&id).ok_or("ui.click: the widget disappeared")?;
        if node.is_disabled() {
            return Err(format!("ui.click: widget {} ({}) is disabled", id.0, node.label().unwrap_or("unlabelled")));
        }
        let label = node.label().map(str::to_owned);
        let action = accesskit::ActionRequest { action: Action::Click, target_tree: accesskit::TreeId::ROOT, target_node: id, data: None };
        drop(s);
        let frames = self.inject(vec![vec![egui::Event::AccessKitActionRequest(action)]]);
        Ok(Handled::AfterFrames(frames, json!({ "clicked": id.0.to_string(), "label": label })))
    }

    fn focus(&mut self, p: &Value) -> Result<Handled, String> {
        only_params(p, "ui.focus", &["id", "label"])?;
        let s = self.shared.lock().map_err(|_| "control state poisoned")?;
        let id = match (p.get("id"), p.get("label").and_then(Value::as_str)) {
            (Some(id), _) => {
                let id: u64 =
                    id.as_u64().or_else(|| id.as_str().and_then(|s| s.parse().ok())).ok_or("ui.focus: id must be a widget id from ui.inspect")?;
                NodeId(id)
            }
            (None, Some(label)) => {
                let want = label.to_lowercase();
                let matches: Vec<NodeId> = s
                    .nodes
                    .iter()
                    .filter(|(_, n)| n.supports_action(Action::Focus) && !n.is_disabled() && n.label().is_some_and(|l| l.to_lowercase() == want))
                    .map(|(id, _)| *id)
                    .collect();
                match matches.as_slice() {
                    [one] => *one,
                    [] => return Err(format!("ui.focus: no enabled focusable widget labelled {label:?}")),
                    many => return Err(format!("ui.focus: {} widgets are labelled {label:?}; focus one by id", many.len())),
                }
            }
            (None, None) => return Err("ui.focus: pass id or label".into()),
        };
        let node = s.nodes.get(&id).ok_or_else(|| format!("ui.focus: no widget with id {} on screen", id.0))?;
        if node.is_disabled() {
            return Err(format!("ui.focus: widget {} is disabled", id.0));
        }
        let action = accesskit::ActionRequest { action: Action::Focus, target_tree: accesskit::TreeId::ROOT, target_node: id, data: None };
        drop(s);
        let frames = self.inject(vec![vec![egui::Event::AccessKitActionRequest(action)]]);
        Ok(Handled::AfterFrames(frames, json!({ "focused": id.0.to_string() })))
    }
}

fn widget(id: NodeId, n: &accesskit::Node, depth: usize, focused: bool) -> Value {
    let mut w = json!({
        // A string: 64-bit ids don't survive JSON parsers that use doubles.
        "id": id.0.to_string(),
        "role": format!("{:?}", n.role()),
        "depth": depth,
        "enabled": !n.is_disabled(),
        "clickable": n.supports_action(Action::Click),
        "focusable": n.supports_action(Action::Focus),
    });
    if let Value::Object(m) = &mut w {
        if let Some(l) = n.label() {
            m.insert("label".into(), json!(l));
        }
        if let Some(v) = n.value() {
            m.insert("value".into(), json!(v));
        }
        if let Some(d) = n.description() {
            m.insert("description".into(), json!(d));
        }
        if let Some(b) = n.bounds() {
            m.insert("rect".into(), json!([b.x0, b.y0, b.x1, b.y1]));
        }
        if let Some(t) = n.toggled() {
            m.insert("toggled".into(), json!(format!("{t:?}").to_lowercase()));
        }
        if let Some(s) = n.is_selected() {
            m.insert("selected".into(), json!(s));
        }
        if focused {
            m.insert("focused".into(), json!(true));
        }
    }
    w
}

fn only_params(p: &Value, method: &str, allowed: &[&str]) -> Result<(), String> {
    // The JSON-RPC transport represents omitted params as null.
    if p.is_null() {
        return Ok(());
    }
    let object = p.as_object().ok_or_else(|| format!("{method}: params must be an object"))?;
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        let hint = if allowed.is_empty() { "takes no parameters".to_owned() } else { format!("allowed: {}", allowed.join(", ")) };
        return Err(format!("{method}: unknown parameter {key:?} ({hint})"));
    }
    Ok(())
}

fn pointer_button(p: &Value, method: &str) -> Result<egui::PointerButton, String> {
    match p.get("button").and_then(Value::as_str).unwrap_or("primary") {
        "primary" | "left" => Ok(egui::PointerButton::Primary),
        "secondary" | "right" => Ok(egui::PointerButton::Secondary),
        "middle" => Ok(egui::PointerButton::Middle),
        other => Err(format!("{method}: unknown button {other:?} (primary, secondary, middle)")),
    }
}

fn modifiers(v: Option<&Value>) -> Result<egui::Modifiers, String> {
    let mut m = egui::Modifiers::NONE;
    for name in v.and_then(Value::as_array).into_iter().flatten() {
        match name.as_str().unwrap_or("") {
            "command" | "cmd" => m.command = true,
            "shift" => m.shift = true,
            "alt" | "option" => m.alt = true,
            "ctrl" | "control" => m.ctrl = true,
            "mac_cmd" => m.mac_cmd = true,
            other => return Err(format!("ui.key: unknown modifier {other:?} (command, shift, alt, ctrl)")),
        }
    }
    // "command" is ⌘ on macOS and Ctrl elsewhere, as egui expects.
    if m.command {
        if cfg!(target_os = "macos") {
            m.mac_cmd = true;
        } else {
            m.ctrl = true;
        }
    }
    Ok(m)
}

fn rect_param(v: &Value) -> Result<egui::Rect, String> {
    let a: Vec<f32> = v.as_array().map(|a| a.iter().filter_map(Value::as_f64).map(|f| f as f32).collect()).unwrap_or_default();
    match a.as_slice() {
        [x0, y0, x1, y1] if x1 > x0 && y1 > y0 && [x0, y0, x1, y1].iter().all(|v| v.is_finite()) => {
            Ok(egui::Rect::from_min_max(egui::pos2(*x0, *y0), egui::pos2(*x1, *y1)))
        }
        _ => Err("region must be [x0, y0, x1, y1] in points".into()),
    }
}

/// Encode (a region of) a screenshot as PNG; the reply carries base64 data and the size.
pub(crate) fn screenshot_png(image: &egui::ColorImage, region: Option<egui::Rect>, ppp: f32) -> Reply {
    let [w, h] = image.size;
    let (x0, y0, x1, y1) = match region {
        Some(r) => {
            let px = |v: f32, max: usize| ((v * ppp).round().max(0.0) as usize).min(max);
            (px(r.min.x, w), px(r.min.y, h), px(r.max.x, w), px(r.max.y, h))
        }
        None => (0, 0, w, h),
    };
    if x1 <= x0 || y1 <= y0 {
        return Err("the region is outside the window".into());
    }
    let mut rgba = Vec::with_capacity((x1 - x0).saturating_mul(y1 - y0).saturating_mul(4));
    for y in y0..y1 {
        let row = image.pixels.get(y * w + x0..y * w + x1).ok_or("the screenshot is smaller than the window")?;
        for px in row {
            rgba.extend_from_slice(&px.to_srgba_unmultiplied());
        }
    }
    let (cw, ch) = (u32::try_from(x1 - x0).map_err(|e| e.to_string())?, u32::try_from(y1 - y0).map_err(|e| e.to_string())?);
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, cw, ch);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    use base64::Engine as _;
    Ok(json!({ "png_base64": base64::engine::general_purpose::STANDARD.encode(out), "width": cw, "height": ch, "pixels_per_point": ppp }))
}

impl Host for crate::XmlCraftApp {
    fn state(&self) -> Value {
        json!({
            "app": crate::APP_NAME,
            "version": crate::VERSION,
            "theme_preference": format!("{:?}", self.theme_preference).to_lowercase(),
            "theme": self.applied_theme().map(|k| format!("{k:?}").to_lowercase()),
            "tabs": self.tabs().iter().map(|t| json!({ "name": t.name, "dirty": t.dirty })).collect::<Vec<_>>(),
            "active": self.active(),
            "documents": self.session.documents().iter().map(|d| {
                let head = d.editor().primary().head();
                let pos = d.editor().buffer().position(head);
                json!({
                    "name": d.name(),
                    "path": d.path().map(|p| p.display().to_string()),
                    "dirty": d.is_dirty(),
                    "lossy": d.is_lossy(),
                    "encoding": d.encoding().encoding.name(),
                    "bom": d.encoding().bom,
                    "bytes": d.editor().buffer().len(),
                    "lines": d.editor().buffer().line_count(),
                    "caret": { "line": pos.line + 1, "column": pos.col + 1 },
                    "selected": d.editor().has_selection(),
                })
            }).collect::<Vec<_>>(),
            "dialog": self.dialog.as_ref().map(|d| format!("{d:?}").split([' ', '{']).next().unwrap_or("").to_lowercase()),
            "mode": format!("{:?}", self.mode).to_lowercase(),
            "panes": { "outline": self.left_open, "entry_helpers": self.right.is_some(), "messages": self.bottom_open },
            "palette": { "open": self.palette_open, "query": self.palette_query },
            "status": self.status,
        })
    }

    fn command(&mut self, id: &str) -> Reply {
        let Some(spec) = xmlcraft_engine::commands::command(id) else {
            return Err(format!("unknown command {id:?} (ui.commands lists them)"));
        };
        if !self.command_enabled(spec) {
            return Err(format!("{id} is disabled right now"));
        }
        if self.execute(id) { Ok(json!({ "ran": id })) } else { Err(format!("{id} did not run: {}", self.status)) }
    }

    fn commands(&self) -> Value {
        let list: Vec<Value> = xmlcraft_engine::commands::COMMANDS
            .iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "label": c.label,
                    "menu": c.menu,
                    "shortcut": c.shortcut.map(|s| s.label(false)),
                    "enabled": self.command_enabled(c),
                    "checked": self.command_checked(c.id),
                })
            })
            .collect();
        json!({ "commands": list })
    }

    fn quit(&mut self) -> bool {
        // A document with unsaved changes needs the user's decision, so an agent can never discard work by asking to quit.
        !self.has_unsaved_changes()
    }

    fn open(&mut self, path: &str) -> Reply {
        if self.open_path(std::path::Path::new(path)) {
            Ok(json!({ "opened": path, "active": self.active() }))
        } else {
            Err(format!("could not open {path}: {}", self.status))
        }
    }

    fn document_text(&self, index: Option<usize>) -> Reply {
        let doc = match index {
            Some(i) => self.session.document(i),
            None => self.session.active_document(),
        };
        match doc {
            Some(doc) => Ok(json!({ "name": doc.name(), "text": doc.editor().text() })),
            None => Err("no such document".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn white(w: usize, h: usize) -> egui::ColorImage {
        egui::ColorImage::filled([w, h], egui::Color32::WHITE)
    }

    fn decode(reply: &Value) -> (u32, u32, Vec<u8>) {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD.decode(reply["png_base64"].as_str().expect("base64")).expect("valid base64");
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().expect("a PNG");
        let mut buf = vec![0; reader.output_buffer_size().expect("size")];
        let info = reader.next_frame(&mut buf).expect("a frame");
        (info.width, info.height, buf)
    }

    #[test]
    fn a_screenshot_is_a_png_of_the_whole_window() {
        let reply = screenshot_png(&white(40, 20), None, 2.0).expect("encodes");
        let (w, h, pixels) = decode(&reply);
        assert_eq!((w, h), (40, 20));
        assert_eq!(reply["pixels_per_point"], 2.0);
        assert!(pixels.iter().all(|b| *b == 255), "opaque white");
    }

    #[test]
    fn a_region_is_in_points_and_cropped_in_pixels() {
        let mut img = white(40, 20);
        img.pixels[5 * 40 + 10] = egui::Color32::RED;
        // Points [4, 2, 12, 8] at 2 px per point are pixels [8, 4, 24, 16].
        let reply = screenshot_png(&img, Some(egui::Rect::from_min_max(egui::pos2(4.0, 2.0), egui::pos2(12.0, 8.0))), 2.0).expect("encodes");
        let (w, h, pixels) = decode(&reply);
        assert_eq!((w, h), (16, 12));
        let at = |x: usize, y: usize| &pixels[(y * 16 + x) * 4..(y * 16 + x) * 4 + 4];
        assert_eq!(at(2, 1), [255, 0, 0, 255], "the red pixel (10, 5) is at (2, 1) in the crop");
        assert_eq!(at(0, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn a_region_outside_the_window_is_an_error_and_a_partly_outside_one_is_clipped() {
        let outside = egui::Rect::from_min_max(egui::pos2(500.0, 500.0), egui::pos2(600.0, 600.0));
        assert!(screenshot_png(&white(40, 20), Some(outside), 1.0).is_err());
        let partly = egui::Rect::from_min_max(egui::pos2(30.0, 10.0), egui::pos2(900.0, 900.0));
        let (w, h, _) = decode(&screenshot_png(&white(40, 20), Some(partly), 1.0).expect("clipped"));
        assert_eq!((w, h), (10, 10));
    }

    #[test]
    fn rectangles_must_be_four_finite_ordered_numbers() {
        assert!(rect_param(&json!([0, 0, 10, 10])).is_ok());
        for bad in [json!([0, 0, 10]), json!([10, 0, 0, 10]), json!([0, 0, 0, 10]), json!("x"), json!(null), json!([0, 0, 10, "a"])] {
            assert!(rect_param(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn modifiers_map_command_to_the_platform_key() {
        let m = modifiers(Some(&json!(["command", "shift"]))).expect("valid");
        assert!(m.command && m.shift);
        assert_eq!((m.mac_cmd, m.ctrl), (cfg!(target_os = "macos"), !cfg!(target_os = "macos")));
        assert_eq!(modifiers(None).expect("none"), egui::Modifiers::NONE);
        assert!(modifiers(Some(&json!(["hyper"]))).expect_err("unknown").contains("unknown modifier"));
    }

    #[test]
    fn parameters_are_checked_by_name() {
        assert!(only_params(&Value::Null, "m", &[]).is_ok());
        assert!(only_params(&json!({"a": 1}), "m", &["a"]).is_ok());
        assert!(only_params(&json!({"b": 1}), "m", &["a"]).expect_err("unknown").contains("allowed: a"));
        assert!(only_params(&json!({"b": 1}), "m", &[]).expect_err("unknown").contains("takes no parameters"));
        assert!(only_params(&json!([1]), "m", &["a"]).expect_err("not an object").contains("must be an object"));
    }

    #[test]
    fn pointer_buttons_have_friendly_names() {
        assert_eq!(pointer_button(&json!({}), "m"), Ok(egui::PointerButton::Primary));
        assert_eq!(pointer_button(&json!({"button": "right"}), "m"), Ok(egui::PointerButton::Secondary));
        assert_eq!(pointer_button(&json!({"button": "middle"}), "m"), Ok(egui::PointerButton::Middle));
        assert!(pointer_button(&json!({"button": "fourth"}), "m").is_err());
    }
}
