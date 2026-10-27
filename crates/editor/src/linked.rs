//! Linked names: the name in an element's start tag and the one in its end tag are renamed together.
//!
//! The view (or a tool) tells the editor which two names belong together ([`Editor::link_names`]); from then on typing or deleting in one
//! of them writes the same change into the other, as part of the same undo step. The link is kept through other edits and dropped as soon
//! as something else happens to a name (a character that cannot be in a name, a paste, a selection that reaches out of it, an undo).

use std::ops::Range;

use xmlcraft_syntax::is_name_char;
use xmlcraft_text::{Edit, EditKind};

use crate::caret::Caret;
use crate::editor::{Bias, Editor, map_offset};

/// The longest name kept in step: the two names are compared before every change, so their length is bounded.
const MAX_LINKED_NAME: usize = 1024;

/// What a change does to the linked names: the edits to make (the typed one, and the one that mirrors it), the carets after them and
/// the names as they will be, if they stay linked.
pub(crate) struct LinkedStep {
    pub edits: Vec<Edit>,
    pub carets: Vec<Caret>,
    pub names: Option<[Range<usize>; 2]>,
}

impl Editor {
    /// Keep the names `a` and `b` (byte ranges of the text as it is now) in step. They are linked only if both are the same, non-empty
    /// name, apart from each other, and [`crate::Settings::sync_tag_names`] is on; otherwise any earlier link is dropped. Returns whether
    /// they are linked.
    pub fn link_names(&mut self, a: Range<usize>, b: Range<usize>) -> bool {
        self.linked = None;
        if !self.settings().sync_tag_names || a.is_empty() || a.len() > MAX_LINKED_NAME || a.len() != b.len() {
            return false;
        }
        if !(a.end <= b.start || b.end <= a.start) {
            return false;
        }
        let (Ok(first), Ok(second)) = (self.buffer.slice(a.clone()), self.buffer.slice(b.clone())) else { return false };
        if first != second || !first.chars().all(is_name_char) || !self.names_a_tag(&a) || !self.names_a_tag(&b) {
            return false;
        }
        self.linked = Some([a, b]);
        true
    }

    /// Whether `name` stands where the name of a tag does: after `<` or `</`, and not followed by more of a name.
    fn names_a_tag(&self, name: &Range<usize>) -> bool {
        let before = self.buffer.prev_char(name.start);
        let opened = match self.buffer.char_at(before) {
            Some('<') => true,
            Some('/') => self.buffer.char_at(self.buffer.prev_char(before)) == Some('<'),
            _ => false,
        };
        opened && self.buffer.char_at(name.end).is_none_or(|c| !is_name_char(c))
    }

    /// Stop keeping names in step.
    pub fn unlink_names(&mut self) {
        self.linked = None;
    }

    /// The two names that are kept in step, if there are any.
    pub fn linked_names(&self) -> Option<&[Range<usize>; 2]> {
        self.linked.as_ref()
    }

    /// Whether the editor has a single caret (no selection) in or next to one of the linked names.
    pub fn caret_in_linked_names(&self) -> bool {
        let [caret] = self.carets.as_slice() else { return false };
        caret.is_empty() && self.linked.as_ref().is_some_and(|names| names.iter().any(|name| name.start <= caret.head() && caret.head() <= name.end))
    }

    /// What linked names make of `edits` (ascending, in the coordinates of the text as it is now) that are about to be made as `kind`
    /// with `carets` as the result: a single change inside one name is written into the other name too.
    pub(crate) fn step_linked(&self, mut edits: Vec<Edit>, mut carets: Vec<Caret>, kind: EditKind) -> LinkedStep {
        let Some(names) = &self.linked else { return LinkedStep { edits, carets, names: None } };
        // Edits elsewhere can have taken the tags apart (the `</` of the end tag deleted, say): what is left is text, not a name to follow.
        if !names.iter().all(|name| self.names_a_tag(name)) {
            return LinkedStep { edits, carets, names: None };
        }
        let mirror = match edits.as_slice() {
            [edit] if self.carets.len() == 1 && matches!(kind, EditKind::Typing | EditKind::Delete) => self.mirror_of(names, edit),
            _ => None,
        };
        let mirrored = mirror.is_some();
        if let Some(mirror) = mirror {
            // The carets were worked out for the typed edit alone: the mirrored one moves them when it is before them.
            let delta = mirror.delta();
            let before = edits.first().is_some_and(|edit| mirror.range.end <= edit.range.start);
            if before {
                for caret in &mut carets {
                    caret.sel.anchor = caret.sel.anchor.saturating_add_signed(delta);
                    caret.sel.head = caret.sel.head.saturating_add_signed(delta);
                }
            }
            edits.push(mirror);
            edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
        }
        let names = remap(names, &edits, mirrored);
        LinkedStep { edits, carets, names }
    }

