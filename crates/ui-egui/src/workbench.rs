//! The Workbench: the document's editor in the middle and the tool windows around it, hosted by the craft-ui docking engine
//! ([`craft_ui::docking`]). A *tool window* is any panel that is not the editor. Tool windows dock in a column on the left or the
//! right edge (stacked one above the other there) or in tab groups below the editor; they never float, and nothing goes above the
//! editor or into it ([`Workbench::apply`] refuses it).
//!
//! Everything is keyed by [`Panel`] with one table, [`Panel::spec`] (key, title, icon, rail zone, default place and visibility, the
//! command that toggles it): a new tool window is a new variant and a row there; the default layout, the rail and the saved names come
//! from it. The arrangement is a [`Workspace`], saved with the preferences ([`Workbench::load`] drops panels it does not know, repairs
//! or replaces what it reads, and migrates the pane switches of 0.2.1).

use craft_ui::docking::{
    Action, Column, DockArea, DockContent, DockError, DockStyle, Group, Layout, MAX_DEPTH, Node, PanelLimits, Permissions, Side, Sides, Workspace,
};
use craft_ui::layout::{SplitAxis, SplitSize};
use egui::{Align, Color32, vec2};
use serde::{Deserialize, Serialize};
use xmlcraft_engine::doc_kind::DocKind;

use crate::i18n::tl;
use crate::theme::{self, Tokens};
use crate::{XmlCraftApp, editor_view, icons};

/// Width of the side column in the default layout.
pub const COLUMN_WIDTH: f32 = 264.0;
/// Height of the bottom frame (Problems, Search) in the default layout: the Search form and a handful of hits.
pub const BOTTOM_HEIGHT: f32 = 260.0;
/// The egui id of the dock area.
const AREA: &str = "xmlcraft-workbench";
/// Height of a group's tab strip, and width of the group header at its end (the Close button).
const STRIP: f32 = 26.0;

/// What the Workbench hosts: the editor, and the tool windows around it. Saved in the preferences by [`PanelSpec::key`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Panel {
    /// The document header and the editor, grid or home view; always shown, never moved.
    Editor,
    Outline,
    EntryHelpers,
    Problems,
    /// Find in Files.
    Search,
}

/// Which part of the rail a tool window belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    /// The editor itself (not a tool window).
    Editor,
    /// About the document being shown: nothing to show on the home view.
    Document,
    /// About the files and the work around the documents: useful without a document too.
    Workspace,
}

/// Where a tool window goes in a fresh layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefaultPlace {
    Center,
    Left,
    Right,
    Bottom,
}

/// One row of the panel table: everything the Workbench, the rail, the menus and the saved layout know about a panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelSpec {
    /// The stable name: saved in the preferences, and what `ui.state` reports.
    pub key: &'static str,
    pub title: &'static str,
    /// The Lucide icon (the rail and the tab).
    pub icon: &'static str,
    pub zone: Zone,
    pub default_place: DefaultPlace,
    pub default_visible: bool,
    /// The command that toggles the panel (none for the editor).
    pub command: Option<&'static str>,
}

/// Where a panel is now, as `ui.state` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Left,
    Right,
    Top,
    Bottom,
    Center,
    Floating,
}

impl Place {
    pub fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::Center => "center",
            Self::Floating => "floating",
        }
    }
}

impl Panel {
    /// Every panel, in the order the rail, the default layout and `ui.state` list them.
    pub const ALL: [Panel; 5] = [Panel::Editor, Panel::Outline, Panel::EntryHelpers, Panel::Problems, Panel::Search];

    /// The panel table: the one place a panel is described.
    pub fn spec(self) -> PanelSpec {
        let tool = |key, title, icon, zone, default_place, default_visible, command| PanelSpec {
            key,
            title,
            icon,
            zone,
            default_place,
            default_visible,
            command: Some(command),
        };
        match self {
            Self::Editor => PanelSpec {
                key: "editor",
                title: tl!("Editor"),
                icon: "file-code",
                zone: Zone::Editor,
                default_place: DefaultPlace::Center,
                default_visible: true,
                command: None,
            },
            Self::Outline => tool("outline", tl!("Outline"), "list-tree", Zone::Document, DefaultPlace::Left, true, "view.outline"),
            Self::EntryHelpers => {
                tool("entry_helpers", tl!("Entry Helpers"), "braces", Zone::Document, DefaultPlace::Left, true, "view.entry_helpers")
            }
            Self::Problems => tool("problems", tl!("Problems"), "triangle-alert", Zone::Document, DefaultPlace::Bottom, true, "view.messages"),
            Self::Search => tool("search", tl!("Search"), "search", Zone::Workspace, DefaultPlace::Bottom, false, "view.search"),
        }
    }

    /// The tool windows (every panel but the editor), in table order.
    pub fn tools() -> impl Iterator<Item = Panel> {
        Self::ALL.into_iter().filter(|p| p.spec().zone != Zone::Editor)
    }

