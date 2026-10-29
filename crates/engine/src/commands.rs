//! The command registry: every user-facing action, by id ("everything is a command").
//!
//! Menus, keyboard shortcuts, the ⌘K palette and automation (the CLI `run`, the control channel and MCP) all go through this
//! table. A frontend implements *what* each id does. The registry says what it is called, where it appears, which key runs it,
//! and when it is enabled, so that every surface agrees.
//!
//! A command is listed here only when it works: the table is never a wish list (`parity/xmlspy-features.toml` is).

use crate::Session;

/// When a command can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Needs {
    /// Always.
    Nothing,
    /// A document is open.
    Document,
    /// There is something to undo.
    Undo,
    /// There is something to redo.
    Redo,
}

/// How a command appears in menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Does something once.
    Action,
    /// Switches something on or off; the menu shows a check box.
    Toggle,
    /// One of several exclusive choices; the menu shows a radio button.
    Choice,
}

/// A keyboard shortcut. `command` is ⌘ on macOS and Ctrl elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub command: bool,
    pub shift: bool,
    /// ⌃ on macOS (in addition to ⌘); Ctrl elsewhere.
    pub mac_ctrl: bool,
    /// ⌥ on macOS, Alt elsewhere.
    pub alt: bool,
    /// Key name: a letter, a digit, punctuation such as `,`, or a name such as `F8` or `Delete`.
    pub key: &'static str,
}

impl Shortcut {
    /// ⌘ (Ctrl elsewhere) and `key`.
    pub const fn cmd(key: &'static str) -> Self {
        Self { command: true, shift: false, mac_ctrl: false, alt: false, key }
    }

    /// ⌥⌘ (Ctrl+Alt elsewhere) and `key`.
    pub const fn cmd_alt(key: &'static str) -> Self {
        Self { command: true, shift: false, mac_ctrl: false, alt: true, key }
    }

    /// ⇧⌘ (Ctrl+Shift elsewhere) and `key`.
    pub const fn cmd_shift(key: &'static str) -> Self {
        Self { command: true, shift: true, mac_ctrl: false, alt: false, key }
    }

    /// `key` on its own (function keys).
    pub const fn key(key: &'static str) -> Self {
        Self { command: false, shift: false, mac_ctrl: false, alt: false, key }
    }

    /// ⇧ and `key` (Shift+F3).
    pub const fn shift_key(key: &'static str) -> Self {
        Self { command: false, shift: true, mac_ctrl: false, alt: false, key }
    }

    /// The key as it is written in a menu: a name such as `OpenBracket` (what egui calls it) is the character.
    fn key_label(&self) -> &'static str {
        match self.key {
            "OpenBracket" => "[",
            "CloseBracket" => "]",
            "Backslash" => "\\",
            "Slash" => "/",
            "ArrowRight" => "→",
            "ArrowLeft" => "←",
            other => other,
        }
    }

    /// How the shortcut is written in menus: `⇧⌘S` on macOS, `Ctrl+Shift+S` elsewhere.
    pub fn label(&self, mac: bool) -> String {
        if mac {
            let mut s = String::new();
            if self.mac_ctrl {
                s.push('⌃');
            }
            if self.alt {
                s.push('⌥');
            }
            if self.shift {
                s.push('⇧');
            }
            if self.command {
                s.push('⌘');
            }
            s.push_str(self.key_label());
            s
        } else {
            let mut parts = Vec::new();
            if self.command || self.mac_ctrl {
                parts.push("Ctrl");
            }
            if self.alt {
                parts.push("Alt");
            }
            if self.shift {
                parts.push("Shift");
            }
            parts.push(self.key_label());
            parts.join("+")
        }
    }

    /// Number of modifiers (more specific shortcuts are matched first).
    pub fn modifier_count(&self) -> usize {
        usize::from(self.command) + usize::from(self.shift) + usize::from(self.mac_ctrl) + usize::from(self.alt)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Top-level menu it appears in (one of [`MENUS`]), if any.
    pub menu: Option<&'static str>,
    pub shortcut: Option<Shortcut>,
    pub needs: Needs,
    pub kind: Kind,
    /// A separator is drawn before it in its menu.
    pub separator_before: bool,
    /// The shortcut also works while a text field has focus.
    pub in_text: bool,
    /// Lucide icon name for palettes and toolbars.
    pub icon: &'static str,
}

