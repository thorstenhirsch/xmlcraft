//! The analysis of a JSON document: its problems, its outline and what can be folded, from one read of the text.

use std::ops::Range;

use xmlcraft_json_syntax::{Code, Data, Diagnostic, Options, Parse, Value, parse, parse_lines};

use crate::analysis::{Analysis, MAX_OUTLINE_ITEMS, OutlineItem};
use crate::folding::{FoldKind, FoldRange, Lines, MAX_FOLD_RANGES};
use crate::language::{JsonFlavor, Language};

/// The longest preview of a scalar value shown beside a name in the outline.
const MAX_LABEL: usize = 40;

pub(crate) fn analyze_json(text: &str, version: u64, flavor: JsonFlavor) -> Analysis {
    analyze_json_as(text, version, flavor, Language::Json(flavor))
}

/// The text of a BSON file: read as the JSON it is laid out as, and as Extended JSON besides (the values that are not valid Extended
/// JSON are problems too).
pub(crate) fn analyze_bson(text: &str, version: u64) -> Analysis {
    let flavor = match xmlcraft_bson::layout_of_text(text) {
        xmlcraft_bson::Layout::Lines => JsonFlavor::Lines,
        xmlcraft_bson::Layout::Single => JsonFlavor::Json,
    };
    let mut analysis = analyze_json_as(text, version, flavor, Language::Bson);
    // A file of no documents is a valid BSON file (an empty list), not an empty JSON text.
    if text.trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}').is_empty() {
        analysis.diagnostics.retain(|d| d.code.key() != "json.empty-document");
    }
    // Only a text that is JSON can be asked what it means.
    if analysis.diagnostics.iter().all(|d| d.severity() != xmlcraft_json_syntax::Severity::Error)
        && let Err(errors) = xmlcraft_bson::from_text(text)
    {
        for e in errors.into_iter().take(1_000) {
            let end = (e.offset + e.length.max(1)).min(text.len());
            analysis.diagnostics.push(Diagnostic::new(Code::BsonInvalid, e.offset.min(end)..end, vec![e.message]));
        }
        analysis.diagnostics.sort_by_key(|d| (d.span.start, d.span.end));
    }
    analysis
}