    pub fn title(self) -> &'static str {
        self.spec().title
    }

    pub fn icon(self) -> &'static str {
        self.spec().icon
    }

    /// The name `ui.state` uses, and the saved one.
    pub fn key(self) -> &'static str {
        self.spec().key
    }

    /// The panel saved under `name`: its key, or the variant name the M11.1 builds saved.
    pub fn from_name(name: &str) -> Option<Panel> {
        Self::ALL.into_iter().find(|p| p.key() == name || format!("{p:?}") == name)
    }

    /// Whether the panel has something to say about a document of this kind (`None`: the home view). A tool that is not relevant is
    /// still shown when asked; the rail dims it.
    pub fn relevant(self, kind: Option<DocKind>) -> bool {
        match (self.spec().zone, kind) {
            (Zone::Document, None) => false,
            _ if self == Panel::EntryHelpers => matches!(kind, Some(DocKind::Xml | DocKind::Xsd | DocKind::Dtd)),
            _ => true,
        }
    }

    /// What may happen to the panel: the editor stays where it is and takes no tabs (panels may open beside it); tool panels move and
    /// close but never float.
    pub fn permissions(self) -> Permissions {
        match self {
            Self::Editor => Permissions { accepts_splits: true, ..Permissions::PROTECTED },
            _ => Permissions { floatable: false, ..Permissions::default() },
        }
    }

    fn limits(self) -> PanelLimits {
        match self {
            Self::Editor => PanelLimits { min: vec2(320.0, 160.0), ..PanelLimits::default() },
            _ => PanelLimits { min: vec2(200.0, 60.0), ..PanelLimits::default() },
        }
    }
}

impl Serialize for Panel {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.key())
    }
}

impl<'de> Deserialize<'de> for Panel {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Panel::from_name(&name).ok_or_else(|| serde::de::Error::custom(format!("unknown panel {name:?}")))
    }
}

/// The arrangement of the panels, and the commands that change it.
#[derive(Clone, Debug, PartialEq)]
pub struct Workbench {
    workspace: Workspace<Panel>,
    /// Tool windows that have news while they are hidden (a search that finished): the rail marks them until they are shown.
    attention: Vec<Panel>,
}

impl Default for Workbench {
    fn default() -> Self {
        Self { workspace: default_workspace(), attention: Vec::new() }
    }
}

/// The default arrangement, from the panel table: a column on the left with the `Left` panels stacked one above the other, the
/// `Right` panels likewise on the right, the editor, and the `Bottom` panels as tabs of one group below it. Panels the table does not
/// show by default are in it too (where they come back); [`default_workspace`] hides them.
pub fn default_layout() -> Layout<Panel> {
    let at = |place: DefaultPlace| Panel::tools().filter(move |p| p.spec().default_place == place).collect::<Vec<_>>();
    let dock = |side: Side, panels: Vec<Panel>| {
        let groups = panels.into_iter().map(|p| Group::new(vec![p], 0)).collect();
        Node::Dock { side, columns: vec![Column::new(groups, COLUMN_WIDTH)] }
    };
    let editor = Node::Tabs { panels: vec![Panel::Editor], active: 0 };
    let bottom = at(DefaultPlace::Bottom);
    let mut root = if bottom.is_empty() {
        editor
    } else {
        let tabs = Node::Tabs { panels: bottom, active: 0 };
        Node::Split { axis: SplitAxis::Vertical, size: SplitSize::FixedSecond(BOTTOM_HEIGHT), first: Box::new(editor), second: Box::new(tabs) }
    };
    let right = at(DefaultPlace::Right);
    if !right.is_empty() {
        root = Node::Split {
            axis: SplitAxis::Horizontal,
            size: SplitSize::FixedSecond(COLUMN_WIDTH),
            first: Box::new(root),
            second: Box::new(dock(Side::End, right)),
        };
    }
    let left = at(DefaultPlace::Left);
    if !left.is_empty() {
        root = Node::Split {
            axis: SplitAxis::Horizontal,
            size: SplitSize::FixedFirst(COLUMN_WIDTH),
            first: Box::new(dock(Side::Start, left)),
            second: Box::new(root),
        };
    }
    Layout { root: Some(root), floating: Vec::new() }
}

fn default_workspace() -> Workspace<Panel> {
    let mut workspace = Workspace::new(default_layout());
    hide_the_hidden_by_default(&mut workspace);
    workspace
}

/// Hide the panels the table does not show by default (in reverse table order, so that they come back as one group).
fn hide_the_hidden_by_default(workspace: &mut Workspace<Panel>) {
    for panel in Panel::ALL.into_iter().rev() {
        if !panel.spec().default_visible
            && let Err(e) = workspace.set_visible(panel, false, None, |p| p.permissions())
        {
            log::warn!("the default layout cannot hide {panel:?}: {e}");
        }
    }
}

/// Whether a workspace keeps the Workbench's rules: nothing floats and nothing is above the editor (the engine's permissions keep
/// tabs out of the editor and the editor where it is).
fn check(workspace: &Workspace<Panel>) -> Result<(), DockError> {
    if !workspace.layout.floating.is_empty() {
        return Err(DockError::NotAllowed);
    }
    let mut found = Vec::new();
    if let Some(root) = &workspace.layout.root {
        walk(root, None, 0, &mut found);
    }
    if found.iter().any(|(_, place)| matches!(place, Place::Top | Place::Floating)) {
        return Err(DockError::NotAllowed);
    }
    Ok(())
}