    /// The change to the other name that goes with `edit`, when `edit` is a change of one name, made of characters a name can have, and
    /// the two names are still the same.
    fn mirror_of(&self, names: &[Range<usize>; 2], edit: &Edit) -> Option<Edit> {
        let within = |name: &Range<usize>| name.start <= edit.range.start && edit.range.end <= name.end;
        let (from, to) = if within(&names[0]) {
            (&names[0], &names[1])
        } else if within(&names[1]) {
            (&names[1], &names[0])
        } else {
            return None;
        };
        if !edit.text.chars().all(is_name_char) || from.len() != to.len() {
            return None;
        }
        if self.buffer.slice(from.clone()).ok()? != self.buffer.slice(to.clone()).ok()? {
            return None;
        }
        let start = to.start.checked_add(edit.range.start.checked_sub(from.start)?)?;
        let end = to.start.checked_add(edit.range.end.checked_sub(from.start)?)?;
        Some(Edit::replace(start..end, edit.text.clone()))
    }
}

/// The names after `edits` (ascending, not overlapping). A change inside a name is allowed only when it has been mirrored; one that
/// reaches into a name from outside drops the link. A name grows with what is typed at either of its ends.
fn remap(names: &[Range<usize>; 2], edits: &[Edit], mirrored: bool) -> Option<[Range<usize>; 2]> {
    for edit in edits {
        for name in names {
            let inside = name.start <= edit.range.start && edit.range.end <= name.end;
            let outside = edit.range.end <= name.start || edit.range.start >= name.end;
            if (inside && !mirrored) || (!inside && !outside) {
                return None;
            }
        }
    }
    let map = |name: &Range<usize>| map_offset(edits, name.start, Bias::Before)..map_offset(edits, name.end, Bias::After);
    Some([map(&names[0]), map(&names[1])])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Motion, Settings};
    use xmlcraft_text::Selection;

    /// `<a>x</a>` with the caret at the end of the start tag's name and the two names linked.
    fn linked(text: &str, a: Range<usize>, b: Range<usize>, caret: usize) -> Editor {
        let mut e = Editor::new(text);
        assert!(e.link_names(a, b), "{text}");
        e.set_selection(caret, caret);
        e
    }

    #[test]
    fn typing_in_a_name_is_written_into_the_other_name_as_one_undo_step() {
        let mut e = linked("<a>x</a>", 1..2, 6..7, 2);
        e.type_input("b");
        assert_eq!((e.text().as_str(), e.primary().head()), ("<ab>x</ab>", 3));
        assert_eq!(e.linked_names(), Some(&[1..3, 7..9]));
        e.type_input("c");
        assert_eq!((e.text().as_str(), e.primary().head()), ("<abc>x</abc>", 4));
        // Typing is merged into one step, and the step holds both names.
        assert!(e.undo());
        assert_eq!(e.text(), "<a>x</a>");
        assert!(!e.undo());
    }

    #[test]
    fn it_works_from_the_end_tag_too_and_the_caret_stays_where_it_typed() {
        let mut e = linked("<a>x</a>", 1..2, 6..7, 7);
        e.type_input("b");
        assert_eq!((e.text().as_str(), e.primary().head()), ("<ab>x</ab>", 9));
        e.backspace();
        assert_eq!((e.text().as_str(), e.primary().head()), ("<a>x</a>", 7));
        assert_eq!(e.linked_names(), Some(&[1..2, 6..7]));
    }

    #[test]
    fn deleting_replacing_and_typing_at_the_start_of_a_name_are_mirrored() {
        let mut e = linked("<ab>x</ab>", 1..3, 7..9, 3);
        e.backspace();
        assert_eq!(e.text(), "<a>x</a>");
        e.set_selection(1, 1);
        e.delete_forward();
        assert_eq!(e.text(), "<>x</>", "a name can be emptied and typed again");
        e.type_input("q");
        assert_eq!(e.text(), "<q>x</q>");
        e.set_selection(2, 1);
        e.type_input("pr");
        assert_eq!(e.text(), "<pr>x</pr>");
        e.set_selection(1, 1);
        e.type_input("s");
        assert_eq!(e.text(), "<spr>x</spr>", "typing at the start of a name goes into it");
    }

    #[test]
    fn something_that_is_not_part_of_a_name_ends_the_link_and_is_not_mirrored() {
        for typed in [" ", "/", "\"", "é!"] {
            let mut e = linked("<a>x</a>", 1..2, 6..7, 2);
            e.type_input(typed);
            assert_eq!(e.linked_names(), None, "{typed:?}");
            assert!(e.text().ends_with("x</a>"), "{typed:?}: {}", e.text());
        }
        // A letter with an accent is a name character and goes along.
        let mut e = linked("<a>x</a>", 1..2, 6..7, 2);
        e.type_input("é");
        assert_eq!(e.text(), "<aé>x</aé>");
    }

    #[test]
    fn stepping_over_the_bracket_leaves_the_name_and_a_torn_tag_is_not_followed() {
        let mut e = linked("<a>x</a>", 1..2, 6..7, 2);
        e.type_input(">");
        assert_eq!((e.text().as_str(), e.primary().head()), ("<a>x</a>", 3));
        assert!(!e.caret_in_linked_names());
        // The `</` of the end tag is deleted (by something that is not typing): the `a` that is left is text, not a name to follow.
        let mut e = linked("<a>x</a>", 1..2, 6..7, 2);
        assert!(e.apply_edits(&[Edit::delete(4..6)]));
        assert_eq!((e.text().as_str(), e.linked_names()), ("<a>xa>", Some(&[1..2, 4..5])));
        e.set_selection(2, 2);
        e.type_input("b");
        assert_eq!(e.text(), "<ab>xa>");
        assert_eq!(e.linked_names(), None);
    }

    #[test]
    fn names_are_linked_only_when_they_are_the_same_name() {
        let mut e = Editor::new("<a>x</b>");
        assert!(!e.link_names(1..2, 6..7), "different names");
        assert!(!e.link_names(1..1, 6..6), "empty");
        assert!(!e.link_names(1..2, 1..2), "the same place");
        assert!(!e.link_names(1..2, 9..10), "past the end");
        assert!(!e.link_names(2..3, 6..7), "not a name");
        let mut e = Editor::new("<a>a</a>");
        assert!(!e.link_names(1..2, 3..4), "text between tags is not a tag name");
        assert!(e.link_names(1..2, 6..7));
        let mut e = Editor::new("<é>x</é>");
        assert!(!e.link_names(1..2, 6..8), "inside a character");
        assert!(e.link_names(1..3, 7..9));
        let mut e = Editor::with_settings("<a></a>", Settings { sync_tag_names: false, ..Settings::default() });
        assert!(!e.link_names(1..2, 5..6), "switched off");
    }

    #[test]
    fn several_carets_or_a_change_that_is_not_typing_end_the_link() {
        let mut e = linked("<a>x</a>", 1..2, 6..7, 2);
        e.set_selections(&[Selection::caret(2), Selection::caret(3)]);
        e.type_input("b");
        assert_eq!(e.text(), "<ab>bx</a>");
        assert_eq!(e.linked_names(), None);
        let mut e = linked("<a>x</a>", 1..2, 6..7, 2);
        e.paste("bc");
        assert_eq!(e.text(), "<abc>x</a>", "a paste is not mirrored");
        assert_eq!(e.linked_names(), None);
        let mut e = linked("<a>x</a>", 1..2, 6..7, 2);
        e.type_input("b");
        e.undo();
        assert_eq!(e.linked_names(), None, "an undo is not typing");
    }

    #[test]
    fn edits_elsewhere_move_the_names_and_edits_reaching_into_them_end_the_link() {
        let mut e = linked("<a>x</a>", 1..2, 6..7, 2);
        e.set_selection(0, 0);
        e.type_input("zz");
        assert_eq!(e.linked_names(), Some(&[3..4, 8..9]));
        e.set_selection(5, 6);
        e.type_input("yyy");
        assert_eq!(e.text(), "zz<a>yyy</a>");
        assert_eq!(e.linked_names(), Some(&[3..4, 10..11]));
        assert!(e.apply_edits(&[Edit::delete(2..4)]), "from before the name into it");
        assert_eq!(e.linked_names(), None);
    }

    proptest::proptest! {
        /// Whatever is typed, deleted and moved around in a linked name, the two names stay the same as long as they are linked.
        #[test]
        fn linked_names_never_drift_apart(
            ops in proptest::collection::vec((0usize..8, proptest::sample::select(&["a", "b", "é", "-", " ", ">"][..])), 0..40),
        ) {
            let mut e = Editor::new("<ab>x</ab>");
            proptest::prop_assert!(e.link_names(1..3, 7..9));
            e.set_selection(2, 2);
            for (op, text) in ops {
                match op {
                    0..=2 => drop(e.type_input(text)),
                    3 => drop(e.backspace()),
                    4 => drop(e.delete_forward()),
                    5 => e.move_carets(Motion::Left, false),
                    6 => e.move_carets(Motion::Right, false),
                    _ => e.move_carets(Motion::Left, true),
                }
                if let Some([a, b]) = e.linked_names() {
                    let (first, second) = (e.buffer().slice(a.clone()), e.buffer().slice(b.clone()));
                    proptest::prop_assert!(first.is_ok() && first == second, "{:?}: {} {:?}", e.linked_names(), e.text(), (first, second));
                    proptest::prop_assert!(a.end <= b.start, "{:?}", e.linked_names());
                }
            }
        }
    }
}
