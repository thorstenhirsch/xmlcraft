//! Tables of repeated items: the children of a node that repeat (elements of one name in XML, the items of an array of objects or of
//! scalars in JSON and YAML) as rows, and what they hold as columns.
//!
//! - [`Grid::table`] builds the table of a node, or says there is none. A cell holds the text of a scalar (an attribute, an element with
//!   only text, a member), or is [`CellKind::Nested`] (it has structure, opened by drilling in), or [`CellKind::Missing`].
//! - [`Table::sorted`] and [`Table::filtered`] are views: an order or a subset of the rows, and nothing in the text changes. The edits
//!   that change the text are in [`crate::table_edit`].
//! - Bounds: at most [`MAX_COLUMNS`] columns (the rest are counted) and [`crate::MAX_ROWS`] rows (the rest are counted too).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::ops::Range;

use xmlcraft_diff::{Family, Node, NodeKind};

use crate::{Flavor, Grid, GridError, MAX_ROWS, RowKind, fail, kind_of, text_only};

/// The most columns a table shows; the others are counted in [`Table::hidden_columns`].
pub const MAX_COLUMNS: usize = 200;
/// The most rows of tab-separated text that are pasted at once.
pub const MAX_PASTE_ROWS: usize = 10_000;
/// The most cells of tab-separated text that are pasted at once.
pub const MAX_PASTE_CELLS: usize = 200_000;

/// What a column is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColumnKind {
    /// An attribute of the XML items (shown `@name`).
    Attribute,
    /// The child elements of one name of the XML items.
    Element,
    /// The text of XML items that hold only text (shown `#text`).
    Text,
    /// A member of the objects of an array.
    Member,
    /// The items of an array of scalars themselves.
    Value,
}

/// A column of a table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    /// The attribute, element or member name (an attribute without its `@`); `#text` and `value` for the others.
    pub name: String,
    pub kind: ColumnKind,
    /// Every cell with a value in it is a decimal number, so the column sorts by value.
    pub numeric: bool,
    /// The type most of its values have (JSON and YAML; none when it has none): what a value typed into an empty cell becomes.
    pub value_kind: Option<RowKind>,
}

impl Column {
    /// The name as the header shows it: `@id` for an attribute.
    pub fn label(&self) -> String {
        match self.kind {
            ColumnKind::Attribute => format!("@{}", self.name),
            _ => self.name.clone(),
        }
    }
}

/// What a cell holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellKind {
    /// A value: its text is the value.
    Scalar,
    /// Something with structure: an object, an array, an element with elements in it, or several elements of the column's name.
    Nested,
    /// Nothing: the row has no such attribute, element or member.
    Missing,
}

/// Where drilling into a cell goes: the node, and (for several elements of one name) the name, so the table of those elements opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drill {
    pub path: Vec<usize>,
    pub name: Option<String>,
}

/// A cell of a table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub kind: CellKind,
    /// The value of a scalar; `{3}`, `[5]` or `<2>` for something nested; empty when missing.
    pub text: String,
    /// The value in the text (for an attribute, between its quotes); for a missing cell, the empty range where it would be inserted.
    pub span: Range<usize>,
    /// The type of a value (string, number, boolean, null; attribute, text or element for XML).
    pub value_kind: RowKind,
    /// How many things a nested cell holds.
    pub count: usize,
    pub drill: Option<Drill>,
}

impl Cell {
    fn missing(at: usize) -> Cell {
        Cell { kind: CellKind::Missing, text: String::new(), span: at..at, value_kind: RowKind::Null, count: 0, drill: None }
    }
}

/// A row of a table: one item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableRow {
    /// The item's path in the grid (as [`crate::Row::path`]).
    pub path: Vec<usize>,
    /// The item in the text.
    pub span: Range<usize>,
    pub cells: Vec<Cell>,
}

/// How the items of a table are written, which says how one is added, removed or moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    XmlElements,
    JsonArray,
    /// JSON Lines: the document itself, one item on each line.
    JsonLines,
    /// A YAML block sequence (`- item` on lines of their own).
    YamlBlock,
    /// A YAML flow sequence (`[a, b]`).
    YamlFlow,
    /// The documents of a YAML stream (`---`): their cells are edited, but no document is added, removed or moved here.
    YamlDocuments,
}