/// A saved workspace with the panels this build does not know dropped (their places, and where they would come back), the rest as
/// saved. Panel names are read as text first, so one unknown name costs that panel only, not the layout.
fn read_workspace(saved: &serde_json::Value) -> Result<Workspace<Panel>, String> {
    let named = Workspace::<String>::deserialize(saved).map_err(|e| e.to_string())?;
    named.validate().map_err(|e| e.to_string())?;
    let mut named = named;
    let known = |name: &String| Panel::from_name(name).is_some();
    let permissions = |name: &String| Panel::from_name(name).map_or_else(Permissions::default, Panel::permissions);
    // Unknown panels go to `hidden` (with nothing to add from an empty default), and are dropped from there below.
    named.reconcile(known, permissions, &Workspace::new(Layout::default()));
    let mut value = serde_json::to_value(&named).map_err(|e| e.to_string())?;
    if let Some(hidden) = value.get_mut("hidden").and_then(serde_json::Value::as_array_mut) {
        // A hidden panel this build does not know, or one that would come back beside one, is forgotten.
        hidden.retain(|entry| <(Panel, craft_ui::docking::Location<Panel>)>::deserialize(entry).is_ok());
    }
    Workspace::<Panel>::deserialize(&value).map_err(|e| e.to_string())
}

/// The pane switches and sizes of 0.2.1 (`left_open`, `right`, `bottom_open`, `side_panels`), read one by one from saved preferences
/// that have no workspace yet: a value of the wrong kind says nothing.
#[derive(Debug, Default)]
struct Legacy {
    left_open: Option<bool>,
    right_open: Option<bool>,
    bottom_open: Option<bool>,
    outline_width: Option<f32>,
    messages_height: Option<f32>,
}

impl Legacy {
    fn read(prefs: &serde_json::Value) -> Self {
        let size = |key: &str| prefs.get("side_panels").and_then(|s| s.get(key)).and_then(serde_json::Value::as_f64).map(|v| v as f32);
        Self {
            left_open: prefs.get("left_open").and_then(serde_json::Value::as_bool),
            // `null` (closed) or `"EntryHelpers"`.
            right_open: prefs.get("right").map(|v| !v.is_null()),
            bottom_open: prefs.get("bottom_open").and_then(serde_json::Value::as_bool),
            outline_width: size("outline_width"),
            messages_height: size("messages_height"),
        }
    }
}

/// A saved size within reach of the panes of 0.2.1, else `None`.
fn legacy_size(value: Option<f32>, range: std::ops::RangeInclusive<f32>) -> Option<f32> {
    value.filter(|v| v.is_finite()).map(|v| v.clamp(*range.start(), *range.end()))
}

impl Workbench {
    /// The workbench saved in `prefs` (the whole preferences object): a saved workspace that reads and validates, without the panels
    /// this build does not know and fitted to the ones it has; else, when there is none, the default layout with the pane switches and
    /// sizes of 0.2.1 applied; else the default. Never fails: anything wrong gives the default layout (and a line in the log).
    pub fn load(prefs: &serde_json::Value) -> Self {
        let default = default_workspace();
        if let Some(saved) = prefs.get("workspace") {
            let mut workspace = match read_workspace(saved) {
                Ok(workspace) => workspace,
                Err(e) => {
                    log::warn!("the saved panel layout is unreadable, using the default: {e}");
                    return Self::default();
                }
            };
            if let Err(e) = workspace.validate() {
                log::warn!("the saved panel layout is damaged, using the default: {e}");
                return Self::default();
            }
            workspace.reconcile(|_| true, |p| p.permissions(), &default);
            if workspace.validate().is_err() || check(&workspace).is_err() || !workspace.layout.contains(&Panel::Editor) {
                log::warn!("the saved panel layout breaks the Workbench's rules, using the default");
                return Self::default();
            }
            return Self { workspace, attention: Vec::new() };
        }
        Self::migrated(&Legacy::read(prefs))
    }

    fn migrated(legacy: &Legacy) -> Self {
        let mut bench = Self::default();
        // The default layout's paths: the root split (dock | the rest), and in its second child the editor over the bottom frame.
        let mut sizes = Vec::new();
        if let Some(width) = legacy_size(legacy.outline_width, 240.0..=520.0) {
            sizes.push(Action::ResizeSplit { path: Vec::new(), size: SplitSize::FixedFirst(width) });
            sizes.push(Action::ResizeColumn { panel: Panel::Outline, width });
        }
        if let Some(height) = legacy_size(legacy.messages_height, 120.0..=600.0) {
            sizes.push(Action::ResizeSplit { path: vec![true], size: SplitSize::FixedSecond(height) });
        }
        if let Err(e) = bench.apply(sizes) {
            log::warn!("could not keep the pane sizes of the saved preferences: {e}");
        }
        let shown = [(Panel::Outline, legacy.left_open), (Panel::EntryHelpers, legacy.right_open), (Panel::Problems, legacy.bottom_open)];
        for (panel, visible) in shown {
            if let Some(visible) = visible
                && let Err(e) = bench.set_visible(panel, visible)
            {
                log::warn!("could not restore {panel:?}: {e}");
            }
        }
        bench
    }

    /// A workspace from the preferences ([`Workbench::load`] has read and repaired it); one that does not validate gives the default.
    pub fn from_workspace(workspace: Workspace<Panel>) -> Self {
        if workspace.validate().is_ok() && check(&workspace).is_ok() && workspace.layout.contains(&Panel::Editor) {
            Self { workspace, attention: Vec::new() }
        } else {
            Self::default()
        }
    }

    /// What is saved.
    pub fn workspace(&self) -> &Workspace<Panel> {
        &self.workspace
    }

    pub fn is_visible(&self, panel: Panel) -> bool {
        self.workspace.layout.contains(&panel)
    }

    /// Change the workspace on a copy, and keep the copy only when the change worked and keeps the Workbench's rules ([`check`]).
    fn change<T>(&mut self, change: impl FnOnce(&mut Workspace<Panel>) -> Result<T, DockError>) -> Result<T, DockError> {
        let mut proposed = self.workspace.clone();
        let out = change(&mut proposed)?;
        check(&proposed)?;
        self.workspace = proposed;
        self.attention.retain(|p| !self.workspace.layout.contains(p));
        Ok(out)
    }

