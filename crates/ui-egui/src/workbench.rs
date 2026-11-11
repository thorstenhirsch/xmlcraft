//! The Workbench: the document's editor in the middle and the tool panels around it, hosted by the craft-ui docking engine
//! ([`craft_ui::docking`]). Tool panels dock in a column on the left or the right edge (stacked one above the other there) or in tab
//! groups below the editor; they never float, and nothing goes above the editor or into it.
//!
//! Everything is keyed by [`Panel`] with one table ([`Panel::ALL`], [`Panel::title`], [`Panel::icon`], [`Panel::default_visible`]):
//! a new tool panel is a new variant and a line in each match, not a new boolean. The arrangement is a [`Workspace`], saved with the
//! preferences ([`Workbench::load`] repairs or replaces what it reads, and migrates the pane switches of 0.2.1).

use craft_ui::docking::{
    Action, Column, DockArea, DockContent, DockError, DockStyle, Group, Layout, MAX_DEPTH, Node, PanelLimits, Permissions, Side, Sides, Workspace,
};
use craft_ui::layout::{SplitAxis, SplitSize};
use egui::{Align, Color32, vec2};
use serde::{Deserialize, Serialize};

use crate::i18n::tl;
use crate::theme::{self, Tokens};
use crate::{XmlCraftApp, editor_view, icons};

/// Width of the side column in the default layout.
pub const COLUMN_WIDTH: f32 = 264.0;
/// Height of the bottom frame (Problems) in the default layout.
pub const BOTTOM_HEIGHT: f32 = 220.0;
/// The egui id of the dock area.
const AREA: &str = "xmlcraft-workbench";
/// Width of the group header at the end of a tab strip: the Close button.
const HEADER_WIDTH: f32 = 28.0;