/// The table of a node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    /// The node whose children are the rows.
    pub path: Vec<usize>,
    /// The name of the XML elements that are the rows.
    pub item_name: Option<String>,
    pub layout: Layout,
    pub columns: Vec<Column>,
    pub rows: Vec<TableRow>,
    /// How many items there are (more than [`Table::rows`] when the table is cut at [`MAX_ROWS`]).
    pub total_rows: usize,
    /// How many columns are not shown (past [`MAX_COLUMNS`]).
    pub hidden_columns: usize,
    /// The family of the text.
    pub family: Family,
}

/// The extended JSON wrappers that stand for one value (`{"$numberLong": "5"}`, `{"$oid": "…"}`).
fn wrapped_value(flavor: Flavor, node: &Node) -> Option<&Node> {
    if flavor != Flavor::ExtendedJson || node.kind != NodeKind::Object {
        return None;
    }
    match node.children.as_slice() {
        [only] if only.name.starts_with('$') && matches!(only.kind, NodeKind::String | NodeKind::Number | NodeKind::Bool) => Some(only),
        _ => None,
    }
}

fn is_scalar(kind: NodeKind) -> bool {
    matches!(kind, NodeKind::String | NodeKind::Number | NodeKind::Bool | NodeKind::Null)
}

/// Whether an XML text node is no more than white space.
fn blank(node: &Node) -> bool {
    node.kind == NodeKind::Text && node.text.trim().is_empty()
}

/// The end of `span` without the white space at its end (a YAML block mapping runs on to where the next item starts).
pub(crate) fn trimmed_end(text: &str, span: &Range<usize>) -> usize {
    text.get(span.clone()).map_or(span.end, |s| span.start + s.trim_end().len())
}

impl Grid {
    /// The table of the node at `path`, when its children repeat: XML elements of one name (at least two, the most common name; the
    /// other children stay out of the table), or the items of a JSON or YAML array whose items are all objects or all scalars.
    pub fn table(&self, text: &str, path: &[usize]) -> Option<Table> {
        self.table_named(text, path, None)
    }

    /// [`Grid::table`] with the XML elements of `name` as the rows (one is enough), as drilling into a cell of several elements opens.
    pub fn table_named(&self, text: &str, path: &[usize], name: Option<&str>) -> Option<Table> {
        let (container, _) = self.at(path)?;
        text.get(container.span.clone())?;
        let (layout, item_name, items) = self.items_of(text, container, name)?;
        let columns = self.default_columns(text, container, &items);
        Some(self.build(text, path, container, layout, item_name, items, columns))
    }

    /// The table at `path` with `columns` as its columns, in this order (columns that are not in the text yet have empty cells).
    pub(crate) fn table_with_columns(&self, text: &str, table: &Table, columns: Vec<Column>) -> Result<Table, GridError> {
        let (container, _) = self.at(&table.path).ok_or_else(|| GridError("the table is not there any more".to_owned()))?;
        let (layout, item_name, items) =
            self.items_of(text, container, table.item_name.as_deref()).ok_or_else(|| GridError("the node no longer holds a table".to_owned()))?;
        Ok(self.build(text, &table.path, container, layout, item_name, items, columns))
    }

