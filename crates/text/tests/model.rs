//! The buffer and its history against a plain `String`: whatever edits are thrown at them, they agree, and undo/redo walk back and
//! forth through exactly the texts that were there.

use proptest::prelude::*;
use xmlcraft_text::{Buffer, Edit, EditKind, History, Selection};

/// An edit written as fractions of the text, so it is valid or deliberately not, whatever the text is.
#[derive(Clone, Debug)]
struct Raw {
    start: usize,
    len: usize,
    text: String,
}

fn raw() -> impl Strategy<Value = Raw> {
    (0usize..400, 0usize..40, "[a-zé日😀\n\r <>/]{0,12}").prop_map(|(start, len, text)| Raw { start, len, text })
}

/// The nearest char boundaries at or before `at`.
fn floor(s: &str, mut at: usize) -> usize {
    at = at.min(s.len());
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn to_edit(model: &str, raw: &Raw) -> Edit {
    let start = floor(model, raw.start);
    let end = floor(model, start + raw.len);
    Edit::replace(start..end, raw.text.clone())
}

proptest! {
    #[test]
    fn the_buffer_agrees_with_a_string(initial in "[a-zé日\n\r ]{0,60}", raws in proptest::collection::vec(raw(), 0..30)) {
        let mut buf = Buffer::from_text(&initial);
        let mut model = initial.clone();
        for r in &raws {
            let edit = to_edit(&model, r);
            let inverse = buf.apply(&edit).expect("a valid edit");
            model.replace_range(edit.range.clone(), &edit.text);
            prop_assert!(buf.eq_str(&model));
            prop_assert_eq!(buf.len(), model.len());
            prop_assert_eq!(buf.text(), model.clone());
            // The line index agrees with the string.
            let lines = model.split('\n').count();
            prop_assert_eq!(buf.line_count(), lines);
            // Every line's text, without its break, is what the model says. A `\r` is part of the break only before a `\n`, so the
            // last line (which has no `\n` after it) keeps a trailing `\r`.
            for (i, line) in model.split('\n').enumerate() {
                let want = if i + 1 < lines { line.strip_suffix('\r').unwrap_or(line) } else { line };
                let got = buf.line_text(i);
                prop_assert_eq!(got.as_deref(), Some(want));
            }
            // The inverse restores the text exactly.
            let mut back = buf.clone();
            back.apply(&inverse).expect("the inverse is valid");
            let mut before = model.clone();
            before.replace_range(inverse.range.clone(), &inverse.text);
            prop_assert_eq!(back.text(), before);
        }
    }

    #[test]
    fn invalid_edits_are_refused_and_change_nothing(initial in "[a-zé日]{0,30}", start in 0usize..80, end in 0usize..80) {
        let mut buf = Buffer::from_text(&initial);
        let edit = Edit::replace(start..end, "X");
        let valid = start <= end && end <= initial.len() && initial.is_char_boundary(start) && initial.is_char_boundary(end);
        let result = buf.apply(&edit);
        prop_assert_eq!(result.is_ok(), valid);
        if !valid {
            prop_assert!(buf.eq_str(&initial));
        }
    }

    #[test]
    fn undo_and_redo_walk_through_every_text(
        initial in "[a-zé日\n ]{0,40}",
        steps in proptest::collection::vec((raw(), 0u8..3), 1..20),
    ) {
        let mut buf = Buffer::from_text(&initial);
        let mut history = History::with_limit(1 << 30);
        let mut texts = vec![initial.clone()];
        let mut model = initial;
        let mut time = 0.0;
        for (r, kind) in &steps {
            let edit = to_edit(&model, r);
            let kind = [EditKind::Typing, EditKind::Delete, EditKind::Other][usize::from(*kind)];
            let caret = edit.range.start;
            let inverse = buf.apply_all(std::slice::from_ref(&edit)).expect("valid");
            model.replace_range(edit.range.clone(), &edit.text);
            let after = caret + edit.text.len();
            history.record(vec![edit], inverse, vec![Selection::caret(caret)], vec![Selection::caret(after)], kind, time);
            time += 0.1;
            if texts.last() != Some(&model) {
                texts.push(model.clone());
            }
        }
        let end_text = buf.text();
        // Undo all the way: the buffer ends at the initial text, passing only through texts that existed.
        let mut seen = vec![buf.text()];
        while history.can_undo() {
            prop_assert!(history.undo(&mut buf).expect("consistent").is_some());
            seen.push(buf.text());
        }
        prop_assert_eq!(&buf.text(), &texts[0]);
        for t in &seen {
            prop_assert!(texts.contains(t), "undo produced a text that never existed: {:?}", t);
        }
        // Redo all the way: back at the end.
        while history.can_redo() {
            prop_assert!(history.redo(&mut buf).expect("consistent").is_some());
        }
        prop_assert_eq!(buf.text(), end_text);
    }
}
