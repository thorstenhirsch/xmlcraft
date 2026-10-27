//! The editor against a plain `String`: multi-caret typing and deleting give what rebuilding the string by hand gives, and after any
//! sequence of commands the carets are valid and undo and redo agree with the text.

use proptest::prelude::*;
use xmlcraft_editor::{Editor, FoldMap, Motion};
use xmlcraft_text::Selection;

/// Pieces of text: ASCII, line breaks of both kinds, tabs, multi-byte characters, a combining mark, an emoji.
const PIECES: &[&str] =
    &["a", "b", "xy", " ", "  ", "\n", "\r\n", "\t", "<", "/>", "\"", "é", "日", "😀", "e\u{301}", "-", ".", "=", "'", "<a ", ">", "<!--", "-->"];

fn text() -> impl Strategy<Value = String> {
    proptest::collection::vec(proptest::sample::select(PIECES), 0..30).prop_map(|v| v.concat())
}

fn ascii_text() -> impl Strategy<Value = String> {
    "[a-z \n]{0,40}"
}

#[derive(Clone, Debug)]
enum Op {
    Type(String),
    /// Typing on the keyboard: with the aids.
    Input(String),
    Paste(String),
    Backspace,
    Delete,
    Newline,
    WordBackward,
    WordForward,
    DeleteToLineStart,
    DeleteToLineEnd,
    Indent,
    Outdent,
    Cut,
    Move(usize, bool),
    SelectAll,
    Place(usize, bool),
    SelectWord(usize, bool),
    SelectLine(usize, bool),
    Extend(usize),
    Fold(usize, usize),
    Unfold(usize),
    UnfoldAll,
}

const MOTIONS: &[Motion] = &[
    Motion::Left,
    Motion::Right,
    Motion::Up,
    Motion::Down,
    Motion::WordLeft,
    Motion::WordRight,
    Motion::LineStart,
    Motion::LineEnd,
    Motion::DocStart,
    Motion::DocEnd,
    Motion::PageUp(3),
    Motion::PageDown(3),
];

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => text().prop_map(Op::Type),
        3 => proptest::collection::vec(proptest::sample::select(&["=", "\"", "'", "a", " ", ">", "<", "/", "\n", "é"][..]), 1..3).prop_map(|v| Op::Input(v.concat())),
        1 => text().prop_map(Op::Paste),
        2 => Just(Op::Backspace),
        1 => Just(Op::Delete),
        1 => Just(Op::Newline),
        1 => Just(Op::WordBackward),
        1 => Just(Op::WordForward),
        1 => Just(Op::DeleteToLineStart),
        1 => Just(Op::DeleteToLineEnd),
        1 => Just(Op::Indent),
        1 => Just(Op::Outdent),
        1 => Just(Op::Cut),
        4 => (0..MOTIONS.len(), any::<bool>()).prop_map(|(m, e)| Op::Move(m, e)),
        1 => Just(Op::SelectAll),
        2 => (0usize..200, any::<bool>()).prop_map(|(o, a)| Op::Place(o, a)),
        1 => (0usize..200, any::<bool>()).prop_map(|(o, a)| Op::SelectWord(o, a)),
        1 => (0usize..200, any::<bool>()).prop_map(|(o, a)| Op::SelectLine(o, a)),
        1 => (0usize..200).prop_map(Op::Extend),
        2 => (0usize..12, 1usize..6).prop_map(|(first, len)| Op::Fold(first, first + len)),
        1 => (0usize..12).prop_map(Op::Unfold),
        1 => Just(Op::UnfoldAll),
    ]
}

fn apply(e: &mut Editor, op: &Op) {
    match op {
        Op::Type(t) => drop(e.type_text(t)),
        // Key by key, as they are typed: the aids look at what is before the caret.
        Op::Input(t) => {
            for c in t.chars() {
                let _ = e.type_input(&c.to_string());
            }
        }
        Op::Paste(t) => drop(e.paste(t)),
        Op::Backspace => drop(e.backspace()),
        Op::Delete => drop(e.delete_forward()),
        Op::Newline => drop(e.insert_newline()),
        Op::WordBackward => drop(e.delete_word_backward()),
        Op::WordForward => drop(e.delete_word_forward()),
        Op::DeleteToLineStart => drop(e.delete_to_line_start()),
        Op::DeleteToLineEnd => drop(e.delete_to_line_end()),
        Op::Indent => drop(e.indent()),
        Op::Outdent => drop(e.outdent()),
        Op::Cut => drop(e.cut()),
        Op::Move(m, extend) => e.move_carets(MOTIONS[*m], *extend),
        Op::SelectAll => e.select_all(),
        Op::Place(o, add) => e.place_caret(*o, *add),
        Op::SelectWord(o, add) => e.select_word_at(*o, *add),
        Op::SelectLine(o, add) => e.select_line_at(*o, *add),
        Op::Extend(o) => e.extend_selection_to(*o),
        Op::Fold(first, last) => drop(e.fold_lines(*first, *last)),
        Op::Unfold(line) => drop(e.unfold_line(*line)),
        Op::UnfoldAll => drop(e.unfold_all()),
    }
}