    /// The children of `container` that are the rows (their indexes), how they are written, and their element name.
    fn items_of(&self, text: &str, container: &Node, name: Option<&str>) -> Option<(Layout, Option<String>, Vec<usize>)> {
        match self.family {
            Family::Xml => {
                if container.kind != NodeKind::Element {
                    return None;
                }
                let elements = || container.children.iter().enumerate().filter(|(_, c)| c.kind == NodeKind::Element);
                let chosen = match name {
                    Some(n) => n.to_owned(),
                    None => {
                        // The most common name (the first of those as common), seen twice or more.
                        let mut counts: Vec<(&str, usize)> = Vec::new();
                        for (_, c) in elements() {
                            match counts.iter_mut().find(|(n, _)| *n == c.name) {
                                Some((_, k)) => *k += 1,
                                None => counts.push((&c.name, 1)),
                            }
                        }
                        let best = counts.iter().fold(None::<(&str, usize)>, |best, &(n, k)| match best {
                            Some((_, b)) if b >= k => best,
                            _ => Some((n, k)),
                        })?;
                        if best.1 < 2 {
                            return None;
                        }
                        best.0.to_owned()
                    }
                };
                let items: Vec<usize> = elements().filter(|(_, c)| c.name == chosen).map(|(i, _)| i).collect();
                (!items.is_empty()).then_some((Layout::XmlElements, Some(chosen), items))
            }
            Family::Json | Family::Yaml => {
                if container.kind != NodeKind::Array || container.children.is_empty() {
                    return None;
                }
                let objects = container.children.iter().all(|c| c.kind == NodeKind::Object && wrapped_value(self.flavor, c).is_none());
                let scalars = container.children.iter().all(|c| is_scalar(c.kind) || wrapped_value(self.flavor, c).is_some());
                if !objects && !scalars {
                    return None;
                }
                let at = text.get(container.span.start..).unwrap_or_default();
                let layout = match (self.family, self.flavor) {
                    // (The lines are the array read from the whole text, so it is the root.)
                    (Family::Json, Flavor::JsonLines) if std::ptr::eq(container, &self.root) => Layout::JsonLines,
                    (Family::Json, _) => Layout::JsonArray,
                    _ if at.starts_with('[') => Layout::YamlFlow,
                    // `- item`, and not the `---` that starts a document of a stream.
                    _ if at.starts_with('-') && at.chars().nth(1).is_some_and(char::is_whitespace) => Layout::YamlBlock,
                    _ => Layout::YamlDocuments,
                };
                Some((layout, None, (0..container.children.len()).collect()))
            }
        }
    }

    /// The columns of the items: attributes (`@name`) and then child elements in the order first seen, with `#text` for items that hold
    /// only text; the members of objects in the order first seen; `value` for scalars.
    fn default_columns(&self, _text: &str, container: &Node, items: &[usize]) -> Vec<Column> {
        let nodes = || items.iter().filter_map(|&i| container.children.get(i));
        let column = |name: &str, kind| Column { name: name.to_owned(), kind, numeric: false, value_kind: None };
        let mut columns: Vec<Column> = Vec::new();
        let mut seen: HashMap<(ColumnKind, String), ()> = HashMap::new();
        let mut add = |columns: &mut Vec<Column>, name: &str, kind: ColumnKind| {
            if seen.insert((kind, name.to_owned()), ()).is_none() {
                columns.push(column(name, kind));
            }
        };
        match self.family {
            Family::Xml => {
                for item in nodes() {
                    for a in item.children.iter().filter(|c| c.kind == NodeKind::Attribute) {
                        add(&mut columns, &a.name, ColumnKind::Attribute);
                    }
                }
                if nodes().any(|item| matches!(text_only(item), Some(Some(t)) if !t.text.trim().is_empty())) {
                    add(&mut columns, "#text", ColumnKind::Text);
                }
                for item in nodes() {
                    for e in item.children.iter().filter(|c| c.kind == NodeKind::Element) {
                        add(&mut columns, &e.name, ColumnKind::Element);
                    }
                }
                if columns.is_empty() {
                    add(&mut columns, "#text", ColumnKind::Text);
                }
            }
            Family::Json | Family::Yaml => {
                if nodes().all(|c| c.kind == NodeKind::Object && wrapped_value(self.flavor, c).is_none()) {
                    for item in nodes() {
                        for m in &item.children {
                            add(&mut columns, &m.name, ColumnKind::Member);
                        }
                    }
                } else {
                    add(&mut columns, "value", ColumnKind::Value);
                }
            }
        }
        columns
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        &self,
        text: &str,
        path: &[usize],
        container: &Node,
        layout: Layout,
        item_name: Option<String>,
        items: Vec<usize>,
        mut columns: Vec<Column>,
    ) -> Table {
        let hidden_columns = columns.len().saturating_sub(MAX_COLUMNS);
        columns.truncate(MAX_COLUMNS);
        let total_rows = items.len();
        let mut rows = Vec::with_capacity(items.len().min(MAX_ROWS));
        for &i in items.iter().take(MAX_ROWS) {
            let Some(item) = container.children.get(i) else { continue };
            let mut item_path = path.to_vec();
            item_path.push(i);
            let cells = columns.iter().map(|c| self.cell_of(text, item, &item_path, c)).collect();
            rows.push(TableRow { path: item_path, span: item.span.clone(), cells });
        }
        // What the columns hold: numbers (to sort by), and the type most of their values have.
        for (k, column) in columns.iter_mut().enumerate() {
            let scalars = || rows.iter().filter_map(|r| r.cells.get(k)).filter(|c| c.kind == CellKind::Scalar);
            let mut with_value = scalars().filter(|c| !c.text.trim().is_empty()).peekable();
            column.numeric = with_value.peek().is_some() && with_value.all(|c| decimal(&c.text).is_some());
            if self.family != Family::Xml {
                let mut counts: Vec<(RowKind, usize)> = Vec::new();
                for c in scalars().filter(|c| c.value_kind != RowKind::Null) {
                    match counts.iter_mut().find(|(k, _)| *k == c.value_kind) {
                        Some((_, n)) => *n += 1,
                        None => counts.push((c.value_kind, 1)),
                    }
                }
                column.value_kind = counts
                    .iter()
                    .fold(None::<(RowKind, usize)>, |best, &(k, n)| match best {
                        Some((_, b)) if b >= n => best,
                        _ => Some((k, n)),
                    })
                    .map(|(k, _)| k);
            }
        }
        // Where a missing cell would be written: the place of the edit that would write it.
        {
            let ctx = self.ctx(text, &columns, layout, container, item_name.as_deref());
            for row in &mut rows {
                let Some(item) = row.path.last().and_then(|i| container.children.get(*i)) else { continue };
                for (c, cell) in row.cells.iter_mut().enumerate() {
                    if cell.kind == CellKind::Missing
                        && let Some(at) = self.insertion_point(&ctx, item, c)
                    {
                        cell.span = at..at;
                    }
                }
            }
        }
        Table { path: path.to_vec(), item_name, layout, columns, rows, total_rows, hidden_columns, family: self.family }
    }

