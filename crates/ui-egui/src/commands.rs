//! What each registered command does in this frontend (`xmlcraft_engine::commands` says what it is called, where it appears,
//! which key runs it and when it is enabled).
//!
//! Menus, keyboard shortcuts, the palette and automation (the control channel) all call [`XmlCraftApp::execute`].

use egui::{Key, Modifiers};
use xmlcraft_engine::commands::{self, COMMANDS, CommandSpec, Kind, Shortcut};
use xmlcraft_engine::folding::FoldAction;

use crate::XmlCraftApp;
use crate::after_analysis::Then;
use crate::i18n::tl;
use crate::side_panels::RightPanel;
use crate::theme::ThemePreference;

/// Whether shortcuts are written the macOS way (`⇧⌘S`) rather than `Ctrl+Shift+S`. egui knows the platform from the build target,
/// and on the web from the browser's user agent.
pub(crate) fn mac_shortcuts(ctx: &egui::Context) -> bool {
    ctx.os() == egui::os::OperatingSystem::Mac
}

/// How `s` is written on this platform.
pub(crate) fn shortcut_label(ctx: &egui::Context, s: Shortcut) -> String {
    s.label(mac_shortcuts(ctx))
}

/// How a registered command's shortcut is written on this platform ("" without one).
pub(crate) fn command_shortcut_label(ctx: &egui::Context, id: &str) -> String {
    commands::command(id).and_then(|c| c.shortcut).map(|s| shortcut_label(ctx, s)).unwrap_or_default()
}

/// The egui key and modifiers for a registry shortcut; `None` for a key name egui does not know.
pub(crate) fn egui_shortcut(s: Shortcut) -> Option<egui::KeyboardShortcut> {
    let key = Key::from_name(s.key)?;
    let mut modifiers = Modifiers::NONE;
    if s.command {
        modifiers |= Modifiers::COMMAND;
    }
    if s.shift {
        modifiers |= Modifiers::SHIFT;
    }
    if s.mac_ctrl {
        modifiers |= Modifiers::CTRL;
    }
    if s.alt {
        modifiers |= Modifiers::ALT;
    }
    Some(egui::KeyboardShortcut::new(modifiers, key))
}

impl XmlCraftApp {
    /// Whether a registered command can run now. The engine judges the documents; state it cannot see is checked here.
    pub fn command_enabled(&self, spec: &CommandSpec) -> bool {
        commands::is_enabled(spec, &self.session)
    }

    /// The checked state of a toggle or choice command, `None` for a plain action.
    pub fn command_checked(&self, id: &str) -> Option<bool> {
        match id {
            "view.outline" => Some(self.left_open),
            "view.entry_helpers" => Some(self.right.is_some()),
            "view.messages" => Some(self.bottom_open),
            "view.theme.system" => Some(self.theme_preference == ThemePreference::System),
            "view.theme.light" => Some(self.theme_preference == ThemePreference::Light),
            "view.theme.dark" => Some(self.theme_preference == ThemePreference::Dark),
            _ => None,
        }
    }

    /// Run a registered command by id. Returns `false` when the id is unknown or the command is disabled right now.
    ///
    /// Last-resort guard (AGENTS.md §5): a command that panics is reported and the app, with its open documents, keeps running.
    pub fn execute(&mut self, id: &str) -> bool {
        let Some(spec) = commands::command(id) else {
            log::warn!("unknown command {id}");
            return false;
        };
        if !self.command_enabled(spec) {
            return false;
        }
        // Cleared first, so what the command says ("Saved a.xml") is what the status bar shows afterwards.
        self.status.clear();
        match xmlcraft_engine::guard(|| self.execute_unguarded(id)) {
            Ok(done) => done,
            Err(message) => {
                log::error!("command {id} failed: {message}");
                self.status = crate::i18n::fmt(
                    tl!("That didn't work: an internal error stopped it ({message}). Your documents are unchanged."),
                    &[("message", &message)],
                );
                false
            }
        }
    }