    /// Show a panel where it was last (or where the default layout has it), or hide it. The editor cannot be hidden.
    pub fn set_visible(&mut self, panel: Panel, visible: bool) -> Result<(), DockError> {
        if self.is_visible(panel) == visible {
            return Ok(());
        }
        self.change(|w| w.set_visible(panel, visible, None, |p| p.permissions()))
    }

    /// Show a panel and make it the shown tab of its group.
    pub fn reveal(&mut self, panel: Panel) -> Result<(), DockError> {
        self.change(|w| w.set_visible(panel, true, None, |p| p.permissions()))
    }

    /// Apply the actions of a gesture or a command, all or none, with the panels' permissions. Refused: anything that floats a panel or
    /// puts one above the editor.
    pub fn apply(&mut self, actions: Vec<Action<Panel>>) -> Result<Vec<Panel>, DockError> {
        if actions.is_empty() {
            return Ok(Vec::new());
        }
        self.change(|w| w.apply_all(actions, |p| p.permissions()))
    }

    /// Mark a hidden tool window: it has news. A shown one needs no mark.
    pub fn set_attention(&mut self, panel: Panel) {
        if !self.is_visible(panel) && !self.attention.contains(&panel) {
            self.attention.push(panel);
        }
    }

    /// The arrangement drawn on the home view: the same, without the document's tool windows (there is no document).
    fn home_layout(&self) -> Layout<Panel> {
        let mut workspace = self.workspace.clone();
        for panel in Panel::tools().filter(|p| p.spec().zone == Zone::Document) {
            if workspace.set_visible(panel, false, None, |p| p.permissions()).is_err() {
                return Layout { root: Some(Node::Tabs { panels: vec![Panel::Editor], active: 0 }), floating: Vec::new() };
            }
        }
        workspace.layout
    }

    /// Where each shown panel is, in [`Panel::ALL`] order.
    pub fn places(&self) -> Vec<(Panel, Place)> {
        let mut found = Vec::new();
        if let Some(root) = &self.workspace.layout.root {
            walk(root, None, 0, &mut found);
        }
        for window in &self.workspace.layout.floating {
            found.extend(window.panels().map(|p| (*p, Place::Floating)));
        }
        Panel::ALL.iter().filter_map(|panel| found.iter().find(|(p, _)| p == panel).copied()).collect()
    }

    pub fn place(&self, panel: Panel) -> Option<Place> {
        self.places().into_iter().find(|(p, _)| *p == panel).map(|(_, place)| place)
    }
}

/// Whether the editor is in `node`.
fn holds_editor(node: &Node<Panel>, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    match node {
        Node::Tabs { panels, .. } => panels.contains(&Panel::Editor),
        Node::Split { first, second, .. } => holds_editor(first, depth + 1) || holds_editor(second, depth + 1),
        _ => false,
    }
}

/// Collect where the panels of `node` are: a dock's by its side, a frame's by where it is from the editor (`around`, the side of the
/// nearest split that has the editor on its other side).
fn walk(node: &Node<Panel>, around: Option<Place>, depth: usize, out: &mut Vec<(Panel, Place)>) {
    if depth > MAX_DEPTH {
        return;
    }
    let frame_place = |panel: &Panel| if *panel == Panel::Editor { Place::Center } else { around.unwrap_or(Place::Center) };
    match node {
        Node::Dock { side, columns } => {
            let place = if *side == Side::Start { Place::Left } else { Place::Right };
            for column in columns {
                for group in &column.groups {
                    out.extend(group.panels.iter().map(|p| (*p, place)));
                }
            }
        }
        Node::Tabs { panels, .. } => out.extend(panels.iter().map(|p| (*p, frame_place(p)))),
        Node::Stack { entries } => out.extend(entries.iter().map(|e| (e.panel, frame_place(&e.panel)))),
        Node::Split { axis, first, second, .. } => {
            let (before, after) = match axis {
                SplitAxis::Horizontal => (Place::Left, Place::Right),
                SplitAxis::Vertical => (Place::Top, Place::Bottom),
            };
            match (holds_editor(first, depth + 1), holds_editor(second, depth + 1)) {
                (true, false) => {
                    walk(first, around, depth + 1, out);
                    walk(second, Some(after), depth + 1, out);
                }
                (false, true) => {
                    walk(first, Some(before), depth + 1, out);
                    walk(second, around, depth + 1, out);
                }
                _ => {
                    walk(first, around, depth + 1, out);
                    walk(second, around, depth + 1, out);
                }
            }
        }
        _ => {}
    }
}

/// The docking chrome in XmlCraft's colours: compact strips, no floating, and drops only at the sides and below the editor.
fn style(ui: &egui::Ui) -> DockStyle {
    let t = Tokens::get(ui.ctx());
    let mut style = DockStyle::from_ui(ui);
    style.tab_height = STRIP;
    style.background = t.panel;
    style.tab_background = t.chrome;
    style.active_background = t.panel;
    style.text = t.text;
    style.inactive_text = t.text_faint;
    style.border = egui::Stroke::new(1.0, t.divider);
    style.accent = t.accent;
    style.font = theme::semibold(12.0);
    style.min_tab_width = style.font.size * 2.5 + 24.0;
    style.gap = 0.0;
    // The bar above a dock column (it drags the column and collapses it to icons) is hidden: a column moves by its groups' tabs, and
    // the rail shows and hides the panels.
    style.column_bar_height = 0.0;
    // Nothing floats: a drop on no target does nothing, and the outer edge takes drops only below (above the editor is not a place).
    style.float_on_empty_drop = false;
    style.title_bar_docks = false;
    style.close_button = false;
    style.drop_edges = Sides { left: false, right: false, top: false, bottom: true };
    labels(&mut style);
    style
}