    fn cell_of(&self, text: &str, item: &Node, item_path: &[usize], column: &Column) -> Cell {
        let at = item.span.start;
        let child_path = |i: usize| {
            let mut p = item_path.to_vec();
            p.push(i);
            p
        };
        let scalar = |node: &Node, span: Range<usize>| Cell {
            kind: CellKind::Scalar,
            text: node.text.clone(),
            span,
            value_kind: kind_of(node.kind),
            count: 0,
            drill: None,
        };
        let nested = |label: String, count: usize, drill: Drill| Cell {
            kind: CellKind::Nested,
            text: label,
            span: item.span.clone(),
            value_kind: RowKind::Element,
            count,
            drill: Some(drill),
        };
        match column.kind {
            ColumnKind::Attribute => match item.children.iter().find(|c| c.kind == NodeKind::Attribute && c.name == column.name) {
                Some(a) => {
                    let span = self.attribute_value(text, a).map_or_else(|_| a.span.clone(), |(r, _)| r);
                    scalar(a, span)
                }
                None => Cell::missing(at),
            },
            ColumnKind::Text => match text_only(item) {
                Some(Some(t)) => scalar(t, t.span.clone()),
                Some(None) => Cell::missing(at),
                None if item.children.iter().any(|c| c.kind == NodeKind::Text && !blank(c)) => {
                    let count = item.children.iter().filter(|c| c.kind != NodeKind::Attribute && !blank(c)).count();
                    nested(format!("<{count}>"), count, Drill { path: item_path.to_vec(), name: None })
                }
                None => Cell::missing(at),
            },
            ColumnKind::Element => {
                let named: Vec<(usize, &Node)> =
                    item.children.iter().enumerate().filter(|(_, c)| c.kind == NodeKind::Element && c.name == column.name).collect();
                match named.as_slice() {
                    [] => Cell::missing(at),
                    [(i, only)] => match text_only(only) {
                        Some(Some(t)) => {
                            let mut c = scalar(t, t.span.clone());
                            c.value_kind = RowKind::Element;
                            c.count = only.children.iter().filter(|a| a.kind == NodeKind::Attribute).count();
                            c.drill = (c.count > 0).then(|| Drill { path: child_path(*i), name: None });
                            c
                        }
                        Some(None) => Cell {
                            kind: CellKind::Scalar,
                            text: String::new(),
                            span: only.span.clone(),
                            value_kind: RowKind::Element,
                            count: only.children.len(),
                            drill: (!only.children.is_empty()).then(|| Drill { path: child_path(*i), name: None }),
                        },
                        None => {
                            let count = only.children.iter().filter(|c| !blank(c)).count();
                            nested(format!("<{count}>"), count, Drill { path: child_path(*i), name: None })
                        }
                    },
                    several => {
                        nested(format!("<{}>", several.len()), several.len(), Drill { path: item_path.to_vec(), name: Some(column.name.clone()) })
                    }
                }
            }
            ColumnKind::Member => match item.children.iter().enumerate().find(|(_, m)| m.name == column.name) {
                Some((i, m)) => self.data_cell(m, child_path(i)),
                None => Cell::missing(at),
            },
            ColumnKind::Value => self.data_cell(item, item_path.to_vec()),
        }
    }