const fn c(
    id: &'static str,
    label: &'static str,
    menu: Option<&'static str>,
    shortcut: Option<Shortcut>,
    needs: Needs,
    icon: &'static str,
) -> CommandSpec {
    CommandSpec { id, label, menu, shortcut, needs, kind: Kind::Action, separator_before: false, in_text: true, icon }
}

impl CommandSpec {
    const fn toggle(mut self) -> Self {
        self.kind = Kind::Toggle;
        self
    }

    const fn choice(mut self) -> Self {
        self.kind = Kind::Choice;
        self
    }

    /// The shortcut belongs to the focused text field while one has the focus (the editor does Undo, Redo and Select All itself).
    const fn not_in_text(mut self) -> Self {
        self.in_text = false;
        self
    }

    const fn sep(mut self) -> Self {
        self.separator_before = true;
        self
    }
}

use Needs::Nothing;

const FILE: Option<&str> = Some("File");
const EDIT: Option<&str> = Some("Edit");
const XML: Option<&str> = Some("XML");
const VIEW: Option<&str> = Some("View");
const HELP: Option<&str> = Some("Help");

/// The top-level menus, in order.
pub const MENUS: &[&str] = &["File", "Edit", "XML", "View", "Help"];

/// Every command, in menu order.
pub const COMMANDS: &[CommandSpec] = &[
    c("file.new", "New", FILE, Some(Shortcut::cmd("N")), Nothing, "file-plus"),
    c("file.new_from_template", "New from Template…", FILE, Some(Shortcut::cmd_shift("N")), Nothing, "file-plus"),
    c("file.open", "Open…", FILE, Some(Shortcut::cmd("O")), Nothing, "folder-open"),
    c("file.clear_recent", "Clear Recent Files", FILE, None, Nothing, "trash-2"),
    c("file.save", "Save", FILE, Some(Shortcut::cmd("S")), Needs::Document, "save").sep(),
    c("file.save_as", "Save As…", FILE, Some(Shortcut::cmd_shift("S")), Needs::Document, "save"),
    c("file.revert", "Revert", FILE, None, Needs::Document, "file-text"),
    c("file.close", "Close", FILE, Some(Shortcut::cmd("W")), Needs::Document, "x").sep(),
    c("file.quit", "Quit", FILE, Some(Shortcut::cmd("Q")), Nothing, "x"),
    c("edit.undo", "Undo", EDIT, Some(Shortcut::cmd("Z")), Needs::Undo, "undo-2").not_in_text(),
    c("edit.redo", "Redo", EDIT, Some(Shortcut::cmd_shift("Z")), Needs::Redo, "redo-2").not_in_text(),
    c("edit.select_all", "Select All", EDIT, Some(Shortcut::cmd("A")), Needs::Document, "align-left").sep().not_in_text(),
    c("edit.find", "Find…", EDIT, Some(Shortcut::cmd("F")), Needs::Document, "search").sep(),
    c("edit.replace", "Replace…", EDIT, Some(Shortcut::cmd_shift("H")), Needs::Document, "replace"),
    c("edit.find_next", "Find Next", EDIT, Some(Shortcut::key("F3")), Needs::Document, "chevron-down"),
    c("edit.find_previous", "Find Previous", EDIT, Some(Shortcut::shift_key("F3")), Needs::Document, "chevron-up"),
    c("edit.select_next_occurrence", "Select Next Occurrence", EDIT, Some(Shortcut::cmd("D")), Needs::Document, "plus").sep(),
    c("edit.select_all_occurrences", "Select All Occurrences", EDIT, Some(Shortcut::cmd_shift("L")), Needs::Document, "plus"),
    c("edit.go_to_line", "Go to Line…", EDIT, Some(Shortcut::cmd("G")), Needs::Document, "align-left").sep(),
    c("edit.go_to_matching_tag", "Go to Matching Tag", EDIT, Some(Shortcut::cmd_shift("Backslash")), Needs::Document, "code-xml"),
    c("edit.toggle_comment", "Toggle Comment", EDIT, Some(Shortcut::cmd("Slash")), Needs::Document, "code-xml").sep(),
    c("edit.expand_selection", "Expand Selection", EDIT, Some(Shortcut::cmd_alt("ArrowRight")), Needs::Document, "chevron-right"),
    c("edit.shrink_selection", "Shrink Selection", EDIT, Some(Shortcut::cmd_alt("ArrowLeft")), Needs::Document, "chevron-left"),
    c("edit.fold", "Fold", EDIT, Some(Shortcut::cmd_alt("OpenBracket")), Needs::Document, "chevron-right").sep(),
    c("edit.unfold", "Unfold", EDIT, Some(Shortcut::cmd_alt("CloseBracket")), Needs::Document, "chevron-down"),
    c("edit.fold_all", "Fold All", EDIT, None, Needs::Document, "chevron-right"),
    c("edit.unfold_all", "Unfold All", EDIT, None, Needs::Document, "chevron-down"),
    c("edit.fold_level_1", "Fold Level 1", EDIT, None, Needs::Document, "chevron-right"),
    c("edit.fold_level_2", "Fold Level 2", EDIT, None, Needs::Document, "chevron-right"),
    c("edit.fold_level_3", "Fold Level 3", EDIT, None, Needs::Document, "chevron-right"),
    c("xml.check", "Check Well-formedness", XML, Some(Shortcut::key("F7")), Needs::Document, "file-check"),
    c("xml.validate", "Validate", XML, Some(Shortcut::key("F8")), Needs::Document, "shield-check"),
    c("xml.validate_typing", "Validate While Typing", XML, None, Nothing, "shield-check").toggle(),
    c("xml.assign_schema", "Assign Schema…", XML, None, Needs::Document, "book-open"),
    c("xml.pretty_print", "Pretty-print", XML, None, Needs::Document, "wand-sparkles").sep(),
    c("xml.minify", "Strip White Space", XML, None, Needs::Document, "align-left"),
    c("xml.generate_schema", "Generate Schema…", XML, None, Needs::Document, "file-code").sep(),
    c("xml.generate_sample", "Generate Sample XML…", XML, None, Needs::Document, "sparkles"),
    c("view.palette", "Find tools and commands…", VIEW, Some(Shortcut::cmd("K")), Nothing, "search"),
    c("view.outline", "Outline", VIEW, None, Nothing, "list-tree").toggle().sep(),
    c("view.entry_helpers", "Entry Helpers", VIEW, None, Nothing, "braces").toggle(),
    c("view.messages", "Messages", VIEW, None, Nothing, "panel-bottom").toggle(),
    c("view.theme.system", "Use system theme", VIEW, None, Nothing, "monitor").choice().sep(),
    c("view.theme.light", "Light theme", VIEW, None, Nothing, "sun").choice(),
    c("view.theme.dark", "Dark theme", VIEW, None, Nothing, "moon").choice(),
    c("app.about", "About XmlCraft", HELP, None, Nothing, "circle-help"),
];