fn analyze_json_as(text: &str, version: u64, flavor: JsonFlavor, language: Language) -> Analysis {
    let options = Options { dialect: flavor.dialect(), ..Options::default() };
    let lines = Lines::new(text);
    let mut out = Walk {
        text,
        lines: &lines,
        outline: Vec::new(),
        outline_truncated: false,
        folds: Vec::new(),
        folds_truncated: false,
        extended: language == Language::Bson,
    };
    let mut diagnostics = Vec::new();
    let mut capped = false;
    if flavor == JsonFlavor::Lines {
        for (range, read) in parse_lines(text, &options) {
            let Parse { root, diagnostics: found } = read;
            // The cap is for the whole text, not for each line.
            let room = options.max_diagnostics.saturating_sub(diagnostics.len());
            capped |= found.len() > room;
            diagnostics.extend(found.into_iter().take(room));
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
    if capped {
        diagnostics.push(Diagnostic::new(Code::TooManyDiagnostics, text.len()..text.len(), vec![]));
    }
    Analysis {
        version,
        language,
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
pub(crate) fn label_of(value: &Value) -> Option<String> {
    let shown = match &value.data {
        Data::String(s) => {
            // (Line breaks at the ends are not part of what is shown; the spaces of the text are.)
            let one_line: String = s.trim_matches(['\n', '\r']).chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
            let mut shown: String = one_line.chars().take(MAX_LABEL).collect();
            if one_line.chars().count() > MAX_LABEL {
                shown.push('…');
            }
            format!("\"{shown}\"")
        }
        Data::Number(n) => {
            let mut shown: String = n.chars().take(MAX_LABEL).collect();
            if n.chars().count() > MAX_LABEL {
                shown.push('…');
            }
            shown
        }
        Data::Bool(b) => b.to_string(),
        Data::Null => "null".to_owned(),
        Data::Array(_) | Data::Object(_) => return None,
    };
    Some(shown)
}

/// The bracket that closes a container (`}` for an object, `]` for an array): one of the other kind closes nothing.
fn closer(value: &Value) -> char {
    if matches!(value.data, Data::Object(_)) { '}' } else { ']' }
}

struct Walk<'a> {
    text: &'a str,
    lines: &'a Lines<'a>,
    outline: Vec<OutlineItem>,
    outline_truncated: bool,
    folds: Vec<FoldRange>,
    folds_truncated: bool,
    /// The text is Extended JSON: an object like `{"$oid": …}` is one typed value, not a container.
    extended: bool,
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
        if self.typed(value).is_some() {
            return;
        }
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

    /// In Extended JSON, the look at the value of an object that stands for one typed value (`7 ($numberInt)`).
    fn typed(&self, value: &Value) -> Option<String> {
        if !self.extended {
            return None;
        }
        let Data::Object(members) = &value.data else { return None };
        // (`$code` may come after `$scope`, as in the conversion.)
        let first = members.iter().find(|m| m.key == "$code").or_else(|| members.first().filter(|m| xmlcraft_bson::is_wrapper_name(&m.key)))?;
        let inner = match &first.value.data {
            Data::Object(inner) => inner.first().and_then(|m| label_of(&m.value)),
            _ => label_of(&first.value),
        };
        Some(format!("{} ({})", inner.unwrap_or_default().trim_matches('"'), first.key))
    }

    fn item(&self, name: String, name_range: Range<usize>, value: &Value, depth: usize) -> OutlineItem {
        let typed = self.typed(value);
        let container = typed.is_none() && matches!(value.data, Data::Object(_) | Data::Array(_));
        let (start_tag, end_tag) = if container {
            let open = value.span.start..(value.span.start + 1).min(value.span.end);
            // (A container that was not closed has no closing bracket.)
            let closed = self.text.get(value.span.clone()).is_some_and(|t| t.len() > 1 && t.ends_with(closer(value)));
            (open, closed.then(|| value.span.end - 1..value.span.end))
        } else {
            (value.span.clone(), None)
        };
        let range = if container { value.span.clone() } else { name_range.start.min(value.span.start)..value.span.end.max(name_range.end) };
        OutlineItem { depth, name, label: typed.or_else(|| label_of(value)), range, name_range, start_tag, end_tag, end_name_range: None }
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
        if text.len() < 2 || !text.ends_with(closer(value)) {
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

    #[test]
    fn a_bracket_of_the_other_kind_closes_nothing_in_the_outline_or_the_folds() {
        for text in ["{\n\"x\": {\n\"b\": 1\n]\n}", "[\n[1,\n2\n}\n]", "{\n\"a\": [\n1\n}\n"] {
            let a = analysis(text, JsonFlavor::Json);
            for item in &a.outline {
                let open = &text[item.start_tag.clone()];
                if let Some(end) = &item.end_tag {
                    assert_eq!((open, &text[end.clone()]), if open == "{" { ("{", "}") } else { ("[", "]") }, "{text:?}");
                }
            }
            for fold in &a.folds {
                // A fold stops at a bracket that matches: the line it ends on has one of its kind.
                let end_line = text.lines().nth(fold.end_line).unwrap_or_default();
                let start_line = text.lines().nth(fold.first_line).unwrap_or_default();
                let wants = if start_line.contains('{') && !start_line.contains('[') { '}' } else { ']' };
                assert!(end_line.contains(wants), "{text:?}: {fold:?}");
            }
        }
    }

    #[test]
    fn json_lines_report_no_more_problems_than_the_cap_for_the_whole_text() {
        let text = "{\"a\" 1}\n".repeat(5000);
        let a = analysis(&text, JsonFlavor::Lines);
        let max = Options::default().max_diagnostics;
        assert_eq!(a.diagnostics.len(), max + 1, "the cap and the one that says there are more");
        assert_eq!(a.diagnostics.last().map(|d| d.code), Some(Code::TooManyDiagnostics));
    }

    #[test]
    fn a_long_number_is_shown_with_an_ellipsis_like_a_long_string() {
        let digits = "1".repeat(100);
        let a = analysis(&format!("{{\"n\": {digits}, \"s\": \"{digits}\"}}"), JsonFlavor::Json);
        let labels: Vec<_> = a.outline.iter().filter_map(|i| i.label.clone()).collect();
        assert!(labels.iter().all(|l| l.ends_with('…') || l.ends_with("…\"")), "{labels:?}");
        assert_eq!(a.language, Language::Json(JsonFlavor::Json));
    }

    #[test]
    fn bson_text_is_checked_as_extended_json_too() {
        let a = analyze_as(Language::Bson, "{\"id\": {\"$oid\": \"zz\"}, \"n\": 1}\n", 2);
        assert_eq!(a.language, Language::Bson);
        let codes: Vec<_> = a.diagnostics.iter().map(|d| d.code.key()).collect();
        assert_eq!(codes, ["bson.invalid-value"]);
        assert_eq!(&"{\"id\": {\"$oid\": \"zz\"}, \"n\": 1}\n"[a.diagnostics[0].span.clone()], "\"zz\"");
        // A syntax error is the JSON reader's, and Extended JSON is not asked about a text that is not JSON.
        let a = analyze_as(Language::Bson, "{\"id\": {\"$oid\": ", 2);
        assert!(a.diagnostics.iter().all(|d| d.code.key().starts_with("json.")), "{:?}", a.diagnostics);
        // Documents one to a line have an item each; one document is a tree.
        let lines = analyze_as(Language::Bson, "{\"a\": 1}\n{\"b\": 2}\n", 2);
        assert_eq!(lines.outline.iter().filter(|i| i.depth == 0).count(), 2);
        assert!(analyze_as(Language::Bson, "{\"a\": 1}\n", 1).is_well_formed());
        // A typed value is one item with a look at it; its wrapper is not a container to fold or list.
        let typed = analyze_as(
            Language::Bson,
            "{\n  \"id\": {\"$oid\": \"0123456789abcdef01234567\"},\n  \"o\": {\n    \"d\": {\"$date\": {\"$numberLong\": \"5\"}}\n  }\n}\n",
            1,
        );
        let shown: Vec<_> = typed.outline.iter().map(|i| (i.depth, i.name.as_str(), i.label.as_deref())).collect();
        assert_eq!(shown, [(0, "id", Some("0123456789abcdef01234567 ($oid)")), (0, "o", None), (1, "d", Some("5 ($date)"))]);
        // A file of no documents is valid BSON, not an empty JSON text.
        assert!(analyze_as(Language::Bson, "", 1).diagnostics.is_empty());
        assert!(analyze_as(Language::Bson, " \n", 1).diagnostics.is_empty());
        assert!(!analyze_as(Language::Json(JsonFlavor::Json), "", 1).diagnostics.is_empty(), "while an empty JSON file is empty");
        // JavaScript with a scope is one typed value, whichever member comes first.
        let code = analyze_as(Language::Bson, "{\"f\": {\"$scope\": {\"x\": {\"$numberInt\": \"1\"}}, \"$code\": \"go()\"}}\n", 1);
        let shown: Vec<_> = code.outline.iter().map(|i| (i.name.as_str(), i.label.as_deref())).collect();
        assert_eq!(shown, [("f", Some("go() ($code)"))]);
    }
}