    fn data_cell(&self, node: &Node, path: Vec<usize>) -> Cell {
        let inner = wrapped_value(self.flavor, node).unwrap_or(node);
        match inner.kind {
            NodeKind::Object => Cell {
                kind: CellKind::Nested,
                text: format!("{{{}}}", inner.children.len()),
                span: inner.span.clone(),
                value_kind: RowKind::Object,
                count: inner.children.len(),
                drill: Some(Drill { path, name: None }),
            },
            NodeKind::Array => Cell {
                kind: CellKind::Nested,
                text: format!("[{}]", inner.children.len()),
                span: inner.span.clone(),
                value_kind: RowKind::Array,
                count: inner.children.len(),
                drill: Some(Drill { path, name: None }),
            },
            k => Cell { kind: CellKind::Scalar, text: inner.text.clone(), span: inner.span.clone(), value_kind: kind_of(k), count: 0, drill: None },
        }
    }

    /// The deepest node with a table that holds `offset`: the table the caret is in.
    pub fn table_path_at(&self, text: &str, offset: usize) -> Option<Vec<usize>> {
        let mut path = Vec::new();
        let mut found = self.table(text, &path).map(|_| Vec::new());
        let mut current = &self.root;
        // (Bounded by the depth of the tree, which the readers bound.)
        loop {
            let visible = self.visible_children(current);
            let Some((i, next)) = visible.iter().enumerate().find(|(_, c)| c.span.start <= offset && offset < c.span.end.max(c.span.start + 1))
            else {
                break;
            };
            path.push(i);
            current = next;
            if self.table(text, &path).is_some() {
                found = Some(path.clone());
            }
        }
        found
    }

    /// The path of a node named by a pointer: a JSON Pointer for JSON and YAML (`/items/0`), element names for XML (`/library/book[2]`,
    /// counted from 1 among the elements of that name). `/` or nothing is the top.
    pub fn path_of_pointer(&self, pointer: &str) -> Result<Vec<usize>, GridError> {
        let mut path = Vec::new();
        let mut current = &self.root;
        let trimmed = pointer.strip_prefix('/').unwrap_or(pointer);
        if trimmed.is_empty() {
            return Ok(path);
        }
        for raw in trimmed.split('/') {
            let visible = self.visible_children(current);
            let step = match self.family {
                Family::Xml => {
                    let (name, nth) = match raw.split_once('[') {
                        Some((n, rest)) => {
                            let k = rest.strip_suffix(']').and_then(|k| k.parse::<usize>().ok()).filter(|k| *k >= 1);
                            (n, k.ok_or_else(|| GridError(format!("{raw:?} is a name, or a name and a number from 1 in brackets")))?)
                        }
                        None => (raw, 1),
                    };
                    visible.iter().enumerate().filter(|(_, c)| c.kind == NodeKind::Element && c.name == name).nth(nth - 1).map(|(i, _)| i)
                }
                Family::Json | Family::Yaml => {
                    let key = raw.replace("~1", "/").replace("~0", "~");
                    if current.kind == NodeKind::Array {
                        key.parse::<usize>().ok().filter(|i| *i < visible.len())
                    } else {
                        visible.iter().position(|c| c.name == key)
                    }
                }
            };
            let step = step.ok_or_else(|| GridError(format!("nothing is at {raw:?} in {pointer:?}")))?;
            path.push(step);
            current = visible.get(step).copied().ok_or_else(|| GridError(format!("nothing is at {raw:?} in {pointer:?}")))?;
        }
        Ok(path)
    }

    /// The name of every step of `path` (for a breadcrumb): element names, keys, `[2]` for array items.
    pub fn names_along(&self, path: &[usize]) -> Vec<String> {
        let mut names = Vec::new();
        let mut current = &self.root;
        for step in path {
            let visible = self.visible_children(current);
            let Some(next) = visible.get(*step) else { break };
            names.push(if next.name.is_empty() { format!("[{step}]") } else { next.name.clone() });
            current = next;
        }
        names
    }
}

