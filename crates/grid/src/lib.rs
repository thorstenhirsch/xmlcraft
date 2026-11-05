#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! The grid: a document of XML, JSON or YAML as a table of rows (name, type, value) that nest, and the edits that change a cell.
//!
//! - **Layer:** L4, no UI and no I/O, so it compiles for the web.
//! - [`Grid::read`] reads a text into a tree (the tree of `xmlcraft-diff`, which knows where every node is in the text). [`Grid::rows`]
//!   lists the rows that are visible given which rows are expanded.
//! - **The text is the model.** Nothing here keeps a second copy of the document: a change is a list of [`Edit`]s of the text, each as
//!   small as the cell (a value replaced, a name replaced, a member removed with its comma), made with the places the tree knows. Apply
//!   them to the buffer and read the text again.
//! - What a cell cannot be changed to (a number that is not a number, a name that is no name, a comment with `--` in it, a block scalar
//!   or flow collection of YAML) is an error that says why, and nothing is edited.

use std::collections::HashSet;
use std::ops::Range;

use xmlcraft_diff::{Family, Node, NodeKind};
use xmlcraft_text::Edit;

/// The most rows [`Grid::rows`] lists.
pub const MAX_ROWS: usize = 100_000;

/// A grid that cannot be made, or a change that cannot be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridError(pub String);

impl std::fmt::Display for GridError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GridError {}

fn fail<T>(message: impl Into<String>) -> Result<T, GridError> {
    Err(GridError(message.into()))
}

/// What a row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Document,
    Element,
    Attribute,
    Text,
    Comment,
    ProcessingInstruction,
    Object,
    Array,
    String,
    Number,
    Bool,
    Null,
}

impl RowKind {
    /// The word for the type column.
    pub fn label(self) -> &'static str {
        match self {
            RowKind::Document => "document",
            RowKind::Element => "element",
            RowKind::Attribute => "attribute",
            RowKind::Text => "text",
            RowKind::Comment => "comment",
            RowKind::ProcessingInstruction => "instruction",
            RowKind::Object => "object",
            RowKind::Array => "array",
            RowKind::String => "string",
            RowKind::Number => "number",
            RowKind::Bool => "boolean",
            RowKind::Null => "null",
        }
    }
}

fn kind_of(kind: NodeKind) -> RowKind {
    match kind {
        NodeKind::Document => RowKind::Document,
        NodeKind::Element => RowKind::Element,
        NodeKind::Attribute => RowKind::Attribute,
        NodeKind::Text => RowKind::Text,
        NodeKind::Comment => RowKind::Comment,
        NodeKind::ProcessingInstruction => RowKind::ProcessingInstruction,
        NodeKind::Object => RowKind::Object,
        NodeKind::Array => RowKind::Array,
        NodeKind::String => RowKind::String,
        NodeKind::Number => RowKind::Number,
        NodeKind::Bool => RowKind::Bool,
        NodeKind::Null => RowKind::Null,
    }
}

/// One row of the grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The way down from the root: the index of the child at each step. The root is the empty path.
    pub path: Vec<usize>,
    pub depth: usize,
    /// The name of an element or attribute, the key of a member, the position of an array item (`[0]`), `#text`, `#comment`.
    pub name: String,
    pub kind: RowKind,
    /// The value of a cell that has one (a leaf, or an element that holds nothing but text); a preview (`{3}`, `[2]`) for containers.
    pub value: String,
    /// How many rows are below it.
    pub child_count: usize,
    /// The row in the text, and its name in the text (empty where it has none).
    pub span: Range<usize>,
    pub name_span: Range<usize>,
    /// Whether the value cell can be edited.
    pub editable: bool,
}

/// A document as a tree.
#[derive(Clone, Debug)]
pub struct Grid {
    root: Node,
    family: Family,
}

/// An element that holds nothing but one run of text (or nothing): its value is that text, and the value is edited in place.
fn text_only(node: &Node) -> Option<Option<&Node>> {
    if node.kind != NodeKind::Element {
        return None;
    }
    let mut content = node.children.iter().filter(|c| c.kind != NodeKind::Attribute);
    match (content.next(), content.next()) {
        (None, _) => Some(None),
        (Some(only), None) if only.kind == NodeKind::Text => Some(Some(only)),
        _ => None,
    }
}

