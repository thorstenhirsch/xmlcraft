//! The analysis of a YAML document: its problems, its outline and what can be folded, from one read of the text.

use xmlcraft_yaml_syntax::{Code, Data, Diagnostic, Options, Value, parse};

use crate::analysis::{Analysis, MAX_OUTLINE_ITEMS, OutlineItem, count_children};
use crate::folding::{FoldKind, FoldRange, Lines, MAX_FOLD_RANGES};
use crate::json_analysis::{kind_of, label_of};
use crate::language::Language;

pub(crate) fn analyze_yaml(text: &str, version: u64) -> Analysis {
    let options = Options::default();
    let read = parse(text, &options);
    let lines = Lines::new(text);
    let mut walk = Walk { text, lines: &lines, outline: Vec::new(), outline_truncated: false, folds: Vec::new(), folds_truncated: false };
    // A stream of several documents has an item for each; a single one has its members at the top.
    let several = read.documents.len() > 1;
    for (i, document) in read.documents.iter().enumerate() {
        if several {
            walk.push_item(OutlineItem {
                depth: 0,
                name: format!("document {}", i + 1),
                label: None,
                range: document.span.clone(),
                name_range: document.span.start..document.span.start,
                start_tag: document.span.start..document.span.start,
                end_tag: None,
                end_name_range: None,
                kind: kind_of(&document.data),
                children: 0,
                preview: label_of(document).unwrap_or_default(),
            });
        }
        walk.value(document, usize::from(several), 1);
    }
    count_children(&mut walk.outline);
    let mut diagnostics = read.diagnostics;
    let capped = diagnostics.iter().position(|d| d.code == Code::TooManyDiagnostics);
    let last = capped.map(|at| diagnostics.remove(at));
    diagnostics.sort_by_key(|d| (d.span.start, d.span.end));
    diagnostics.extend(last.map(|d| Diagnostic::new(Code::TooManyDiagnostics, text.len()..text.len(), d.args)));
    Analysis {
        version,
        language: Language::Yaml,
        diagnostics,
        outline: walk.outline,
        outline_truncated: walk.outline_truncated,
        folds: walk.folds,
        folds_truncated: walk.folds_truncated,
        doctype: None,
        doctype_ids: None,
        schema_hints: Vec::new(),
        root_namespace: None,
        skipped: None,
    }
}

struct Walk<'a> {
    text: &'a str,
    lines: &'a Lines<'a>,
    outline: Vec<OutlineItem>,
    outline_truncated: bool,
    folds: Vec<FoldRange>,
    folds_truncated: bool,
}