// --- views -------------------------------------------------------------------------------------------------------------------

/// A decimal number as a sign, its significant digits and the exponent of the first of them, so two numbers compare exactly (no
/// floating point): `-12.50` is (negative, "125", 1). Zero has no digits. Anything else (hexadecimal, `Infinity`) is none.
fn decimal(text: &str) -> Option<(Ordering, String, i64)> {
    let t = text.trim();
    let (negative, rest) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let (mantissa, exponent) = match rest.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i64>().ok().filter(|e| e.unsigned_abs() < 1_000_000_000)?),
        None => (rest, 0),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if (int.is_empty() && frac.is_empty()) || !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    let digits = format!("{int}{frac}");
    let significant_start = digits.trim_start_matches('0');
    if significant_start.is_empty() {
        return Some((Ordering::Equal, String::new(), 0));
    }
    let leading = i64::try_from(digits.len() - significant_start.len()).ok()?;
    let point = i64::try_from(int.len()).ok()?.checked_sub(leading)?.checked_sub(1)?;
    let sign = if negative { Ordering::Less } else { Ordering::Greater };
    Some((sign, significant_start.trim_end_matches('0').to_owned(), point.checked_add(exponent)?))
}

/// Compare two decimal numbers exactly.
fn compare_decimal(a: &(Ordering, String, i64), b: &(Ordering, String, i64)) -> Ordering {
    let rank = |s: Ordering| match s {
        Ordering::Less => 0,
        Ordering::Equal => 1,
        Ordering::Greater => 2,
    };
    match rank(a.0).cmp(&rank(b.0)) {
        Ordering::Equal => {}
        other => return other,
    }
    let magnitude = a.2.cmp(&b.2).then_with(|| a.1.cmp(&b.1));
    match a.0 {
        Ordering::Less => magnitude.reverse(),
        Ordering::Equal => Ordering::Equal,
        Ordering::Greater => magnitude,
    }
}

impl Table {
    /// The index of the column whose label (`@id`, `title`) or name is `name`.
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.label() == name).or_else(|| self.columns.iter().position(|c| c.name == name))
    }

    /// Whether every item is a row (the table is not cut).
    pub fn is_whole(&self) -> bool {
        self.rows.len() == self.total_rows
    }

    /// The rows in the order of `column`: by value for a numeric column (exactly, as decimals), by text otherwise; empty and nested
    /// cells last either way, and rows that are equal stay in their order. A view: the text is not changed ([`Grid::apply_order`]).
    pub fn sorted(&self, column: usize, ascending: bool) -> Vec<usize> {
        self.sorted_rows(&(0..self.rows.len()).collect::<Vec<_>>(), column, ascending)
    }

    /// [`Table::sorted`] of some of the rows (those a filter leaves).
    pub fn sorted_rows(&self, rows: &[usize], column: usize, ascending: bool) -> Vec<usize> {
        let numeric = self.columns.get(column).is_some_and(|c| c.numeric);
        let key = |r: usize| self.rows.get(r).and_then(|row| row.cells.get(column)).filter(|c| c.kind == CellKind::Scalar);
        let mut order = rows.to_vec();
        order.sort_by(|&a, &b| {
            let (ca, cb) = (key(a), key(b));
            match (ca, cb) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(x), Some(y)) => {
                    let o = if numeric {
                        match (decimal(&x.text), decimal(&y.text)) {
                            (Some(p), Some(q)) => compare_decimal(&p, &q),
                            (Some(_), None) => Ordering::Less,
                            (None, Some(_)) => Ordering::Greater,
                            (None, None) => Ordering::Equal,
                        }
                    } else {
                        x.text.to_lowercase().cmp(&y.text.to_lowercase()).then_with(|| x.text.cmp(&y.text))
                    };
                    if ascending { o } else { o.reverse() }
                }
            }
        });
        order
    }

    /// The rows with `needle` in one of their cells (letters of any case). A view.
    pub fn filtered(&self, needle: &str) -> Vec<usize> {
        let needle = needle.trim().to_lowercase();
        (0..self.rows.len())
            .filter(|&r| {
                needle.is_empty()
                    || self
                        .rows
                        .get(r)
                        .is_some_and(|row| row.cells.iter().any(|c| c.kind != CellKind::Missing && c.text.to_lowercase().contains(&needle)))
            })
            .collect()
    }

    /// The cells of `rows` × `columns` as tab-separated text (a line for each row; a cell with a tab, a line break or a quote is quoted
    /// the way spreadsheets quote it). Missing cells are empty; nested cells are their label (`{3}`).
    pub fn to_tsv(&self, rows: &[usize], columns: &[usize]) -> String {
        let mut out = String::new();
        for &r in rows {
            let Some(row) = self.rows.get(r) else { continue };
            let fields: Vec<String> = columns.iter().map(|&c| row.cells.get(c).map_or_else(String::new, |cell| tsv_field(&cell.text))).collect();
            out.push_str(&fields.join("\t"));
            out.push('\n');
        }
        out
    }
}