/// The docking engine's words, in the interface language.
fn labels(style: &mut DockStyle) {
    let words: [(&mut String, &str); 24] = [
        (&mut style.float_label, tl!("Float Panel")),
        (&mut style.close_label, tl!("Close Panel")),
        (&mut style.move_panel_to_label, tl!("Move Panel to Group")),
        (&mut style.panels_label, tl!("Panels")),
        (&mut style.resize_label, tl!("Resize panels")),
        (&mut style.resize_window_label, tl!("Resize panel window")),
        (&mut style.move_group_label, tl!("Move tab group")),
        (&mut style.close_group_label, tl!("Close Tab Group")),
        (&mut style.minimize_label, tl!("Minimize")),
        (&mut style.expand_label, tl!("Expand")),
        (&mut style.float_group_label, tl!("Float Tab Group")),
        (&mut style.move_group_to_label, tl!("Move Group To")),
        (&mut style.auto_collapse_label, tl!("Auto-Collapse Iconic Panels")),
        (&mut style.move_column_label, tl!("Move panel column")),
        (&mut style.collapse_to_icons_label, tl!("Collapse to Icons")),
        (&mut style.expand_panels_label, tl!("Expand Panels")),
        (&mut style.collapse_column_label, tl!("Collapse column to icons")),
        (&mut style.expand_column_label, tl!("Expand column")),
        (&mut style.move_window_label, tl!("Move panel window")),
        (&mut style.close_window_label, tl!("Close panel window")),
        (&mut style.close_others_label, tl!("Close Other Panels in Group")),
        (&mut style.maximize_label, tl!("Maximize Frame")),
        (&mut style.restore_frame_label, tl!("Restore Frame Size")),
        (&mut style.float_hint_label, tl!("Hold {key} to undock")),
    ];
    for (field, word) in words {
        *field = word.to_owned();
    }
}

/// Draws the workbench into the rest of the window (after the bars and the rail). On the home view the document's tool windows are
/// left out and the arrangement cannot be changed (it is the documents' arrangement); the others can still be closed.
pub fn show(app: &mut XmlCraftApp, ui: &mut egui::Ui) {
    let home = app.session.active().is_none();
    let layout = if home { app.workbench.home_layout() } else { app.workbench.workspace.layout.clone() };
    let style = style(ui);
    let mut content = Content { app, close: Vec::new() };
    let output = DockArea::new(egui::Id::unique(AREA)).show_content(ui, &layout, &style, &mut content);
    let Content { app, close } = content;
    if let Some(e) = output.error {
        app.status = crate::i18n::fmt(tl!("The panels cannot go there: {reason}"), &[("reason", &e.to_string())]);
    }
    if !home && let Err(e) = app.workbench.apply(output.actions) {
        app.status = crate::i18n::fmt(tl!("The panels cannot go there: {reason}"), &[("reason", &e.to_string())]);
    }
    for panel in close {
        if let Err(e) = app.workbench.set_visible(panel, false) {
            app.status = crate::i18n::fmt(tl!("{panel} cannot close: {reason}"), &[("panel", panel.title()), ("reason", &e.to_string())]);
        }
    }
}

struct Content<'a> {
    app: &'a mut XmlCraftApp,
    /// Panels whose Close button was clicked this frame.
    close: Vec<Panel>,
}

impl DockContent<Panel> for Content<'_> {
    fn body(&mut self, ui: &mut egui::Ui, panel: &Panel) {
        match panel {
            Panel::Editor => editor_body(ui, self.app),
            // A tool window draws its own frame (`kit::tool_body`).
            Panel::Search => crate::tools::search::body(ui, self.app),
            // The tab strip names the panel: the bodies start a little below it (they drew their own title above them before).
            Panel::Outline => {
                ui.add_space(8.0);
                crate::side_panels::outline_body(ui, self.app);
            }
            Panel::EntryHelpers => {
                ui.add_space(8.0);
                crate::entry_helpers::body(ui, self.app);
            }
            Panel::Problems => {
                ui.add_space(8.0);
                crate::problems::body(ui, self.app);
            }
        }
    }

    fn logical_label(&self, panel: &Panel) -> String {
        panel.title().to_owned()
    }

    fn permissions(&self, panel: &Panel) -> Permissions {
        panel.permissions()
    }

    fn limits(&self, panel: &Panel) -> PanelLimits {
        panel.limits()
    }

    fn icon(&self, panel: &Panel) -> Option<egui::ImageSource<'static>> {
        Some(icons::source(panel.icon()))
    }

    fn group_header_width(&self, _panels: &[Panel], active: &Panel, _floating: bool, strip_width: f32) -> f32 {
        if active.permissions().closable { STRIP.min(strip_width) } else { 0.0 }
    }

    fn group_header(&mut self, ui: &mut egui::Ui, _panels: &[Panel], active: &Panel, _floating: bool) {
        if !active.permissions().closable {
            return;
        }
        let rect = ui.max_rect();
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::right_to_left(Align::Center)), |ui| {
            let label = crate::i18n::fmt(tl!("Close {title}"), &[("title", active.title())]);
            if icons::button(ui, "x", 22.0, false, &label).clicked() {
                self.close.push(*active);
            }
        });
    }
}

