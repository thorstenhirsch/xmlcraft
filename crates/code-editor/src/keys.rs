//! Which key does what. Pure, so every binding is tested without a window.

use egui::{Key, Modifiers};
use xmlcraft_editor::Motion;

/// What a key press asks the editor to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Move(Motion, bool),
    Backspace,
    Delete,
    DeleteWordBackward,
    DeleteWordForward,
    DeleteToLineStart,
    DeleteToLineEnd,
    Newline,
    Indent,
    Outdent,
    SelectAll,
    Undo,
    Redo,
    /// Escape: drop the extra carets.
    KeepPrimaryCaret,
}

/// The command for `key` pressed with `modifiers`. `mac` selects the macOS conventions (Option moves by word, Command by line).
/// `page` is how many lines a Page Up or Page Down moves.
pub fn command_for(key: Key, modifiers: Modifiers, mac: bool, page: usize) -> Option<Command> {
    let extend = modifiers.shift;
    // The modifier that makes a move go by word, and the one that makes it go to the edge of the line or document.
    let by_word = if mac { modifiers.alt } else { modifiers.ctrl };
    let to_edge = if mac { modifiers.mac_cmd } else { false };
    let page = page.max(1);
    Some(match key {
        Key::ArrowLeft if by_word => Command::Move(Motion::WordLeft, extend),
        Key::ArrowRight if by_word => Command::Move(Motion::WordRight, extend),
        Key::ArrowLeft if to_edge => Command::Move(Motion::LineStart, extend),
        Key::ArrowRight if to_edge => Command::Move(Motion::LineEnd, extend),
        Key::ArrowUp if to_edge => Command::Move(Motion::DocStart, extend),
        Key::ArrowDown if to_edge => Command::Move(Motion::DocEnd, extend),
        Key::ArrowLeft => Command::Move(Motion::Left, extend),
        Key::ArrowRight => Command::Move(Motion::Right, extend),
        Key::ArrowUp => Command::Move(Motion::Up, extend),
        Key::ArrowDown => Command::Move(Motion::Down, extend),
        Key::Home if modifiers.command => Command::Move(Motion::DocStart, extend),
        Key::End if modifiers.command => Command::Move(Motion::DocEnd, extend),
        Key::Home => Command::Move(Motion::LineStart, extend),
        Key::End => Command::Move(Motion::LineEnd, extend),
        Key::PageUp => Command::Move(Motion::PageUp(page), extend),
        Key::PageDown => Command::Move(Motion::PageDown(page), extend),
        Key::Backspace if to_edge => Command::DeleteToLineStart,
        Key::Backspace if by_word => Command::DeleteWordBackward,
        Key::Backspace => Command::Backspace,
        Key::Delete if by_word => Command::DeleteWordForward,
        Key::Delete => Command::Delete,
        Key::Enter => Command::Newline,
        Key::Tab if modifiers.shift => Command::Outdent,
        Key::Tab => Command::Indent,
        Key::Escape => Command::KeepPrimaryCaret,
        Key::A if modifiers.command && !modifiers.alt => Command::SelectAll,
        Key::Z if modifiers.command && modifiers.shift => Command::Redo,
        Key::Z if modifiers.command => Command::Undo,
        Key::Y if modifiers.command && !mac => Command::Redo,
        Key::K if modifiers.ctrl && !mac && modifiers.shift => Command::DeleteToLineEnd,
        Key::K if modifiers.ctrl && mac => Command::DeleteToLineEnd,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers::NONE;

    fn mods(f: impl FnOnce(&mut Modifiers)) -> Modifiers {
        let mut m = Modifiers::NONE;
        f(&mut m);
        m
    }

    #[test]
    fn plain_keys() {
        assert_eq!(command_for(Key::ArrowLeft, NONE, false, 10), Some(Command::Move(Motion::Left, false)));
        assert_eq!(command_for(Key::ArrowDown, mods(|m| m.shift = true), false, 10), Some(Command::Move(Motion::Down, true)));
        assert_eq!(command_for(Key::Enter, NONE, false, 10), Some(Command::Newline));
        assert_eq!(command_for(Key::Tab, NONE, false, 10), Some(Command::Indent));
        assert_eq!(command_for(Key::Tab, mods(|m| m.shift = true), false, 10), Some(Command::Outdent));
        assert_eq!(command_for(Key::Backspace, NONE, true, 10), Some(Command::Backspace));
        assert_eq!(command_for(Key::Delete, NONE, true, 10), Some(Command::Delete));
        assert_eq!(command_for(Key::PageDown, NONE, false, 0), Some(Command::Move(Motion::PageDown(1), false)), "a page is at least a line");
        assert_eq!(command_for(Key::Escape, NONE, false, 10), Some(Command::KeepPrimaryCaret));
        assert_eq!(command_for(Key::F5, NONE, false, 10), None);
        assert_eq!(command_for(Key::Q, NONE, false, 10), None, "letters are text, not commands");
    }

    #[test]
    fn windows_and_linux() {
        let ctrl = mods(|m| {
            m.ctrl = true;
            m.command = true;
        });
        assert_eq!(command_for(Key::ArrowLeft, ctrl, false, 10), Some(Command::Move(Motion::WordLeft, false)));
        assert_eq!(command_for(Key::Backspace, ctrl, false, 10), Some(Command::DeleteWordBackward));
        assert_eq!(command_for(Key::Delete, ctrl, false, 10), Some(Command::DeleteWordForward));
        assert_eq!(command_for(Key::Home, ctrl, false, 10), Some(Command::Move(Motion::DocStart, false)));
        assert_eq!(command_for(Key::End, NONE, false, 10), Some(Command::Move(Motion::LineEnd, false)));
        assert_eq!(command_for(Key::Z, ctrl, false, 10), Some(Command::Undo));
        assert_eq!(command_for(Key::Y, ctrl, false, 10), Some(Command::Redo));
        assert_eq!(command_for(Key::A, ctrl, false, 10), Some(Command::SelectAll));
        let ctrl_shift = mods(|m| {
            m.ctrl = true;
            m.command = true;
            m.shift = true;
        });
        assert_eq!(command_for(Key::Z, ctrl_shift, false, 10), Some(Command::Redo));
        assert_eq!(command_for(Key::K, ctrl_shift, false, 10), Some(Command::DeleteToLineEnd));
    }

    #[test]
    fn macos() {
        let option = mods(|m| m.alt = true);
        let command = mods(|m| {
            m.mac_cmd = true;
            m.command = true;
        });
        assert_eq!(command_for(Key::ArrowRight, option, true, 10), Some(Command::Move(Motion::WordRight, false)));
        assert_eq!(command_for(Key::Backspace, option, true, 10), Some(Command::DeleteWordBackward));
        assert_eq!(command_for(Key::ArrowLeft, command, true, 10), Some(Command::Move(Motion::LineStart, false)));
        assert_eq!(command_for(Key::ArrowDown, command, true, 10), Some(Command::Move(Motion::DocEnd, false)));
        assert_eq!(command_for(Key::Backspace, command, true, 10), Some(Command::DeleteToLineStart));
        assert_eq!(command_for(Key::Z, command, true, 10), Some(Command::Undo));
        assert_eq!(command_for(Key::Y, command, true, 10), None, "Command-Y is not redo on a Mac");
        let shift_command = mods(|m| {
            m.mac_cmd = true;
            m.command = true;
            m.shift = true;
        });
        assert_eq!(command_for(Key::Z, shift_command, true, 10), Some(Command::Redo));
        assert_eq!(command_for(Key::ArrowLeft, shift_command, true, 10), Some(Command::Move(Motion::LineStart, true)));
        assert_eq!(command_for(Key::K, mods(|m| m.ctrl = true), true, 10), Some(Command::DeleteToLineEnd));
    }
}
