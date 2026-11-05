//! The analysis of a JSON document: its problems, its outline and what can be folded, from one read of the text.

use std::ops::Range;

use xmlcraft_json_syntax::{Data, Options, Parse, Value, parse, parse_lines};

use crate::analysis::{Analysis, MAX_OUTLINE_ITEMS, OutlineItem};
use crate::folding::{FoldKind, FoldRange, Lines, MAX_FOLD_RANGES};
use crate::language::JsonFlavor;

/// The longest preview of a scalar value shown beside a name in the outline.
const MAX_LABEL: usize = 40;

pub(crate) fn analyze_json(text: &str, version: u64, flavor: JsonFlavor) -> Analysis {
    let options = Options { dialect: flavor.dialect(), ..Options::default() };
    let lines = Lines::new(text);
    let mut out = Walk { text, lines: &lines, outline: Vec::new(), outline_truncated: false, folds: Vec::new(), folds_truncated: false };
    let mut diagnostics = Vec::new();
    if flavor == JsonFlavor::Lines {
        for (range, read) in parse_lines(text, &options) {
            let Parse { root, diagnostics: found } = read;
            diagnostics.extend(found);
            if let Some(root) = root {
                // Each line is an item of its own, named by its number, with the members of its value under it.
                let number = lines.line_of(range.start) + 1;
                out.push_item(OutlineItem {
                    depth: 0,
                    name: format!("#{number}"),
                    label: label_of(&root),
                    range: root.span.clone(),
                    name_range: range.start..range.start,
                    start_tag: root.span.start..root.span.start.saturating_add(1).min(root.span.end),
                    end_tag: None,
                    end_name_range: None,
                });
                out.value(&root, 1, 1);
            }
        }
    } else {
        let Parse { root, diagnostics: found } = parse(text, &options);
        diagnostics = found;
        if let Some(root) = root {
            out.value(&root, 0, 1);
        }
    }
    diagnostics.sort_by_key(|d| (d.span.start, d.span.end));
    Analysis {
        version,
        diagnostics,
        outline: out.outline,
        outline_truncated: out.outline_truncated,
        folds: out.folds,
        folds_truncated: out.folds_truncated,
        doctype: None,
        doctype_ids: None,
        schema_hints: Vec::new(),
        root_namespace: None,
        skipped: None,
    }
}