/// The editor panel: the document header above the editor, grid or home view.
fn editor_body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    crate::header::show(app, ui);
    let t = Tokens::get(ui.ctx());
    let fill: Color32 = if app.session.has_document() { editor_view::editor_colors(&t).background } else { t.pasteboard };
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(fill)).show(ui, |ui| app.central(ui));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places(bench: &Workbench) -> Vec<(Panel, &'static str)> {
        bench.places().into_iter().map(|(p, place)| (p, place.name())).collect()
    }

    #[test]
    fn the_default_layout_stacks_the_outline_over_the_entry_helpers_on_the_left_and_problems_below() {
        let bench = Workbench::default();
        assert!(bench.workspace().validate().is_ok());
        assert_eq!(places(&bench), [(Panel::Editor, "center"), (Panel::Outline, "left"), (Panel::EntryHelpers, "left"), (Panel::Problems, "bottom")]);
        let Some(Node::Split { first, .. }) = &bench.workspace().layout.root else { panic!("a split") };
        let Node::Dock { columns, .. } = first.as_ref() else { panic!("a dock first") };
        assert_eq!(columns.len(), 1, "one column");
        assert_eq!(columns[0].groups.len(), 2, "two stacked groups");
        assert_eq!(columns[0].width, COLUMN_WIDTH);
    }

    #[test]
    fn panels_hide_and_come_back_where_they_were() {
        let mut bench = Workbench::default();
        for panel in [Panel::Outline, Panel::EntryHelpers, Panel::Problems] {
            bench.set_visible(panel, false).expect("hides");
            assert!(!bench.is_visible(panel));
        }
        assert_eq!(places(&bench), [(Panel::Editor, "center")]);
        for panel in [Panel::Outline, Panel::EntryHelpers, Panel::Problems] {
            bench.set_visible(panel, true).expect("shows");
        }
        assert_eq!(bench.workspace().layout, Workbench::default().workspace().layout, "the same arrangement again");
        // In the other order too.
        for panel in [Panel::Problems, Panel::EntryHelpers, Panel::Outline] {
            bench.set_visible(panel, false).expect("hides");
        }
        for panel in [Panel::Problems, Panel::EntryHelpers, Panel::Outline] {
            bench.set_visible(panel, true).expect("shows");
        }
        assert_eq!(places(&bench), places(&Workbench::default()));
    }

    #[test]
    fn the_editor_cannot_be_hidden_moved_or_tabbed_into_and_nothing_floats() {
        let mut bench = Workbench::default();
        assert!(bench.set_visible(Panel::Editor, false).is_err());
        assert!(bench.is_visible(Panel::Editor));
        let before = bench.clone();
        assert!(bench.apply(vec![Action::Float { panel: Panel::Outline, rect: [10.0, 10.0, 200.0, 200.0] }]).is_err(), "no floating");
        assert!(
            bench
                .apply(vec![Action::Move {
                    panel: Panel::Outline,
                    anchor: Panel::Editor,
                    placement: craft_ui::docking::Placement::Tab { before: None },
                }])
                .is_err(),
            "no tabs in the editor"
        );
        assert!(
            bench.apply(vec![Action::MovePanel { panel: Panel::Editor, to: craft_ui::docking::Target::NewDock { side: Side::End } }]).is_err(),
            "the editor stays"
        );
        assert_eq!(bench, before, "a refused action changes nothing");
    }

    #[test]
    fn a_panel_moved_to_the_end_dock_is_on_the_right() {
        let mut bench = Workbench::default();
        bench
            .apply(vec![Action::MovePanel { panel: Panel::EntryHelpers, to: craft_ui::docking::Target::NewDock { side: Side::End } }])
            .expect("moves");
        assert_eq!(bench.place(Panel::EntryHelpers), Some(Place::Right));
        assert_eq!(bench.place(Panel::Outline), Some(Place::Left));
        // Hidden and shown again, it goes back to the right.
        bench.set_visible(Panel::EntryHelpers, false).expect("hides");
        bench.set_visible(Panel::EntryHelpers, true).expect("shows");
        assert_eq!(bench.place(Panel::EntryHelpers), Some(Place::Right));
    }

    #[test]
    fn a_saved_workspace_round_trips() {
        let mut bench = Workbench::default();
        bench
            .apply(vec![Action::MovePanel { panel: Panel::EntryHelpers, to: craft_ui::docking::Target::NewDock { side: Side::End } }])
            .expect("moves");
        bench.set_visible(Panel::Problems, false).expect("hides");
        let json = serde_json::json!({ "workspace": serde_json::to_value(bench.workspace()).expect("serializes") });
        let again = Workbench::load(&json);
        assert_eq!(again, bench);
        assert_eq!(again.place(Panel::EntryHelpers), Some(Place::Right));
        assert!(!again.is_visible(Panel::Problems));
    }

    #[test]
    fn the_pane_switches_of_0_2_1_are_migrated() {
        // Preferences as 0.2.1 wrote them.
        let saved = r#"{"theme":"Dark","left_open":false,"right":"EntryHelpers","bottom_open":false,
            "side_panels":{"outline_width":300.0,"helpers_width":330.0,"messages_height":180.0},
            "recent":[],"entry_mandatory":true,"validate_typing":true,"work_offline":false,"editor_zoom":1.0,"user_catalogs":[]}"#;
        let bench = Workbench::load(&serde_json::from_str(saved).expect("json"));
        assert!(!bench.is_visible(Panel::Outline));
        assert!(bench.is_visible(Panel::EntryHelpers));
        assert!(!bench.is_visible(Panel::Problems));
        assert_eq!(bench.place(Panel::EntryHelpers), Some(Place::Left));
        let mut bench = bench;
        bench.set_visible(Panel::Problems, true).expect("shows");
        bench.set_visible(Panel::Outline, true).expect("shows");
        let Some(Node::Split { size, first, second, .. }) = &bench.workspace().layout.root else { panic!("a split") };
        assert_eq!(*size, SplitSize::FixedFirst(300.0), "the Outline's width");
        let Node::Dock { columns, .. } = first.as_ref() else { panic!("a dock") };
        assert_eq!(columns[0].width, 300.0);
        assert_eq!(columns[0].groups.iter().map(|g| g.panels.clone()).collect::<Vec<_>>(), [vec![Panel::Outline], vec![Panel::EntryHelpers]]);
        let Node::Split { size, .. } = second.as_ref() else { panic!("the bottom split") };
        assert_eq!(*size, SplitSize::FixedSecond(180.0), "the Problems height");
        // The Entry Helpers closed, and sizes out of range.
        let bench = Workbench::load(&serde_json::json!({ "right": null, "side_panels": { "outline_width": 1e30, "messages_height": "x" } }));
        assert!(!bench.is_visible(Panel::EntryHelpers) && bench.is_visible(Panel::Outline) && bench.is_visible(Panel::Problems));
        // Nothing said: the default.
        assert_eq!(Workbench::load(&serde_json::json!({ "theme": "Light" })), Workbench::default());
        assert_eq!(Workbench::load(&serde_json::Value::Null), Workbench::default());
    }

    #[test]
    fn a_corrupt_layout_gives_the_default() {
        let bad = [
            serde_json::json!("a layout"),
            serde_json::json!({ "layout": { "root": { "Tabs": { "panels": ["Outline", "Outline"], "active": 0 } }, "floating": [] } }),
            serde_json::json!({ "layout": { "root": { "Tabs": { "panels": ["Outline"], "active": 7 } }, "floating": [] } }),
            serde_json::json!({ "version": 99, "layout": { "root": null, "floating": [] } }),
            serde_json::json!({ "layout": { "root": { "Split": { "axis": "Vertical", "size": { "Ratio": 7.0 },
                "first": { "Tabs": { "panels": ["Editor"], "active": 0 } }, "second": { "Tabs": { "panels": ["Problems"], "active": 0 } } } },
                "floating": [] } }),
        ];
        for workspace in bad {
            let bench = Workbench::load(&serde_json::json!({ "workspace": workspace }));
            assert_eq!(bench, Workbench::default(), "{workspace}");
        }
        // A layout that reads but lacks panels gets them back, and the editor is always there.
        let partial =
            serde_json::json!({ "workspace": { "layout": { "root": { "Tabs": { "panels": ["Outline"], "active": 0 } }, "floating": [] } } });
        let bench = Workbench::load(&partial);
        assert!(bench.is_visible(Panel::Editor), "the editor is always shown");
        assert!(bench.workspace().validate().is_ok());
    }

    #[test]
    fn a_deep_hostile_tree_does_not_overflow_the_stack() {
        // Deeper than the engine allows (the preferences file is read with serde_json's own nesting limit, 128).
        let mut node = serde_json::json!({ "Tabs": { "panels": ["Editor"], "active": 0 } });
        for _ in 0..100 {
            node = serde_json::json!({ "Split": { "axis": "Vertical", "size": { "Ratio": 0.5 }, "first": node, "second": { "Tabs": { "panels": [], "active": 0 } } } });
        }
        let bench = Workbench::load(&serde_json::json!({ "workspace": { "layout": { "root": node, "floating": [] } } }));
        assert_eq!(bench, Workbench::default());
    }

    #[test]
    fn the_panel_table_names_each_panel_once_and_every_tool_has_a_command() {
        let keys: std::collections::HashSet<_> = Panel::ALL.iter().map(|p| p.key()).collect();
        assert_eq!(keys.len(), Panel::ALL.len(), "keys are unique");
        for panel in Panel::ALL {
            let spec = panel.spec();
            assert_eq!(Panel::from_name(spec.key), Some(panel), "{panel:?} reads back by its key");
            assert_eq!(Panel::from_name(&format!("{panel:?}")), Some(panel), "and by the name the M11.1 builds saved");
            assert_eq!(serde_json::to_value(panel).ok(), Some(serde_json::json!(spec.key)), "it is saved by its key");
            assert_eq!(spec.command.is_none(), panel == Panel::Editor, "{panel:?}");
            assert!(crate::icons::exists(spec.icon), "{panel:?}");
            if let Some(command) = spec.command {
                assert!(xmlcraft_engine::commands::command(command).is_some(), "{command} is registered");
            }
        }
        assert_eq!(Panel::tools().collect::<Vec<_>>(), [Panel::Outline, Panel::EntryHelpers, Panel::Problems, Panel::Search]);
        assert_eq!(Panel::Search.spec().zone, Zone::Workspace);
        assert!(Panel::from_name("Gizmo").is_none() && Panel::from_name("").is_none());
    }

    #[test]
    fn what_a_tool_is_relevant_for() {
        assert!(Panel::EntryHelpers.relevant(Some(DocKind::Xsd)) && !Panel::EntryHelpers.relevant(Some(DocKind::Json)));
        assert!(!Panel::Outline.relevant(None), "no document, no outline");
        assert!(Panel::Outline.relevant(Some(DocKind::Yaml)) && Panel::Search.relevant(None));
    }

    #[test]
    fn search_is_hidden_by_default_and_comes_up_as_a_tab_next_to_problems() {
        let mut bench = Workbench::default();
        assert!(!bench.is_visible(Panel::Search));
        bench.reveal(Panel::Search).expect("shows");
        assert_eq!(bench.place(Panel::Search), Some(Place::Bottom));
        let Some(Node::Split { second, .. }) = &bench.workspace().layout.root else { panic!("a split") };
        let Node::Split { second: bottom, .. } = second.as_ref() else { panic!("the editor over the bottom frame") };
        assert_eq!(**bottom, Node::Tabs { panels: vec![Panel::Problems, Panel::Search], active: 1 }, "one group, Search shown");
        // Revealed again from behind Problems, it is the shown tab again.
        bench.apply(vec![Action::Activate { panel: Panel::Problems }]).expect("activates");
        bench.reveal(Panel::Search).expect("shows");
        let Some(Node::Split { second, .. }) = &bench.workspace().layout.root else { panic!("a split") };
        let Node::Split { second: bottom, .. } = second.as_ref() else { panic!("the bottom frame") };
        assert!(matches!(bottom.as_ref(), Node::Tabs { active: 1, .. }));
    }

    #[test]
    fn nothing_goes_above_the_editor() {
        use craft_ui::docking::{Placement, Target, Zone as DropZone};
        let mut bench = Workbench::default();
        let before = bench.clone();
        let above = [
            Action::Move { panel: Panel::Problems, anchor: Panel::Editor, placement: Placement::Split(DropZone::Top) },
            Action::MovePanel { panel: Panel::Outline, to: Target::At { anchor: Panel::Editor, placement: Placement::Split(DropZone::Top) } },
            Action::MoveGroup { panel: Panel::Problems, to: Target::At { anchor: Panel::Editor, placement: Placement::Split(DropZone::Top) } },
            Action::MovePanel { panel: Panel::Problems, to: Target::Float { rect: None } },
        ];
        for action in above {
            assert!(bench.apply(vec![action.clone()]).is_err(), "{action:?}");
            assert_eq!(bench, before, "a refused action changes nothing: {action:?}");
        }
        // Below and beside it is fine.
        bench
            .apply(vec![Action::Move { panel: Panel::Outline, anchor: Panel::Editor, placement: Placement::Split(DropZone::Bottom) }])
            .expect("below");
        assert_eq!(bench.place(Panel::Outline), Some(Place::Bottom));
        // A saved layout with a panel above the editor is not taken.
        let mut top = Workbench::default().workspace().clone();
        top.layout.root = Some(Node::Split {
            axis: SplitAxis::Vertical,
            size: SplitSize::FixedFirst(100.0),
            first: Box::new(Node::Tabs { panels: vec![Panel::Problems], active: 0 }),
            second: Box::new(Node::Tabs { panels: vec![Panel::Editor], active: 0 }),
        });
        let json = serde_json::json!({ "workspace": serde_json::to_value(&top).expect("serializes") });
        assert_eq!(Workbench::load(&json), Workbench::default());
    }

    #[test]
    fn unknown_panels_in_a_saved_layout_are_dropped_and_the_rest_is_kept() {
        let mut bench = Workbench::default();
        bench
            .apply(vec![Action::MovePanel { panel: Panel::EntryHelpers, to: craft_ui::docking::Target::NewDock { side: Side::End } }])
            .expect("moves");
        let mut saved = serde_json::to_value(bench.workspace()).expect("serializes");
        // A later build's panels: a tab next to Problems, a group in the left column, and one hidden.
        // (serde_json writes the keys of an object in order.)
        let outline = r#"{"active":0,"height":null,"minimized":false,"panels":["outline"]}"#;
        let text = saved.to_string().replacen(r#"["problems"]"#, r#"["problems","gizmo"]"#, 1).replacen(
            outline,
            &format!(r#"{outline},{{"active":0,"panels":["widget"]}}"#),
            1,
        );
        saved = serde_json::from_str(&text).expect("still json");
        if let Some(hidden) = saved.get_mut("hidden").and_then(serde_json::Value::as_array_mut) {
            hidden.push(serde_json::json!(["sprocket", { "floating": null, "anchor": "gizmo", "placement": { "Tab": { "before": null } }, "size": null, "stack": null }]));
        }
        assert!(saved.to_string().contains("gizmo") && saved.to_string().contains("widget"), "the test edits the saved layout: {saved}");
        let again = Workbench::load(&serde_json::json!({ "workspace": saved }));
        assert_eq!(places(&again), places(&bench), "everything else as saved");
        assert_eq!(again.place(Panel::EntryHelpers), Some(Place::Right));
        assert!(again.workspace().validate().is_ok());
        assert!(!serde_json::to_string(again.workspace()).expect("serializes").contains("gizmo"));
        // Search, which the saved layout hid, still comes back next to Problems.
        let mut again = again;
        again.reveal(Panel::Search).expect("shows");
        assert_eq!(again.place(Panel::Search), Some(Place::Bottom));
        // A layout of nothing but unknown panels keeps the editor.
        let only = serde_json::json!({ "workspace": { "layout": { "root": { "Tabs": { "panels": ["Gizmo"], "active": 0 } }, "floating": [] } } });
        let bench = Workbench::load(&only);
        assert!(bench.is_visible(Panel::Editor) && bench.workspace().validate().is_ok());
    }
}