/// The carets are sorted, apart from each other, inside the text and on character boundaries.
fn assert_valid(e: &Editor) -> Result<(), TestCaseError> {
    let len = e.buffer().len();
    let carets = e.carets();
    prop_assert!(!carets.is_empty());
    for c in carets {
        prop_assert!(c.sel.anchor <= len && c.sel.head <= len, "{:?} in {} bytes", c, len);
        prop_assert!(e.buffer().is_boundary(c.sel.anchor) && e.buffer().is_boundary(c.sel.head), "{:?}", c);
    }
    // No caret is in a hidden line, and the folds are stretches of whole lines that the editor's rows agree with.
    for c in carets {
        prop_assert!(!e.is_line_hidden(e.buffer().line_of(c.sel.head)), "{:?} is in a fold", c);
    }
    let buffer = e.buffer();
    for fold in e.folds() {
        let end_of = |offset: usize| buffer.line_range(buffer.line_of(offset)).map(|r| r.end);
        prop_assert!(fold.hidden.start < fold.hidden.end && fold.hidden.end <= buffer.len(), "{:?}", fold);
        prop_assert!(end_of(fold.hidden.start) == Some(fold.hidden.start) && end_of(fold.hidden.end) == Some(fold.hidden.end), "{:?}", fold);
    }
    prop_assert_eq!(e.visible_rows(), if e.folds().is_empty() { buffer.line_count() } else { FoldMap::new(buffer, e.folds()).rows() });
    for pair in carets.windows(2) {
        prop_assert!(
            pair[0].end() < pair[1].start() || (pair[0].end() == pair[1].start() && !pair[0].is_empty() && !pair[1].is_empty()),
            "{:?}",
            pair
        );
    }
    Ok(())
}

proptest! {
    /// Typing at several carets: the string rebuilt by hand, left to right.
    #[test]
    fn typing_matches_a_hand_rebuilt_string(initial in ascii_text(), offsets in proptest::collection::vec((0usize..60, 0usize..4), 1..5), typed in "[a-z\n]{1,3}") {
        let mut e = Editor::new(&initial);
        let selections: Vec<Selection> = offsets.iter().map(|(o, len)| {
            let start = (*o).min(initial.len());
            Selection { anchor: start, head: (start + len).min(initial.len()) }
        }).collect();
        e.set_selections(&selections);
        let carets: Vec<(usize, usize)> = e.carets().iter().map(|c| (c.start(), c.end())).collect();
        e.type_text(&typed);
        let mut want = String::new();
        let mut want_carets = Vec::new();
        let mut last = 0;
        for (start, end) in &carets {
            want.push_str(&initial[last..*start]);
            want.push_str(&typed);
            want_carets.push(want.len());
            last = *end;
        }
        want.push_str(&initial[last..]);
        prop_assert_eq!(e.text(), want);
        prop_assert_eq!(e.carets().iter().map(|c| c.head()).collect::<Vec<_>>(), want_carets);
    }

    /// Backspace and Delete at several carets: each deletes its selection, or the character before or after it; overlapping
    /// deletions join.
    #[test]
    fn deleting_matches_a_hand_rebuilt_string(initial in ascii_text(), offsets in proptest::collection::vec((0usize..60, 0usize..3), 1..5), forward in any::<bool>()) {
        let mut e = Editor::new(&initial);
        let selections: Vec<Selection> = offsets.iter().map(|(o, len)| {
            let start = (*o).min(initial.len());
            Selection { anchor: start, head: (start + len).min(initial.len()) }
        }).collect();
        e.set_selections(&selections);
        let carets: Vec<(usize, usize)> = e.carets().iter().map(|c| (c.start(), c.end())).collect();
        if forward { e.delete_forward(); } else { e.backspace(); }
        // The ranges to remove, joined where they overlap.
        let mut ranges: Vec<(usize, usize)> = carets.iter().filter_map(|&(s, en)| {
            if en > s { Some((s, en)) } else if forward { (s < initial.len()).then_some((s, s + 1)) } else { (s > 0).then(|| (s - 1, s)) }
        }).collect();
        ranges.sort();
        let mut joined: Vec<(usize, usize)> = Vec::new();
        for r in ranges {
            match joined.last_mut() {
                Some(last) if r.0 < last.1 => last.1 = last.1.max(r.1),
                _ => joined.push(r),
            }
        }
        let mut want = String::new();
        let mut last = 0;
        for (s, en) in &joined {
            want.push_str(&initial[last..*s]);
            last = *en;
        }
        want.push_str(&initial[last..]);
        prop_assert_eq!(e.text(), want);
    }

    /// Whatever is done, the carets stay valid; and undoing everything gives back the starting text, redoing everything the final.
    #[test]
    fn any_commands_keep_the_carets_valid_and_undo_redo_consistent(initial in text(), ops in proptest::collection::vec(op(), 0..25)) {
        let mut e = Editor::new(&initial);
        let mut time = 0.0;
        for op in &ops {
            time += 2.0;
            e.set_time(time);
            apply(&mut e, op);
            assert_valid(&e)?;
        }
        let end_text = e.text();
        let mut guard = 0;
        while e.undo() {
            assert_valid(&e)?;
            guard += 1;
            prop_assert!(guard < 1000);
        }
        prop_assert_eq!(e.text(), initial.clone());
        let mut guard = 0;
        while e.redo() {
            assert_valid(&e)?;
            guard += 1;
            prop_assert!(guard < 1000);
        }
        prop_assert_eq!(e.text(), end_text);
    }

    /// Cut then paste at the same place gives the text back (with whole-line cuts only when there was no selection).
    #[test]
    fn copy_paste_round_trips_a_selection(initial in text(), a in 0usize..60, b in 0usize..60) {
        let mut e = Editor::new(&initial);
        let (a, b) = (a.min(initial.len()), b.min(initial.len()));
        e.set_selection(a, b);
        let selected_before = e.carets()[0].sel;
        prop_assume!(!selected_before.is_caret());
        let copied = e.copy_text();
        let cut = e.cut();
        prop_assert_eq!(&cut, &copied);
        e.paste(&cut);
        // The pasted text is the copied text with line breaks written as the document's: equal to the original unless it held `\r\n`
        // (the document uses `\n`).
        if !initial.contains('\r') {
            prop_assert_eq!(e.text(), initial);
        }
    }
}