    fn execute_unguarded(&mut self, id: &str) -> bool {
        // Commands that show a panel or a dialog need the egui context; it is set when the app is created.
        let Some(ctx) = self.ctx.clone() else {
            log::warn!("command {id} ran before the first frame");
            return false;
        };
        let ctx = &ctx;
        match id {
            "file.new" => self.new_document(),
            "file.open" => return self.pick_open(ctx),
            "file.clear_recent" => {
                self.session.clear_recent();
                self.status = tl!("The list of recent files is empty").to_owned();
            }
            "file.save" => self.save_active(ctx),
            "file.save_as" => self.save_as_active(ctx),
            "file.revert" => self.revert_active(),
            "file.close" => {
                if let Some(index) = self.session.active() {
                    self.request_close(index);
                }
            }
            "file.quit" => self.request_quit(ctx),
            "edit.undo" => {
                if let Some(doc) = self.session.active_document_mut() {
                    doc.editor_mut().undo();
                }
            }
            "edit.redo" => {
                if let Some(doc) = self.session.active_document_mut() {
                    doc.editor_mut().redo();
                }
            }
            "edit.go_to_line" => {
                // Not over a question that is being asked: its answer would be lost.
                if self.dialog.is_none() {
                    self.goto_input.clear();
                    self.goto_focus = true;
                    self.dialog = Some(crate::dialog::Dialog::GoToLine);
                }
            }
            "edit.go_to_matching_tag" => self.when_analysed(Then::GoToMatchingTag),
            "edit.toggle_comment" => self.toggle_comment(),
            "edit.expand_selection" => {
                if let Some(doc) = self.session.active_document_mut()
                    && !doc.editor_mut().expand_selection()
                {
                    self.status = tl!("Nothing bigger to select here").to_owned();
                }
            }
            "edit.shrink_selection" => {
                if let Some(doc) = self.session.active_document_mut() {
                    doc.editor_mut().shrink_selection();
                }
            }
            "edit.fold" => self.when_analysed(Then::Fold(FoldAction::Fold)),
            "edit.unfold" => self.when_analysed(Then::Fold(FoldAction::Unfold)),
            "edit.fold_all" => self.when_analysed(Then::Fold(FoldAction::FoldAll)),
            "edit.unfold_all" => self.when_analysed(Then::Fold(FoldAction::UnfoldAll)),
            "edit.fold_level_1" => self.when_analysed(Then::Fold(FoldAction::Level(1))),
            "edit.fold_level_2" => self.when_analysed(Then::Fold(FoldAction::Level(2))),
            "edit.fold_level_3" => self.when_analysed(Then::Fold(FoldAction::Level(3))),
            "edit.find" => self.open_find(false),
            "edit.replace" => self.open_find(true),
            "edit.find_next" => self.find_step(xmlcraft_editor::find::Direction::NEXT),
            "edit.find_previous" => self.find_step(xmlcraft_editor::find::Direction::PREVIOUS),
            "edit.select_next_occurrence" => {
                if let Some(doc) = self.session.active_document_mut() {
                    doc.editor_mut().select_next_occurrence();
                }
            }
            "edit.select_all_occurrences" => self.select_all_occurrences(),
            "xml.check" => self.check_now(),
            "xml.validate" => self.validate_now(),
            "xml.pretty_print" => self.reformat(false),
            "xml.minify" => self.reformat(true),
            "edit.select_all" => {
                if let Some(doc) = self.session.active_document_mut() {
                    doc.editor_mut().select_all();
                }
            }
            "view.palette" => {
                self.palette_open = !self.palette_open;
                self.palette_query.clear();
            }
            "view.outline" => self.left_open = !self.left_open,
            "view.entry_helpers" => self.right = if self.right.is_some() { None } else { Some(RightPanel::EntryHelpers) },
            "view.messages" => self.bottom_open = !self.bottom_open,
            "view.theme.system" => self.theme_preference = ThemePreference::System,
            "view.theme.light" => self.theme_preference = ThemePreference::Light,
            "view.theme.dark" => self.theme_preference = ThemePreference::Dark,
            _ => {
                log::warn!("command {id} is registered but not implemented in this frontend");
                return false;
            }
        }
        true
    }

    /// Run the command whose shortcut was pressed this frame, if any. More specific shortcuts (more modifiers) win.
    pub(crate) fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let mut bound: Vec<(&CommandSpec, egui::KeyboardShortcut)> = COMMANDS.iter().filter_map(|c| Some((c, egui_shortcut(c.shortcut?)?))).collect();
        bound.sort_by_key(|(c, _)| std::cmp::Reverse(c.shortcut.map_or(0, |s| s.modifier_count())));
        // While a text field has focus, only the commands that say so keep their shortcut (Quit does; ⌘Z, ⌘A would not).
        let typing = ctx.egui_wants_keyboard_input();
        for (spec, shortcut) in bound {
            if (typing && !spec.in_text) || !ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
                continue;
            }
            self.execute(spec.id);
            return;
        }
    }

    /// The label to show for a command: its registry label with the interface language applied.
    pub fn command_label(spec: &CommandSpec) -> &'static str {
        crate::i18n::t(spec.label)
    }
}

/// The items of one menu, straight from the registry: actions, check boxes for toggles, radio buttons for choices.
pub(crate) fn registry_menu(app: &mut XmlCraftApp, ui: &mut egui::Ui, menu: &str) {
    let mut run: Option<&'static str> = None;
    for spec in commands::in_menu(menu) {
        if spec.separator_before {
            ui.separator();
        }
        let enabled = app.command_enabled(spec);
        let label = XmlCraftApp::command_label(spec);
        let clicked = ui
            .add_enabled_ui(enabled, |ui| match (spec.kind, app.command_checked(spec.id)) {
                (Kind::Toggle, Some(mut checked)) => ui.add(crate::widgets::checkbox(&mut checked, label)).clicked(),
                (Kind::Choice, Some(selected)) => ui.add(crate::widgets::radio(selected, label)).clicked(),
                _ => {
                    let shortcut = spec.shortcut.map(|s| shortcut_label(ui.ctx(), s)).unwrap_or_default();
                    ui.add(egui::Button::new(label).shortcut_text(shortcut)).clicked()
                }
            })
            .inner;
        if clicked {
            run = Some(spec.id);
        }
    }
    if let Some(id) = run {
        ui.close();
        app.execute(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_shortcut_names_a_key_egui_knows() {
        for c in COMMANDS {
            if let Some(s) = c.shortcut {
                assert!(egui_shortcut(s).is_some(), "{}: egui has no key named {}", c.id, s.key);
            }
        }
    }

    #[test]
    fn registry_shortcuts_become_egui_shortcuts() {
        let k = egui_shortcut(Shortcut::cmd("K")).expect("K is a key");
        assert_eq!(k.logical_key, Key::K);
        assert!(k.modifiers.command && !k.modifiers.shift);
        let s = egui_shortcut(Shortcut::cmd_shift("S")).expect("S is a key");
        assert!(s.modifiers.command && s.modifiers.shift);
        assert!(egui_shortcut(Shortcut::key("NoSuchKey")).is_none());
        assert_eq!(egui_shortcut(Shortcut::key("F8")).map(|s| s.logical_key), Some(Key::F8));
    }
}