impl Grid {
    /// Read `text`, which is written in `family`.
    pub fn read(text: &str, family: Family) -> Result<Grid, GridError> {
        let root = family.read(text).map_err(|e| GridError(e.to_string()))?;
        Ok(Grid { root, family })
    }

    pub fn family(&self) -> Family {
        self.family
    }

    /// The number of nodes in the tree.
    pub fn count(&self) -> usize {
        self.root.count()
    }

    fn row_of(&self, node: &Node, path: Vec<usize>, depth: usize, index_in_array: Option<usize>) -> Row {
        let kind = kind_of(node.kind);
        let name = match node.kind {
            NodeKind::Document => String::new(),
            NodeKind::Text => "#text".to_owned(),
            NodeKind::Comment => "#comment".to_owned(),
            NodeKind::ProcessingInstruction => format!("?{}", node.name),
            _ if node.name.is_empty() => index_in_array.map_or_else(String::new, |i| format!("[{i}]")),
            _ => node.name.clone(),
        };
        let editable_scalar = match self.family {
            Family::Xml => matches!(node.kind, NodeKind::Attribute | NodeKind::Text | NodeKind::Comment),
            Family::Json | Family::Yaml => matches!(node.kind, NodeKind::String | NodeKind::Number | NodeKind::Bool | NodeKind::Null),
        };
        let only_text = text_only(node);
        let (value, editable) = match node.kind {
            NodeKind::Object => (format!("{{{}}}", node.children.len()), false),
            NodeKind::Array => (format!("[{}]", node.children.len()), false),
            NodeKind::Document => (String::new(), false),
            NodeKind::Element => match only_text {
                Some(Some(t)) => (t.text.clone(), true),
                Some(None) => (String::new(), true),
                None => (String::new(), false),
            },
            _ => (node.text.clone(), editable_scalar),
        };
        // Elements with only text show that text in their own row, so it has no row of its own.
        let child_count =
            if only_text.is_some() { node.children.iter().filter(|c| c.kind == NodeKind::Attribute).count() } else { node.children.len() };
        Row { path, depth, name, kind, value, child_count, span: node.span.clone(), name_span: node.name_span.clone(), editable }
    }

    /// The visible rows in order: the root's children (the root itself is the document), and the children of every row whose path is in
    /// `expanded`. At most [`MAX_ROWS`].
    pub fn rows(&self, expanded: &HashSet<Vec<usize>>) -> Vec<Row> {
        let mut rows = Vec::new();
        // An explicit stack: a deeply nested document does not nest the calls. It holds the nodes, so no row is found by walking down again.
        let mut stack: Vec<(&Node, Vec<usize>, Option<usize>)> = Vec::new();
        let top = self.visible_children(&self.root);
        for (i, node) in top.iter().enumerate().rev() {
            stack.push((node, vec![i], (self.root.kind == NodeKind::Array).then_some(i)));
        }
        while let Some((node, path, index_in_array)) = stack.pop() {
            if rows.len() >= MAX_ROWS {
                break;
            }
            let row = self.row_of(node, path.clone(), path.len().saturating_sub(1), index_in_array);
            let open = row.child_count > 0 && expanded.contains(&path);
            rows.push(row);
            if open {
                let visible = self.visible_children(node);
                for (i, child) in visible.iter().enumerate().rev() {
                    let mut next = path.clone();
                    next.push(i);
                    stack.push((child, next, (node.kind == NodeKind::Array).then_some(i)));
                }
            }
        }
        rows
    }