/// The command with this id.
pub fn command(id: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|c| c.id == id)
}

/// The commands of a top-level menu, in order.
pub fn in_menu(menu: &str) -> impl Iterator<Item = &'static CommandSpec> + '_ {
    COMMANDS.iter().filter(move |c| c.menu == Some(menu))
}

/// Whether the command can run now, judged from the session's documents. State the engine cannot see (which pane is open) is
/// checked by the frontend.
pub fn is_enabled(spec: &CommandSpec, session: &Session) -> bool {
    match spec.needs {
        Needs::Nothing => true,
        Needs::Document => session.has_document(),
        Needs::Undo => session.can_undo(),
        Needs::Redo => session.can_redo(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_and_look_like_area_dot_name() {
        let mut seen = HashSet::new();
        for c in COMMANDS {
            assert!(seen.insert(c.id), "duplicate id {}", c.id);
            let (area, name) = c.id.split_once('.').unwrap_or_else(|| panic!("{} has no area", c.id));
            assert!(!area.is_empty() && !name.is_empty());
            assert!(c.id.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '.' || ch == '_'), "{}", c.id);
        }
    }

    #[test]
    fn every_menu_command_names_a_known_menu() {
        for c in COMMANDS {
            if let Some(menu) = c.menu {
                assert!(MENUS.contains(&menu), "{} is in the unknown menu {menu}", c.id);
            }
        }
    }

    #[test]
    fn no_two_commands_share_a_shortcut() {
        let mut seen = HashSet::new();
        for c in COMMANDS {
            if let Some(s) = c.shortcut {
                assert!(seen.insert(s), "{} reuses a shortcut: {}", c.id, s.label(true));
            }
        }
    }

    #[test]
    fn labels_are_not_empty_and_commands_have_icons() {
        for c in COMMANDS {
            assert!(!c.label.trim().is_empty() && !c.icon.is_empty(), "{}", c.id);
        }
    }

    #[test]
    fn lookup_finds_a_command_and_not_a_missing_one() {
        assert_eq!(command("view.palette").map(|c| c.label), Some("Find tools and commands…"));
        assert!(command("no.such").is_none() && command("").is_none());
    }

    #[test]
    fn menus_list_their_commands_in_table_order() {
        let view: Vec<&str> = in_menu("View").map(|c| c.id).collect();
        assert_eq!(view.first(), Some(&"view.palette"));
        assert!(view.contains(&"view.theme.dark"));
        assert_eq!(
            in_menu("File").map(|c| c.id).collect::<Vec<_>>(),
            [
                "file.new",
                "file.new_from_template",
                "file.open",
                "file.clear_recent",
                "file.save",
                "file.save_as",
                "file.revert",
                "file.close",
                "file.quit"
            ]
        );
        assert_eq!(
            in_menu("Edit").map(|c| c.id).collect::<Vec<_>>(),
            [
                "edit.undo",
                "edit.redo",
                "edit.select_all",
                "edit.find",
                "edit.replace",
                "edit.find_next",
                "edit.find_previous",
                "edit.select_next_occurrence",
                "edit.select_all_occurrences",
                "edit.go_to_line",
                "edit.go_to_matching_tag",
                "edit.toggle_comment",
                "edit.expand_selection",
                "edit.shrink_selection",
                "edit.fold",
                "edit.unfold",
                "edit.fold_all",
                "edit.unfold_all",
                "edit.fold_level_1",
                "edit.fold_level_2",
                "edit.fold_level_3"
            ]
        );
        assert_eq!(
            in_menu("XML").map(|c| c.id).collect::<Vec<_>>(),
            [
                "xml.check",
                "xml.validate",
                "xml.validate_typing",
                "xml.assign_schema",
                "xml.pretty_print",
                "xml.minify",
                "xml.generate_schema",
                "xml.generate_sample"
            ]
        );
        assert_eq!(in_menu("Nope").count(), 0);
    }

    #[test]
    fn shortcuts_read_like_each_platform() {
        let save = Shortcut::cmd_shift("S");
        assert_eq!(save.label(true), "⇧⌘S");
        assert_eq!(save.label(false), "Ctrl+Shift+S");
        assert_eq!(Shortcut::cmd("K").label(true), "⌘K");
        assert_eq!(Shortcut::key("F8").label(false), "F8");
        assert_eq!(Shortcut { command: true, shift: false, mac_ctrl: true, alt: false, key: "I" }.label(true), "⌃⌘I");
        assert_eq!(Shortcut { command: true, shift: true, mac_ctrl: true, alt: false, key: "I" }.modifier_count(), 3);
        assert_eq!(Shortcut::cmd_alt("OpenBracket").label(true), "⌥⌘[");
        assert_eq!(Shortcut::cmd_alt("OpenBracket").label(false), "Ctrl+Alt+[");
        assert_eq!(Shortcut::cmd_alt("OpenBracket").modifier_count(), 2);
        assert_eq!(Shortcut::cmd_alt("CloseBracket").label(true), "⌥⌘]");
        assert_eq!(Shortcut::cmd("Slash").label(true), "⌘/");
        assert_eq!(Shortcut::cmd_alt("ArrowRight").label(false), "Ctrl+Alt+→");
        assert_eq!(Shortcut::cmd_shift("Backslash").label(true), "⇧⌘\\");
        assert_eq!(Shortcut::cmd_shift("Backslash").label(false), "Ctrl+Shift+\\");
        // No menu shows the name egui gives a key.
        for c in COMMANDS {
            if let Some(s) = c.shortcut {
                let (mac, other) = (s.label(true), s.label(false));
                assert!(
                    !mac.contains("Bracket") && !other.contains("Backslash") && !other.contains("Arrow") && !other.contains("Slash"),
                    "{}: {other}",
                    c.id
                );
            }
        }
    }

    #[test]
    fn needs_are_judged_from_the_session() {
        let s = Session::new();
        let quit = command("file.quit").expect("registered");
        assert!(is_enabled(quit, &s));
        let doc = CommandSpec { needs: Needs::Document, ..*quit };
        assert!(!is_enabled(&doc, &s), "no document is open");
        let undo = CommandSpec { needs: Needs::Undo, ..*quit };
        let redo = CommandSpec { needs: Needs::Redo, ..*quit };
        assert!(!is_enabled(&undo, &s) && !is_enabled(&redo, &s));
    }

    #[test]
    fn toggles_and_choices_are_marked() {
        assert_eq!(command("view.outline").map(|c| c.kind), Some(Kind::Toggle));
        assert_eq!(command("view.theme.light").map(|c| c.kind), Some(Kind::Choice));
        assert_eq!(command("file.quit").map(|c| c.kind), Some(Kind::Action));
    }
}
