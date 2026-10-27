//! The colours the editor draws with. The host fills them in from its theme; this crate knows no theme.

use egui::Color32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditorColors {
    /// The text area.
    pub background: Color32,
    pub text: Color32,
    pub caret: Color32,
    pub selection: Color32,
    /// The selection while the editor does not have the keyboard focus.
    pub selection_inactive: Color32,
    /// The wash behind the line the caret is on.
    pub current_line: Color32,
    pub gutter_background: Color32,
    pub gutter_text: Color32,
    pub gutter_text_current: Color32,
    /// The line between the gutter and the text.
    pub gutter_border: Color32,
    /// The text being composed through an input method.
    pub preedit_background: Color32,
}

impl EditorColors {
    pub fn light() -> Self {
        Self {
            background: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            text: Color32::from_rgb(0x22, 0x22, 0x26),
            caret: Color32::from_rgb(0x1B, 0x63, 0xE0),
            selection: Color32::from_rgba_unmultiplied(0x1B, 0x63, 0xE0, 0x38),
            selection_inactive: Color32::from_rgba_unmultiplied(0x80, 0x80, 0x88, 0x30),
            current_line: Color32::from_rgb(0xF6, 0xF8, 0xFD),
            gutter_background: Color32::from_rgb(0xFA, 0xFA, 0xFB),
            gutter_text: Color32::from_rgb(0x9A, 0x9A, 0xA3),
            gutter_text_current: Color32::from_rgb(0x44, 0x44, 0x4B),
            gutter_border: Color32::from_rgb(0xE8, 0xE8, 0xEB),
            preedit_background: Color32::from_rgb(0xE3, 0xEC, 0xFD),
        }
    }

    pub fn dark() -> Self {
        Self {
            background: Color32::from_rgb(0x1E, 0x1E, 0x22),
            text: Color32::from_rgb(0xEC, 0xEC, 0xEF),
            caret: Color32::from_rgb(0x4B, 0x8B, 0xF5),
            selection: Color32::from_rgba_unmultiplied(0x4B, 0x8B, 0xF5, 0x48),
            selection_inactive: Color32::from_rgba_unmultiplied(0xA0, 0xA0, 0xA8, 0x30),
            current_line: Color32::from_rgb(0x25, 0x25, 0x2B),
            gutter_background: Color32::from_rgb(0x22, 0x22, 0x26),
            gutter_text: Color32::from_rgb(0x70, 0x70, 0x79),
            gutter_text_current: Color32::from_rgb(0xC4, 0xC4, 0xCB),
            gutter_border: Color32::from_rgb(0x33, 0x33, 0x39),
            preedit_background: Color32::from_rgb(0x2E, 0x3A, 0x55),
        }
    }
}