    /// The children of `node` that have rows: all of them, except the lone text of an element that holds nothing else.
    fn visible_children<'a>(&self, node: &'a Node) -> Vec<&'a Node> {
        if text_only(node).is_some() {
            node.children.iter().filter(|c| c.kind == NodeKind::Attribute).collect()
        } else {
            node.children.iter().collect()
        }
    }

    /// The node at `path` among visible rows, and its index among an array's items.
    fn at(&self, path: &[usize]) -> Option<(&Node, Option<usize>)> {
        let mut current = &self.root;
        let mut index = None;
        for step in path {
            let visible = self.visible_children(current);
            let next = *visible.get(*step)?;
            index = (current.kind == NodeKind::Array).then_some(*step);
            current = next;
        }
        Some((current, index))
    }

    /// The row at `path`.
    pub fn row(&self, path: &[usize]) -> Option<Row> {
        let (node, index) = self.at(path)?;
        Some(self.row_of(node, path.to_vec(), path.len().saturating_sub(1), index))
    }

    /// The paths of every row that has children (to expand all).
    pub fn expandable(&self) -> Vec<Vec<usize>> {
        let mut found = Vec::new();
        let mut stack: Vec<(&Node, Vec<usize>)> = self.visible_children(&self.root).into_iter().enumerate().map(|(i, n)| (n, vec![i])).collect();
        while let Some((node, path)) = stack.pop() {
            if found.len() >= MAX_ROWS {
                break;
            }
            let visible = self.visible_children(node);
            if !visible.is_empty() {
                for (i, child) in visible.into_iter().enumerate() {
                    let mut next = path.clone();
                    next.push(i);
                    stack.push((child, next));
                }
                found.push(path);
            }
        }
        found
    }
}

// --- edits -------------------------------------------------------------------------------------------------------------------

fn escape_xml(text: &str, quote: Option<char>) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c if Some(c) == quote => out.push_str(if c == '"' { "&quot;" } else { "&apos;" }),
            // A tab or a line end in an attribute value would be read as a space.
            '\t' if quote.is_some() => out.push_str("&#9;"),
            '\n' if quote.is_some() => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
    out
}

fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Whether `text` is a number as JSON writes one.
fn is_json_number(text: &str) -> bool {
    let t = text.strip_prefix('-').unwrap_or(text);
    let (int, rest) = match t.find(['.', 'e', 'E']) {
        Some(at) => t.split_at(at),
        None => (t, ""),
    };
    let int_ok = int == "0" || (!int.is_empty() && !int.starts_with('0') && int.chars().all(|c| c.is_ascii_digit()));
    if !int_ok {
        return false;
    }
    let mut rest = rest;
    if let Some(after) = rest.strip_prefix('.') {
        let digits = after.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 {
            return false;
        }
        rest = after.get(digits..).unwrap_or_default();
    }
    if let Some(after) = rest.strip_prefix(['e', 'E']) {
        let after = after.strip_prefix(['+', '-']).unwrap_or(after);
        return !after.is_empty() && after.chars().all(|c| c.is_ascii_digit());
    }
    rest.is_empty()
}

/// Whether a YAML plain scalar can be written as `text` and read back as the same string.
fn yaml_plain_ok(text: &str) -> bool {
    if text.is_empty() || text != text.trim() || text.contains(['\n', '\r', '\t']) {
        return false;
    }
    let first = text.chars().next().unwrap_or(' ');
    if matches!(first, '-' | '?' | ':' | ',' | '[' | ']' | '{' | '}' | '#' | '&' | '*' | '!' | '|' | '>' | '\'' | '"' | '%' | '@' | '`') {
        return false;
    }
    if text.contains(": ") || text.contains(" #") || text.ends_with(':') {
        return false;
    }
    // Words and numbers that are not strings when they are plain.
    let lower = text.to_ascii_lowercase();
    if matches!(lower.as_str(), "true" | "false" | "null" | "~" | "yes" | "no" | "on" | "off" | "y" | "n") {
        return false;
    }
    if text.parse::<f64>().is_ok() || text.starts_with("0x") || text.starts_with("0o") || matches!(lower.as_str(), ".inf" | "-.inf" | ".nan") {
        return false;
    }
    !text.contains(|c: char| c.is_control())
}