/// What the Workbench hosts: the editor, and the tool panels around it. Stable names: they are saved in the preferences.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Panel {
    /// The document header and the editor, grid or home view; always shown, never moved.
    Editor,
    Outline,
    EntryHelpers,
    Problems,
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
    /// Every panel, in the order the Window menu and `ui.state` list them.
    pub const ALL: [Panel; 4] = [Panel::Editor, Panel::Outline, Panel::EntryHelpers, Panel::Problems];

    pub fn title(self) -> &'static str {
        match self {
            Self::Editor => tl!("Editor"),
            Self::Outline => tl!("Outline"),
            Self::EntryHelpers => tl!("Entry Helpers"),
            Self::Problems => tl!("Problems"),
        }
    }

    /// The Lucide icon of the panel (the rail and icon-only columns).
    pub fn icon(self) -> &'static str {
        match self {
            Self::Editor => "file-code",
            Self::Outline => "list-tree",
            Self::EntryHelpers => "braces",
            Self::Problems => "triangle-alert",
        }
    }

    /// Shown in a fresh layout.
    fn default_visible(self) -> bool {
        true
    }

    /// The name `ui.state` uses.
    pub fn key(self) -> &'static str {
        match self {
            Self::Editor => "editor",
            Self::Outline => "outline",
            Self::EntryHelpers => "entry_helpers",
            Self::Problems => "problems",
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

/// The arrangement of the panels, and the commands that change it.
#[derive(Clone, Debug, PartialEq)]
pub struct Workbench {
    workspace: Workspace<Panel>,
}

impl Default for Workbench {
    fn default() -> Self {
        Self { workspace: default_workspace() }
    }
}

/// The default arrangement: a column on the left with the Outline above the Entry Helpers, the editor, and Problems below it.
pub fn default_layout() -> Layout<Panel> {
    let column = Column::new(vec![Group::new(vec![Panel::Outline], 0), Group::new(vec![Panel::EntryHelpers], 0)], COLUMN_WIDTH);
    let dock = Node::Dock { side: Side::Start, columns: vec![column] };
    let editor = Node::Tabs { panels: vec![Panel::Editor], active: 0 };
    let bottom = Node::Tabs { panels: vec![Panel::Problems], active: 0 };
    let middle =
        Node::Split { axis: SplitAxis::Vertical, size: SplitSize::FixedSecond(BOTTOM_HEIGHT), first: Box::new(editor), second: Box::new(bottom) };
    Layout {
        root: Some(Node::Split {
            axis: SplitAxis::Horizontal,
            size: SplitSize::FixedFirst(COLUMN_WIDTH),
            first: Box::new(dock),
            second: Box::new(middle),
        }),
        floating: Vec::new(),
    }
}

fn default_workspace() -> Workspace<Panel> {
    let mut workspace = Workspace::new(default_layout());
    for panel in Panel::ALL {
        if !panel.default_visible()
            && let Err(e) = workspace.set_visible(panel, false, None, |p| p.permissions())
        {
            log::warn!("the default layout cannot hide {panel:?}: {e}");
        }
    }
    workspace
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
    /// The workbench saved in `prefs` (the whole preferences object): a saved workspace that reads and validates, fitted to the panels
    /// this build has; else, when there is none, the default layout with the pane switches and sizes of 0.2.1 applied; else the default.
    /// Never fails: anything wrong gives the default layout (and a line in the log).
    pub fn load(prefs: &serde_json::Value) -> Self {
        let default = default_workspace();
        if let Some(saved) = prefs.get("workspace") {
            let workspace = match Workspace::<Panel>::deserialize(saved) {
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
            let mut workspace = workspace;
            workspace.reconcile(|_| true, |p| p.permissions(), &default);
            if workspace.validate().is_err() {
                return Self::default();
            }
            return Self { workspace };
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
        if workspace.validate().is_ok() && workspace.layout.contains(&Panel::Editor) { Self { workspace } } else { Self::default() }
    }

    /// What is saved.
    pub fn workspace(&self) -> &Workspace<Panel> {
        &self.workspace
    }

    pub fn is_visible(&self, panel: Panel) -> bool {
        self.workspace.layout.contains(&panel)
    }

    /// Show a panel where it was last (or where the default layout has it), or hide it. The editor cannot be hidden.
    pub fn set_visible(&mut self, panel: Panel, visible: bool) -> Result<(), DockError> {
        if self.is_visible(panel) == visible {
            return Ok(());
        }
        self.workspace.set_visible(panel, visible, None, |p| p.permissions())
    }

    /// Apply the actions of a gesture or a command, all or none, with the panels' permissions.
    pub fn apply(&mut self, actions: Vec<Action<Panel>>) -> Result<Vec<Panel>, DockError> {
        if actions.is_empty() {
            return Ok(Vec::new());
        }
        self.workspace.apply_all(actions, |p| p.permissions())
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
    style.tab_height = 26.0;
    style.background = t.panel;
    style.tab_background = t.chrome;
    style.active_background = t.panel;
    style.text = t.text;
    style.inactive_text = t.text_faint;
    style.border = egui::Stroke::new(1.0, t.divider);
    style.accent = t.accent;
    style.font = theme::medium(12.0);
    style.gap = 0.0;
    // Nothing floats: a drop on no target does nothing, and the outer edge takes drops only below (above the editor is not a place).
    style.float_on_empty_drop = false;
    style.title_bar_docks = false;
    style.close_button = false;
    style.drop_edges = Sides { left: false, right: false, top: false, bottom: true };
    style
}

/// Draws the workbench into the rest of the window (after the bars and the rail).
pub fn show(app: &mut XmlCraftApp, ui: &mut egui::Ui) {
    let layout = app.workbench.workspace.layout.clone();
    let style = style(ui);
    let mut content = Content { app, close: Vec::new() };
    let output = DockArea::new(egui::Id::unique(AREA)).show_content(ui, &layout, &style, &mut content);
    let Content { app, close } = content;
    if let Some(e) = output.error {
        app.status = crate::i18n::fmt(tl!("The panels cannot go there: {reason}"), &[("reason", &e.to_string())]);
    }
    if let Err(e) = app.workbench.apply(output.actions) {
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
        if *panel == Panel::Editor {
            return editor_body(ui, self.app);
        }
        // The tab strip names the panel: the bodies start a little below it (they drew their own title above them before).
        ui.add_space(8.0);
        match panel {
            Panel::Outline => crate::side_panels::outline_body(ui, self.app),
            Panel::EntryHelpers => crate::entry_helpers::body(ui, self.app),
            Panel::Problems => crate::problems::body(ui, self.app),
            Panel::Editor => {}
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
        if active.permissions().closable { HEADER_WIDTH.min(strip_width) } else { 0.0 }
    }

    fn group_header(&mut self, ui: &mut egui::Ui, _panels: &[Panel], active: &Panel, _floating: bool) {
        if !active.permissions().closable {
            return;
        }
        let rect = ui.max_rect();
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::right_to_left(Align::Center)), |ui| {
            if icons::button(ui, "x", 22.0, false, tl!("Close")).clicked() {
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
        assert_eq!(bench.workspace().layout, default_layout(), "the same arrangement again");
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
            serde_json::json!({ "layout": { "root": { "Tabs": { "panels": ["Gizmo"], "active": 0 } }, "floating": [] } }),
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
}