impl Walk<'_> {
    fn push_item(&mut self, item: OutlineItem) {
        if self.outline.len() >= MAX_OUTLINE_ITEMS {
            self.outline_truncated = true;
        } else {
            self.outline.push(item);
        }
    }

    /// The end of what a value stands for in the text, without the white space after it (a block value runs to the next line).
    fn content_end(&self, value: &Value) -> usize {
        let text = self.text.get(value.span.clone()).unwrap_or_default();
        value.span.start + text.trim_end().len()
    }

    fn value(&mut self, value: &Value, depth: usize, level: usize) {
        match &value.data {
            Data::Object(members) => {
                for m in members {
                    let end = self.content_end(&m.value).max(m.key_span.end);
                    let container = matches!(m.value.data, Data::Object(_) | Data::Array(_));
                    let label = label_of(&m.value);
                    self.push_item(OutlineItem {
                        depth,
                        name: m.key.clone(),
                        preview: label.clone().unwrap_or_default(),
                        label,
                        range: m.key_span.start..end,
                        name_range: m.key_span.clone(),
                        start_tag: m.key_span.clone(),
                        end_tag: None,
                        end_name_range: None,
                        kind: kind_of(&m.value.data),
                        children: 0,
                    });
                    // The members of a block mapping are folded from their key; so are multi-line scalars.
                    self.fold(m.key_span.start, end, level, &m.value, container, false);
                    self.value(&m.value, depth + 1, level + 1);
                }
            }
            Data::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    let end = self.content_end(v);
                    // Only the entries that are containers are listed; the entries of a list of scalars would drown the outline.
                    let container = matches!(v.data, Data::Object(_) | Data::Array(_));
                    if container {
                        self.push_item(OutlineItem {
                            depth,
                            name: format!("[{i}]"),
                            label: None,
                            range: v.span.start..end.max(v.span.start),
                            name_range: v.span.start..v.span.start,
                            start_tag: v.span.start..v.span.start,
                            end_tag: None,
                            end_name_range: None,
                            kind: kind_of(&v.data),
                            children: 0,
                            preview: String::new(),
                        });
                    }
                    // (An entry that is a block scalar over several lines folds too.)
                    self.fold(v.span.start, end, level, v, container, true);
                    self.value(v, depth + 1, level + 1);
                }
            }
            _ => {}
        }
    }

    /// A fold from the line that `start` is on to the line the content ends on, when that is another line.
    fn fold(&mut self, start: usize, end: usize, level: usize, value: &Value, container: bool, value_start: bool) {
        let multi_line_scalar =
            matches!(&value.data, Data::String(_)) && self.text.get(value.span.clone()).is_some_and(|t| t.trim_end().contains('\n'));
        if !container && !multi_line_scalar {
            return;
        }
        let kind = if matches!(value.data, Data::Array(_)) { FoldKind::Array } else { FoldKind::Object };
        let mut first = self.lines.line_of(start);
        // A block scalar of a list entry starts on the line after its header (`- |`): the fold is of the entry.
        if multi_line_scalar
            && value_start
            && let Some(line) = self.lines.text_of(first)
            && self.text.get(line.start..start).is_some_and(|before| before.trim().is_empty())
        {
            first = first.saturating_sub(1);
        }
        let last = self.lines.line_of(end.max(start));
        if last <= first {
            return;
        }
        // One mark to click on a line: of folds that start on the same line only the outer one is kept.
        if self.folds.last().is_some_and(|f| f.first_line >= first) {
            return;
        }
        if self.folds.len() >= MAX_FOLD_RANGES {
            self.folds_truncated = true;
            return;
        }
        self.folds.push(FoldRange { first_line: first, last_line: last, end_line: last, level, kind });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn yaml_items_have_kinds_children_and_previews() {
        let a = analyze_as(Language::Yaml, "name: Ada\nage: 36\nlist:\n  - a: 1\n  - b\nnothing: ~\n", 1);
        let got: Vec<(&str, crate::analysis::OutlineKind, u32, &str)> =
            a.outline.iter().map(|i| (i.name.as_str(), i.kind, i.children, i.preview.as_str())).collect();
        use crate::analysis::OutlineKind as K;
        assert_eq!(
            got,
            [
                ("name", K::String, 0, "\"Ada\""),
                ("age", K::Number, 0, "36"),
                ("list", K::Array, 1, ""),
                ("[0]", K::Object, 1, ""),
                ("a", K::Number, 0, "1"),
                ("nothing", K::Null, 0, "null"),
            ]
        );
    }

    use super::*;
    use crate::analysis::analyze_as;

    fn analysis(text: &str) -> Analysis {
        analyze_as(Language::Yaml, text, 3)
    }

    #[test]
    fn the_outline_lists_keys_and_containers_and_the_folds_cover_their_lines() {
        let text = "name: x\nitems:\n  - id: 1\n    tags: [a, b]\n  - id: 2\nnote: |\n  one\n  two\nlast: 1\n";
        let a = analysis(text);
        assert!(a.diagnostics.is_empty(), "{:?}", a.diagnostics);
        assert_eq!(a.language, Language::Yaml);
        let shown: Vec<_> = a.outline.iter().map(|i| (i.depth, i.name.as_str(), i.label.as_deref())).collect();
        assert_eq!(
            shown,
            [
                (0, "name", Some("\"x\"")),
                (0, "items", None),
                (1, "[0]", None),
                (2, "id", Some("1")),
                (2, "tags", None),
                (1, "[1]", None),
                (2, "id", Some("2")),
                (0, "note", Some("\"one two\"")),
                (0, "last", Some("1")),
            ]
            .map(|(d, n, l)| (d, n, l))
        );
        let items = a.outline.iter().find(|i| i.name == "items").expect("items");
        assert_eq!(&text[items.name_range.clone()], "items");
        assert!(text[items.range.clone()].starts_with("items:") && text[items.range.clone()].ends_with("id: 2"), "{:?}", &text[items.range.clone()]);
        let folds: Vec<_> = a.folds.iter().map(|f| (f.first_line, f.last_line, f.kind)).collect();
        assert_eq!(folds, [(1, 4, FoldKind::Array), (2, 3, FoldKind::Object), (5, 7, FoldKind::Object)], "{folds:?}");
        // No bracket pairs to jump between.
        assert_eq!(a.matching_tag(items.name_range.start), None);
    }

    #[test]
    fn of_folds_that_start_on_one_line_the_outer_one_is_kept() {
        let a = analysis("- k:\n    x: 1\n    y: 2\n- z: 3\n");
        let folds: Vec<_> = a.folds.iter().map(|f| (f.first_line, f.last_line, f.level)).collect();
        assert_eq!(folds, [(0, 2, 1)], "the entry and its first key both start on line 0");
    }

    #[test]
    fn block_scalars_in_a_list_and_multi_line_ones_fold_and_the_label_keeps_its_spaces() {
        let a = analysis("- |\n  one\n  two\n- plain\nk: \" pad \"\n");
        let folds: Vec<_> = a.folds.iter().map(|f| (f.first_line, f.last_line)).collect();
        assert_eq!(folds, [(0, 2)], "{folds:?}");
        let a = analysis("k: \"  pad  \"\n");
        assert_eq!(a.outline[0].label.as_deref(), Some("\"  pad  \""));
    }

    #[test]
    fn problems_are_the_readers_and_a_stream_of_documents_has_an_item_each() {
        let a = analysis("a: [1, 2\nb: 3\n");
        assert!(a.diagnostics.iter().any(|d| d.code.key() == "yaml.syntax-error"));
        assert!(!a.is_well_formed());
        let a = analysis("a: 1\n---\nb: 2\n");
        let shown: Vec<_> = a.outline.iter().map(|i| (i.depth, i.name.as_str())).collect();
        assert_eq!(shown, [(0, "document 1"), (1, "a"), (0, "document 2"), (1, "b")]);
        assert!(analysis("").outline.is_empty() && analysis("").diagnostics.is_empty());
        let many = "a: 1\n".repeat(5000);
        let a = analysis(&many);
        assert_eq!(a.diagnostics.last().map(|d| d.code), Some(Code::TooManyDiagnostics));
    }

    #[test]
    fn nothing_here_panics() {
        for text in ["", ":", "- - - -", "a: &x [*x]", "? [a]\n: b", "\"open", "a:\n\t- x", "{a: 1,\n", "- !!str\n", "é: é\n"] {
            let a = analysis(text);
            for item in &a.outline {
                assert!(text.get(item.range.clone()).is_some() && text.get(item.name_range.clone()).is_some(), "{text:?}");
            }
        }
    }
}