fn yaml_string(text: &str) -> Result<String, GridError> {
    if yaml_plain_ok(text) {
        return Ok(text.to_owned());
    }
    if text.contains(['\n', '\r']) {
        return fail("a string with line breaks is edited in the text (it may be a block scalar)");
    }
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    Ok(out)
}

/// Whether `name` is an XML name (a letter, `_` or `:` first; then also digits, `-`, `.`).
pub fn is_xml_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
    let start = |c: char| c.is_alphabetic() || c == '_' || c == ':';
    start(first) && chars.all(|c| start(c) || c.is_numeric() || c == '-' || c == '.' || c == '\u{b7}')
}

impl Grid {
    fn node(&self, path: &[usize]) -> Result<&Node, GridError> {
        self.at(path).map(|(n, _)| n).ok_or_else(|| GridError("that row is not there any more".to_owned()))
    }

    /// The text of `range`, checked to be where the tree says (so an edit made on a text that has changed since is refused).
    fn slice<'a>(&self, text: &'a str, range: &Range<usize>) -> Result<&'a str, GridError> {
        text.get(range.clone()).ok_or_else(|| GridError("the document has changed since the grid was read; read it again".to_owned()))
    }

    /// The edits that make the value cell of the row at `path` say `new_value` (the text, as typed).
    pub fn set_value(&self, text: &str, path: &[usize], new_value: &str) -> Result<Vec<Edit>, GridError> {
        let node = self.node(path)?;
        self.slice(text, &node.span)?;
        match self.family {
            Family::Xml => self.set_xml_value(text, node, new_value),
            Family::Json => self.set_json_value(node, new_value, false),
            Family::Yaml => self.set_yaml_value(text, node, new_value),
        }
    }

    fn set_xml_value(&self, text: &str, node: &Node, new_value: &str) -> Result<Vec<Edit>, GridError> {
        match node.kind {
            NodeKind::Attribute => {
                // `name = "value"`: the value is between the quotes after the equals sign.
                let whole = self.slice(text, &node.span)?;
                let after_name = whole.get(node.name_span.end.saturating_sub(node.span.start)..).unwrap_or_default();
                let eq = after_name.find('=').ok_or_else(|| GridError("the attribute has no value to change".to_owned()))?;
                let rest = after_name.get(eq + 1..).unwrap_or_default();
                let quote = rest
                    .trim_start()
                    .chars()
                    .next()
                    .filter(|c| matches!(c, '"' | '\''))
                    .ok_or_else(|| GridError("the attribute value is not quoted".to_owned()))?;
                let open_at = node.name_span.end + eq + 1 + (rest.len() - rest.trim_start().len()) + 1;
                let close_at = node.span.end.checked_sub(1).ok_or_else(|| GridError("the attribute is cut off".to_owned()))?;
                if text.get(close_at..node.span.end) != Some(&quote.to_string()) || open_at > close_at {
                    return fail("the attribute value cannot be found in the text");
                }
                Ok(vec![Edit::replace(open_at..close_at, escape_xml(new_value, Some(quote)))])
            }
            NodeKind::Text => Ok(vec![Edit::replace(node.span.clone(), escape_xml(new_value, None))]),
            NodeKind::Comment => {
                if new_value.contains("--") || new_value.ends_with('-') {
                    return fail("a comment cannot contain \"--\" or end with \"-\"");
                }
                Ok(vec![Edit::replace(node.span.clone(), format!("<!--{new_value}-->"))])
            }
            NodeKind::Element => match text_only(node) {
                Some(Some(t)) => Ok(vec![Edit::replace(t.span.clone(), escape_xml(new_value, None))]),
                Some(None) => {
                    let whole = self.slice(text, &node.span)?;
                    if let Some(open) = whole.strip_suffix("/>") {
                        // `<a b="1"/>` becomes `<a b="1">value</a>`.
                        let open_end = node.span.start + open.len();
                        let name = &node.name;
                        return Ok(vec![Edit::replace(open_end..node.span.end, format!(">{}</{name}>", escape_xml(new_value, None)))]);
                    }
                    // `<a></a>`: the value goes between the tags.
                    let close = format!("</{}", node.name);
                    let at = whole
                        .rfind(&close)
                        .map(|i| node.span.start + i)
                        .ok_or_else(|| GridError("the end tag cannot be found in the text".to_owned()))?;
                    Ok(vec![Edit::insert(at, escape_xml(new_value, None))])
                }
                None => fail("this element holds other elements; edit its children"),
            },
            _ => fail("this cell cannot be edited"),
        }
    }

    fn set_json_value(&self, node: &Node, new_value: &str, raw: bool) -> Result<Vec<Edit>, GridError> {
        let literal = if raw {
            let t = new_value.trim();
            if t == "true" || t == "false" || t == "null" || is_json_number(t) || (t.len() >= 2 && t.starts_with('"') && t.ends_with('"')) {
                t.to_owned()
            } else {
                return fail("that is not a JSON value: write a number, true, false, null or a string in quotes");
            }
        } else {
            match node.kind {
                NodeKind::String => json_string(new_value),
                NodeKind::Number => {
                    let t = new_value.trim();
                    if !is_json_number(t) {
                        return fail("that is not a number as JSON writes one (digits, a minus sign, a fraction, an exponent)");
                    }
                    t.to_owned()
                }
                NodeKind::Bool => match new_value.trim() {
                    "true" => "true".to_owned(),
                    "false" => "false".to_owned(),
                    _ => return fail("a boolean is true or false"),
                },
                NodeKind::Null => match new_value.trim() {
                    "null" => "null".to_owned(),
                    _ => return fail("null is written null; change the type to give it a value"),
                },
                _ => return fail("this cell cannot be edited"),
            }
        };
        Ok(vec![Edit::replace(node.span.clone(), literal)])
    }

    fn set_yaml_value(&self, text: &str, node: &Node, new_value: &str) -> Result<Vec<Edit>, GridError> {
        self.slice(text, &node.span)?;
        // A block scalar's span is its content: the header (`|`, `>-`, `|2`) is the last word before it.
        let before = text.get(..node.span.start).unwrap_or_default().trim_end();
        let header = before.rsplit(char::is_whitespace).next().unwrap_or_default();
        if header.starts_with(['|', '>']) && header.chars().skip(1).all(|c| matches!(c, '-' | '+') || c.is_ascii_digit()) {
            return fail("a block scalar is edited in the text");
        }
        let literal = match node.kind {
            NodeKind::String => yaml_string(new_value)?,
            NodeKind::Number => {
                let t = new_value.trim();
                if t.parse::<f64>().is_err() && !t.starts_with("0x") && !t.starts_with("0o") {
                    return fail("that is not a number");
                }
                t.to_owned()
            }
            NodeKind::Bool => match new_value.trim() {
                "true" | "false" => new_value.trim().to_owned(),
                _ => return fail("a boolean is true or false"),
            },
            NodeKind::Null => match new_value.trim() {
                "null" | "~" => new_value.trim().to_owned(),
                _ => return fail("null is written null; change the type to give it a value"),
            },
            _ => return fail("this cell cannot be edited"),
        };
        Ok(vec![Edit::replace(node.span.clone(), literal)])
    }

    /// The edits that give a JSON scalar a new value of any scalar type (`12`, `true`, `null`, `"text"`): what the type column does.
    pub fn set_literal(&self, text: &str, path: &[usize], literal: &str) -> Result<Vec<Edit>, GridError> {
        let node = self.node(path)?;
        self.slice(text, &node.span)?;
        if self.family != Family::Json {
            return fail("the type of a value is changed in the text for this format");
        }
        if !matches!(node.kind, NodeKind::String | NodeKind::Number | NodeKind::Bool | NodeKind::Null) {
            return fail("only a value can take another type");
        }
        self.set_json_value(node, literal, true)
    }

    /// The edits that rename the element, attribute or member at `path`.
    pub fn rename(&self, text: &str, path: &[usize], new_name: &str) -> Result<Vec<Edit>, GridError> {
        let node = self.node(path)?;
        self.slice(text, &node.span)?;
        match (self.family, node.kind) {
            (Family::Xml, NodeKind::Element) => {
                if !is_xml_name(new_name) {
                    return fail(format!("{new_name:?} is not a name an element can have"));
                }
                let mut edits = vec![Edit::replace(node.name_span.clone(), new_name)];
                // The end tag too (an element that is not self-closing).
                let whole = self.slice(text, &node.span)?;
                let close = format!("</{}", node.name);
                if let Some(i) = whole.rfind(&close).filter(|i| whole.get(i + close.len()..).is_some_and(|rest| rest.trim_start().starts_with('>'))) {
                    let start = node.span.start + i + 2;
                    edits.push(Edit::replace(start..start + node.name.len(), new_name));
                }
                Ok(edits)
            }
            (Family::Xml, NodeKind::Attribute) => {
                if !is_xml_name(new_name) {
                    return fail(format!("{new_name:?} is not a name an attribute can have"));
                }
                Ok(vec![Edit::replace(node.name_span.clone(), new_name)])
            }
            (Family::Json, _) if !node.name.is_empty() && node.name_span.end > node.name_span.start => {
                Ok(vec![Edit::replace(node.name_span.clone(), json_string(new_name))])
            }
            (Family::Yaml, _) if !node.name.is_empty() && node.name_span.end > node.name_span.start => {
                Ok(vec![Edit::replace(node.name_span.clone(), yaml_string(new_name)?)])
            }
            _ => fail("this row has no name to change"),
        }
    }

    /// The edits that remove the row at `path` (and its place: a member's comma, an attribute's space).
    pub fn remove(&self, text: &str, path: &[usize]) -> Result<Vec<Edit>, GridError> {
        if path.is_empty() {
            return fail("the document is not removed from its grid");
        }
        let node = self.node(path)?;
        self.slice(text, &node.span)?;
        let parent_path = path.get(..path.len() - 1).unwrap_or_default();
        let parent = self.node(parent_path)?;
        match self.family {
            Family::Xml => {
                let mut start = node.span.start;
                // An attribute takes the white space before it.
                if node.kind == NodeKind::Attribute {
                    let before = text.get(..start).unwrap_or_default();
                    start -= before.len() - before.trim_end().len();
                }
                Ok(vec![Edit::delete(start..node.span.end)])
            }
            Family::Json => {
                let siblings: Vec<&Node> = self.visible_children(parent);
                let index = *path.last().unwrap_or(&0);
                let member_start = if node.name_span.end > node.name_span.start { node.name_span.start } else { node.span.start };
                if siblings.len() == 1 {
                    // The only one: the container is left empty.
                    return Ok(vec![Edit::delete(member_start..node.span.end)]);
                }
                if index + 1 < siblings.len() {
                    // Not the last: from its start to the start of the next (its comma and the white space go with it).
                    let next = siblings.get(index + 1).ok_or_else(|| GridError("the next row is not there".to_owned()))?;
                    let next_start = if next.name_span.end > next.name_span.start { next.name_span.start } else { next.span.start };
                    Ok(vec![Edit::delete(member_start..next_start)])
                } else {
                    // The last: from the end of the one before (the comma after it goes) to its end.
                    let previous = siblings.get(index.wrapping_sub(1)).ok_or_else(|| GridError("the row before is not there".to_owned()))?;
                    Ok(vec![Edit::delete(previous.span.end..node.span.end)])
                }
            }
            Family::Yaml => {
                // Block style: whole lines.
                let line_start = text.get(..node.span.start).and_then(|b| b.rfind('\n')).map_or(0, |i| i + 1);
                let key_start = if node.name_span.end > node.name_span.start { node.name_span.start } else { node.span.start };
                let before = text.get(line_start..key_start).unwrap_or_default();
                let is_entry = before.trim().is_empty() || before.trim() == "-";
                if !is_entry {
                    return fail("a row in flow style ({ } or [ ]) is removed in the text");
                }
                let mut end = node.span.end;
                if let Some(rest) = text.get(end..) {
                    end += rest.find('\n').map_or(rest.len(), |i| i + 1);
                }
                Ok(vec![Edit::delete(line_start..end)])
            }
        }
    }
}