/// A short look at a scalar value; none for an object or an array.
fn label_of(value: &Value) -> Option<String> {
    let shown = match &value.data {
        Data::String(s) => {
            let one_line: String = s.chars().filter(|c| !c.is_control()).collect();
            let mut shown: String = one_line.chars().take(MAX_LABEL).collect();
            if one_line.chars().count() > MAX_LABEL {
                shown.push('…');
            }
            format!("\"{shown}\"")
        }
        Data::Number(n) => n.chars().take(MAX_LABEL).collect(),
        Data::Bool(b) => b.to_string(),
        Data::Null => "null".to_owned(),
        Data::Array(_) | Data::Object(_) => return None,
    };
    Some(shown)
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

    /// The members (or items) of `value` as outline items at `depth`, and the folds of it and of what is in it. `level` counts the
    /// brackets around, 1 for the outermost.
    fn value(&mut self, value: &Value, depth: usize, level: usize) {
        self.fold(value, level);
        match &value.data {
            Data::Object(members) => {
                for m in members {
                    // (An object that is cut off by a limit is shown as far as it was read.)
                    self.push_item(self.item(m.key.clone(), m.key_span.clone(), &m.value, depth));
                    self.value(&m.value, depth + 1, level + 1);
                }
            }
            Data::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    // The items of an array of scalars would drown the outline; the containers and their places are enough.
                    if matches!(v.data, Data::Object(_) | Data::Array(_)) {
                        let at = v.span.start..v.span.start;
                        self.push_item(self.item(format!("[{i}]"), at, v, depth));
                    }
                    self.value(v, depth + 1, level + 1);
                }
            }
            _ => {}
        }
    }

    fn item(&self, name: String, name_range: Range<usize>, value: &Value, depth: usize) -> OutlineItem {
        let container = matches!(value.data, Data::Object(_) | Data::Array(_));
        let (start_tag, end_tag) = if container {
            let open = value.span.start..(value.span.start + 1).min(value.span.end);
            // (A container that was not closed has no closing bracket.)
            let closed = self.text.get(value.span.clone()).is_some_and(|t| t.ends_with(['}', ']'])) && value.span.len() > 1;
            (open, closed.then(|| value.span.end - 1..value.span.end))
        } else {
            (value.span.clone(), None)
        };
        let range = if container { value.span.clone() } else { name_range.start.min(value.span.start)..value.span.end.max(name_range.end) };
        OutlineItem { depth, name, label: label_of(value), range, name_range, start_tag, end_tag, end_name_range: None }
    }

    /// The fold of an object or array that goes over lines. The closing bracket stays visible when it has a line of its own.
    fn fold(&mut self, value: &Value, level: usize) {
        let kind = match value.data {
            Data::Object(_) => FoldKind::Object,
            Data::Array(_) => FoldKind::Array,
            _ => return,
        };
        let Some(text) = self.text.get(value.span.clone()) else { return };
        // A container that is not closed cannot be folded to its end.
        if !text.ends_with(['}', ']']) || text.len() < 2 {
            return;
        }
        let (first, close) = (self.lines.line_of(value.span.start), self.lines.line_of(value.span.end - 1));
        if close <= first {
            return;
        }
        let Some(line) = self.lines.text_of(close) else { return };
        let before_blank = self.lines.is_blank(line.start, value.span.end - 1).unwrap_or(false);
        // With something before the bracket on its line, that line is hidden with the content, and what follows it (a comma, say)
        // must be nothing else.
        let after = self.text.get(value.span.end.min(line.end)..line.end).unwrap_or_default().trim();
        let last = if before_blank {
            close - 1
        } else if after.is_empty() || after == "," {
            close
        } else {
            return;
        };
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
        self.folds.push(FoldRange { first_line: first, last_line: last, end_line: close, level, kind });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze_as;
    use crate::language::Language;

    fn analysis(text: &str, flavor: JsonFlavor) -> Analysis {
        analyze_as(Language::Json(flavor), text, 7)
    }

    #[test]
    fn the_outline_lists_members_and_containers_at_their_depth() {
        let text = "{\n  \"name\": \"x\",\n  \"tags\": [\"a\", \"b\"],\n  \"items\": [\n    {\"id\": 1},\n    [2]\n  ]\n}";
        let a = analysis(text, JsonFlavor::Json);
        assert!(a.diagnostics.is_empty());
        let shown: Vec<_> = a.outline.iter().map(|i| (i.depth, i.name.as_str(), i.label.as_deref())).collect();
        assert_eq!(
            shown,
            [(0, "name", Some("\"x\"")), (0, "tags", None), (0, "items", None), (1, "[0]", None), (2, "id", Some("1")), (1, "[1]", None)],
            "scalar array items are left out"
        );
        let items = a.outline.iter().find(|i| i.name == "items").expect("items");
        assert_eq!(&text[items.name_range.clone()], "\"items\"");
        assert_eq!(&text[items.start_tag.clone()], "[");
        assert_eq!(items.end_tag.as_ref().map(|r| &text[r.clone()]), Some("]"));
        assert_eq!(&text[items.range.clone()], "[\n    {\"id\": 1},\n    [2]\n  ]");
        let name = a.outline.iter().find(|i| i.name == "name").expect("name");
        assert_eq!(&text[name.range.clone()], "\"name\": \"x\"");
        // The bracket pairs are not tags: the caret next to one finds no partner to jump to or rename.
        assert_eq!(a.matching_tag(items.start_tag.start), None);
        assert_eq!(a.linked_names(items.name_range.start + 1), None);
    }

    #[test]
    fn folds_hide_the_lines_between_the_brackets_and_leave_the_closing_bracket_visible() {
        let text = "{\n  \"a\": {\n    \"b\": 1\n  },\n  \"c\": [1,\n    2],\n  \"d\": {},\n  \"e\": {\"x\": 1}\n}";
        let a = analysis(text, JsonFlavor::Json);
        let folds: Vec<_> = a.folds.iter().map(|f| (f.first_line, f.last_line, f.end_line, f.level, f.kind)).collect();
        assert_eq!(
            folds,
            [
                (0, 7, 8, 1, FoldKind::Object),
                (1, 2, 3, 2, FoldKind::Object),
                // The closing bracket shares its line with content: that line is hidden with the rest.
                (4, 5, 5, 2, FoldKind::Array),
            ]
        );
        // Two containers that open on one line have one mark, on the outer one.
        let a = analysis("[[\n1,\n2\n]]", JsonFlavor::Json);
        assert_eq!(
            a.folds.iter().map(|f| (f.first_line, f.last_line, f.level)).collect::<Vec<_>>(),
            [(0, 3, 1)],
            "the brackets that close both stand on one line, hidden with the content"
        );
        // A container that is not closed cannot be folded.
        assert!(analysis("{\n\"a\": 1,\n", JsonFlavor::Json).folds.is_empty());
        // Something other than a comma after the bracket on a shared line: nothing to fold.
        assert!(analysis("[{\"a\":\n1}  /* c */, 2]", JsonFlavor::Jsonc).folds.iter().all(|f| f.level != 2));
    }

    #[test]
    fn the_problems_are_those_of_the_dialect_in_the_order_of_the_text() {
        let text = "{\"a\": 1, // note\n \"b\": [1,],}";
        let strict: Vec<_> = analysis(text, JsonFlavor::Json).diagnostics.iter().map(|d| d.code.key()).collect();
        assert_eq!(strict, ["json.comment-not-allowed", "json.trailing-comma", "json.trailing-comma"]);
        assert!(analysis(text, JsonFlavor::Jsonc).diagnostics.is_empty());
        let broken = analysis("{\"a\": [1, 2", JsonFlavor::Json);
        assert!(broken.diagnostics.iter().all(|d| d.code.key().starts_with("json.")) && !broken.diagnostics.is_empty());
        assert!(!broken.is_well_formed());
        assert_eq!(broken.version, 7);
        assert!(analysis("", JsonFlavor::Json).diagnostics.iter().any(|d| d.code.key() == "json.empty-document"));
    }

    #[test]
    fn json_lines_have_an_item_per_line_and_problems_at_the_right_line() {
        let text = "{\"a\": 1}\n\n{\"b\": {\"c\": 2}}\n[1,\n";
        let a = analysis(text, JsonFlavor::Lines);
        let shown: Vec<_> = a.outline.iter().map(|i| (i.depth, i.name.as_str())).collect();
        assert_eq!(shown, [(0, "#1"), (1, "a"), (0, "#3"), (1, "b"), (2, "c"), (0, "#4")], "a broken line is still an item");
        assert_eq!(a.diagnostics.len(), 1);
        assert_eq!(&text[a.diagnostics[0].span.clone()], "[", "the problem is on line 4");
    }

    #[test]
    fn nothing_here_panics_and_the_lists_are_bounded() {
        for text in ["", "{", "[[[[", "\"\u{1F600}", "{\"a\":", "}{", "\u{FEFF}{}", "{\"a\": 1}}}}"] {
            for flavor in [JsonFlavor::Json, JsonFlavor::Jsonc, JsonFlavor::Json5, JsonFlavor::Lines] {
                let a = analysis(text, flavor);
                for item in &a.outline {
                    assert!(text.get(item.range.clone()).is_some() && text.get(item.name_range.clone()).is_some(), "{text:?}");
                }
            }
        }
        let big = format!("[{}]", "{\"a\":1},".repeat(MAX_OUTLINE_ITEMS));
        let a = analysis(&big, JsonFlavor::Json);
        assert!(a.outline_truncated && a.outline.len() == MAX_OUTLINE_ITEMS);
    }
}