fn tsv_field(text: &str) -> String {
    if text.contains(['\t', '\n', '\r', '"']) { format!("\"{}\"", text.replace('"', "\"\"")) } else { text.to_owned() }
}

/// Tab-separated text as rows of cells: tabs between cells, a line break between rows, a cell in quotes may hold tabs, line breaks and
/// doubled quotes. The line break at the very end ends the last row. At most [`MAX_PASTE_ROWS`] rows and [`MAX_PASTE_CELLS`] cells.
pub fn parse_tsv(text: &str) -> Result<Vec<Vec<String>>, GridError> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut cells = 0usize;
    let mut chars = text.chars().peekable();
    let mut at_field_start = true;
    let mut quoted = false;
    let push_field = |row: &mut Vec<String>, field: &mut String, cells: &mut usize| -> Result<(), GridError> {
        *cells += 1;
        if *cells > MAX_PASTE_CELLS {
            return fail(format!("more than {MAX_PASTE_CELLS} cells are not pasted at once"));
        }
        row.push(std::mem::take(field));
        Ok(())
    };
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if at_field_start => {
                quoted = true;
                at_field_start = false;
            }
            '\t' => {
                push_field(&mut row, &mut field, &mut cells)?;
                at_field_start = true;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' => {
                push_field(&mut row, &mut field, &mut cells)?;
                rows.push(std::mem::take(&mut row));
                if rows.len() > MAX_PASTE_ROWS {
                    return fail(format!("more than {MAX_PASTE_ROWS} rows are not pasted at once"));
                }
                at_field_start = true;
            }
            c => {
                field.push(c);
                at_field_start = false;
            }
        }
    }
    if !at_field_start || !field.is_empty() || !row.is_empty() || quoted {
        push_field(&mut row, &mut field, &mut cells)?;
        rows.push(row);
    }
    if rows.len() > MAX_PASTE_ROWS {
        return fail(format!("more than {MAX_PASTE_ROWS} rows are not pasted at once"));
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_compare_exactly() {
        let d = |t: &str| decimal(t).unwrap_or((Ordering::Equal, "x".into(), 0));
        let lt = |a: &str, b: &str| compare_decimal(&d(a), &d(b)) == Ordering::Less;
        assert!(lt("2", "10") && lt("-10", "-2") && lt("-1", "0") && lt("0", "0.0001") && lt("9.99", "10"));
        assert!(lt("0.1", "0.10000000000000000000000001"), "more digits than a double has");
        assert_eq!(compare_decimal(&d("1.50"), &d("15e-1")), Ordering::Equal);
        assert_eq!(compare_decimal(&d("-0"), &d("0.000")), Ordering::Equal);
        assert!(decimal("12a").is_none() && decimal("").is_none() && decimal("0x1F").is_none() && decimal("1e99999999999").is_none());
    }

    #[test]
    fn tsv_quotes_and_reads_back() {
        let rows = parse_tsv("a\t\"b\tc\"\n\"x\"\"y\"\t\"two\nlines\"\n").unwrap_or_default();
        assert_eq!(rows, vec![vec!["a".to_owned(), "b\tc".to_owned()], vec!["x\"y".to_owned(), "two\nlines".to_owned()]]);
        assert_eq!(parse_tsv("").unwrap_or_default(), Vec::<Vec<String>>::new());
        assert_eq!(parse_tsv("a\r\nb").unwrap_or_default(), vec![vec!["a".to_owned()], vec!["b".to_owned()]]);
        assert_eq!(parse_tsv("\t").unwrap_or_default(), vec![vec![String::new(), String::new()]]);
        assert_eq!(tsv_field("a\"b"), "\"a\"\"b\"");
    }
}
