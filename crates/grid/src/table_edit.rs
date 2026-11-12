//! The edits of a table: a cell set, created or cleared; a row inserted, duplicated, deleted or moved; a column renamed or deleted; the
//! rows put in an order; tab-separated text pasted.
//!
//! Every edit is a list of minimal [`Edit`]s of the text, made with the places the tree knows, that follow the document's layout: its
//! indentation, its dominant line ending, JSON's commas, YAML's indentation, XML's escaping. Bytes no edit names are never touched. A
//! change that cannot be made is an error that says why, and nothing is edited.

use std::ops::Range;

use xmlcraft_diff::{Family, Node, NodeKind};
use xmlcraft_text::Edit;
use xmlcraft_text::eol::LineEndings;

use crate::table::{CellKind, Column, ColumnKind, Layout, Table, parse_tsv, trimmed_end};
use crate::{
    Flavor, Grid, GridError, RowKind, escape_xml, fail, is_json_number, is_xml_name, is_yaml_number, json_string, text_only, yaml_block_scalar,
    yaml_string,
};

/// What a paste did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pasted {
    pub edits: Vec<Edit>,
    /// How many cells were given a value.
    pub cells: usize,
    /// Cells left as they were: nested ones, and those past the last column.
    pub skipped: usize,
    /// Rows added at the end.
    pub appended: usize,
}

// --- places in the text ------------------------------------------------------------------------------------------------------

fn line_start(text: &str, at: usize) -> usize {
    text.get(..at).and_then(|b| b.rfind(['\n', '\r'])).map_or(0, |i| i + 1)
}

fn line_end(text: &str, at: usize) -> usize {
    text.get(at..).and_then(|r| r.find(['\n', '\r'])).map_or(text.len(), |i| at + i)
}

/// The offset after the line break that ends the line `at` is on (the end of the text on the last line).
fn after_line(text: &str, at: usize) -> usize {
    let end = line_end(text, at);
    match text.get(end..) {
        Some(r) if r.starts_with("\r\n") => end + 2,
        Some(r) if r.starts_with(['\n', '\r']) => end + 1,
        _ => end,
    }
}

/// The white space before `at` on its line, when nothing else is before it there.
fn indent_before(text: &str, at: usize) -> Option<&str> {
    let pre = text.get(line_start(text, at)..at)?;
    pre.chars().all(|c| c == ' ' || c == '\t').then_some(pre)
}

/// The white space the line of `at` starts with.
fn line_indent(text: &str, at: usize) -> &str {
    let start = line_start(text, at);
    let line = text.get(start..line_end(text, start)).unwrap_or_default();
    line.get(..line.len() - line.trim_start_matches([' ', '\t']).len()).unwrap_or_default()
}

/// Whether the rest of the line from `from` is white space or a comment (`//`, `#`, or a `/* */` that ends on the line).
fn rest_is_trivia(text: &str, from: usize) -> bool {
    let rest = text.get(from..line_end(text, from)).unwrap_or_default().trim();
    if rest.is_empty() || rest.starts_with("//") || rest.starts_with('#') {
        return true;
    }
    rest.strip_prefix("/*").and_then(|r| r.find("*/").map(|i| r.get(i + 2..).unwrap_or_default())).is_some_and(rest_is_trivia_str)
}

fn rest_is_trivia_str(rest: &str) -> bool {
    let rest = rest.trim();
    rest.is_empty() || rest.starts_with("//") || rest.starts_with('#')
}

/// The range that removes an element: its whole line when it is alone on it, else just the element.
fn element_removal(text: &str, span: &Range<usize>) -> Range<usize> {
    let alone = indent_before(text, span.start).is_some() && text.get(span.end..line_end(text, span.end)).is_some_and(|r| r.trim().is_empty());
    if alone { line_start(text, span.start)..after_line(text, span.end) } else { span.clone() }
}

/// Ranges that overlap or touch, as one.
fn merge(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|r| (r.start, r.end));
    let mut out: Vec<Range<usize>> = Vec::new();
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// The edits in the order they apply (inserts at one place in the order they were made), refused when two overlap.
fn normalize(mut edits: Vec<Edit>) -> Result<Vec<Edit>, GridError> {
    edits.sort_by_key(|e| (e.range.start, e.range.end));
    for pair in edits.windows(2) {
        if let [a, b] = pair
            && b.range.start < a.range.end
        {
            return fail("these changes overlap in the text; make them one at a time");
        }
    }
    Ok(edits)
}

/// The ranges that delete some entries of a list with commas between them (JSON, YAML flow): an entry takes the comma after it, or the
/// last ones the comma before them; all of them leave the brackets empty (`inner` is what is between them).
fn comma_removal(entries: &[Range<usize>], deleted: &[bool], inner: Range<usize>) -> Vec<Range<usize>> {
    if !entries.is_empty() && deleted.iter().take(entries.len()).filter(|d| **d).count() == entries.len() {
        return vec![inner];
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        if !deleted.get(i).copied().unwrap_or(false) {
            i += 1;
            continue;
        }
        let first = i;
        while i < entries.len() && deleted.get(i).copied().unwrap_or(false) {
            i += 1;
        }
        let last = i - 1;
        match (entries.get(first), entries.get(i), first.checked_sub(1).and_then(|p| entries.get(p)), entries.get(last)) {
            // A kept entry follows: from the first to it.
            (Some(f), Some(next), _, _) => out.push(f.start..next.start),
            // The run is at the end: from the end of the kept entry before it.
            (_, None, Some(before), Some(l)) => out.push(before.end..l.end),
            _ => {}
        }
    }
    out
}

/// The edits that insert `pieces` into a list with commas (the members of a JSON object or YAML flow mapping, the items of a JSON array
/// or YAML flow sequence): after entry `anchor`, or first when there is none. A list written one entry to a line gets one line each, with
/// the indentation of its entries and the comma where the list has it; a comment after an entry stays on its line.
fn comma_insert(
    text: &str,
    eol: &str,
    entries: &[Range<usize>],
    anchor: Option<usize>,
    container: &Range<usize>,
    pieces: &[String],
) -> Result<Vec<Edit>, GridError> {
    let open = text.get(container.clone()).ok_or_else(|| GridError("the document has changed since the grid was read".to_owned()))?;
    if !open.starts_with(['{', '[']) {
        return fail("the list is not written in brackets");
    }
    let first_indent = entries.first().and_then(|e| indent_before(text, e.start));
    let multiline = match entries.first() {
        Some(_) => first_indent.is_some(),
        None => open.contains(['\n', '\r']),
    };
    let indent = first_indent.map_or_else(|| format!("{}  ", line_indent(text, container.start)), str::to_owned);
    let joined = pieces.join(&format!(",{eol}{indent}"));
    let concat = |f: &dyn Fn(&str) -> String| pieces.iter().map(|p| f(p)).collect::<String>();
    let edits = match anchor {
        Some(i) => {
            let e = entries.get(i).ok_or_else(|| GridError("that entry is not there any more".to_owned()))?;
            if !multiline {
                return Ok(vec![Edit::insert(e.end, concat(&|p| format!(", {p}")))]);
            }
            let rest = text.get(e.end..).unwrap_or_default();
            let comma = e.end + (rest.len() - rest.trim_start_matches([' ', '\t']).len());
            if text.get(comma..).is_some_and(|r| r.starts_with(',')) {
                if rest_is_trivia(text, comma + 1) {
                    vec![Edit::insert(line_end(text, comma + 1), concat(&|p| format!("{eol}{indent}{p},")))]
                } else {
                    vec![Edit::insert(e.end, concat(&|p| format!(", {p}")))]
                }
            } else {
                let line_rest = text.get(e.end..line_end(text, e.end)).unwrap_or_default();
                if !line_rest.trim().is_empty() && rest_is_trivia(text, e.end) {
                    // `"a": 1 // note`: the comma goes after the value, the new entries after the comment.
                    vec![Edit::insert(e.end, ","), Edit::insert(line_end(text, e.end), format!("{eol}{indent}{joined}"))]
                } else {
                    vec![Edit::insert(e.end, format!(",{eol}{indent}{joined}"))]
                }
            }
        }
        None => match entries.first() {
            None if multiline => vec![Edit::insert(container.start + 1, format!("{eol}{indent}{joined}"))],
            None => vec![Edit::insert(container.start + 1, pieces.join(", "))],
            Some(f) if multiline => vec![Edit::insert(f.start, concat(&|p| format!("{p},{eol}{indent}")))],
            Some(f) => vec![Edit::insert(f.start, concat(&|p| format!("{p}, ")))],
        },
    };
    Ok(edits)
}

/// The start of a member: its name, or the value for an item.
fn entry_start(node: &Node) -> usize {
    if node.name_span.end > node.name_span.start { node.name_span.start } else { node.span.start }
}

// --- values ------------------------------------------------------------------------------------------------------------------

/// The literal a value typed into a data cell is written as: a number, boolean or null when it reads as one and `kind` is that type,
/// else a string (quoted and escaped as the format needs).
fn data_literal(family: Family, typed: &str, kind: RowKind) -> Result<String, GridError> {
    let t = typed.trim();
    let number = match family {
        Family::Yaml => is_yaml_number(t),
        _ => is_json_number(t),
    };
    match kind {
        RowKind::Number if number => Ok(t.to_owned()),
        RowKind::Bool if t == "true" || t == "false" => Ok(t.to_owned()),
        RowKind::Null if t == "null" => Ok("null".to_owned()),
        _ => match family {
            Family::Json => Ok(json_string(typed)),
            Family::Yaml => yaml_string(typed),
            Family::Xml => fail("an XML cell holds text"),
        },
    }
}

/// A literal written as it is (`12`, `true`, `null`, `"text"`), checked to be one value of the format.
fn raw_literal(family: Family, literal: &str) -> Result<String, GridError> {
    let t = literal.trim();
    let quoted_json = || {
        t.starts_with('"') && {
            let read = xmlcraft_json_syntax::parse(t, &xmlcraft_json_syntax::Options::default());
            read.diagnostics.iter().all(|d| d.severity() != xmlcraft_json_syntax::Severity::Error)
                && read.root.is_some_and(|r| matches!(r.data, xmlcraft_json_syntax::Data::String(_)))
        }
    };
    let ok = match family {
        Family::Json => t == "true" || t == "false" || t == "null" || is_json_number(t) || quoted_json(),
        Family::Yaml => {
            let single = t.len() >= 2
                && t.starts_with('\'')
                && t.ends_with('\'')
                && t.get(1..t.len() - 1).is_some_and(|inner| !inner.replace("''", "").contains('\'') && !inner.contains(['\n', '\r']));
            matches!(t, "true" | "false" | "null" | "~") || is_yaml_number(t) || quoted_json() || single
        }
        Family::Xml => false,
    };
    if !ok {
        return fail("that is not a value written as it is: a number, true, false, null or a string in quotes");
    }
    Ok(t.to_owned())
}

/// The value of an empty new cell: an empty string in a column of strings, null in the others.
fn empty_literal(family: Family, kind: Option<RowKind>) -> String {
    match (family, kind) {
        (_, None | Some(RowKind::String)) => "\"\"".to_owned(),
        _ => "null".to_owned(),
    }
}

fn data_key(family: Family, key: &str) -> Result<String, GridError> {
    match family {
        Family::Yaml => yaml_string(key),
        _ => Ok(json_string(key)),
    }
}

/// The extended JSON wrapper's value, as the table shows it.
fn wrapped(flavor: Flavor, node: &Node) -> &Node {
    if flavor == Flavor::ExtendedJson
        && node.kind == NodeKind::Object
        && let [only] = node.children.as_slice()
        && only.name.starts_with('$')
        && matches!(only.kind, NodeKind::String | NodeKind::Number | NodeKind::Bool)
    {
        return only;
    }
    node
}

// --- the context of an edit -------------------------------------------------------------------------------------------------

/// What every edit of one table needs: the text, its line ending, the columns, the layout and the node of the table.
pub(crate) struct Ctx<'a> {
    text: &'a str,
    eol: &'static str,
    columns: &'a [Column],
    layout: Layout,
    container: &'a Node,
    /// One step of XML indentation (from the container to its items), for content written into an empty item.
    unit: String,
    family: Family,
    flavor: Flavor,
    item_name: Option<&'a str>,
}

impl Ctx<'_> {
    fn is_yaml_flow(&self, node: &Node) -> bool {
        self.text.get(node.span.start..).is_some_and(|t| t.starts_with('{'))
    }

    /// The item in the text: the YAML item without the white space after it.
    fn extent(&self, node: &Node) -> Range<usize> {
        match self.family {
            Family::Yaml => node.span.start..trimmed_end(self.text, &node.span).max(node.span.start),
            _ => node.span.clone(),
        }
    }

    /// The `- ` before a YAML block sequence item (with the indentation before it).
    fn dash_prefix(&self, node: &Node) -> Result<&str, GridError> {
        let prefix = self.text.get(line_start(self.text, node.span.start)..node.span.start).unwrap_or_default();
        if prefix.trim() != "-" {
            return fail("the item is not on a line of its own after \"- \"; edit it in the text");
        }
        Ok(prefix)
    }
}

impl Grid {
    pub(crate) fn ctx<'a>(
        &'a self,
        text: &'a str,
        columns: &'a [Column],
        layout: Layout,
        container: &'a Node,
        item_name: Option<&'a str>,
    ) -> Ctx<'a> {
        let first_item = container.children.iter().find(|c| match item_name {
            Some(n) => c.kind == NodeKind::Element && c.name == n,
            None => true,
        });
        let unit = match (indent_before(text, container.span.start), first_item.and_then(|f| indent_before(text, f.span.start))) {
            (Some(outer), Some(inner)) => inner.strip_prefix(outer).filter(|u| !u.is_empty()).unwrap_or("  ").to_owned(),
            _ => "  ".to_owned(),
        };
        Ctx {
            text,
            eol: LineEndings::count(text).dominant().as_str(),
            columns,
            layout,
            container,
            unit,
            family: self.family,
            flavor: self.flavor,
            item_name,
        }
    }

    /// The context of `table`, checked against the text and the tree.
    fn table_ctx<'a>(&'a self, text: &'a str, table: &'a Table) -> Result<Ctx<'a>, GridError> {
        let container = self.node(&table.path)?;
        self.slice(text, &container.span)?;
        if table.family != self.family {
            return fail("the table is of another document");
        }
        Ok(self.ctx(text, &table.columns, table.layout, container, table.item_name.as_deref()))
    }

    /// The item of row `row`, checked to be where the table says.
    fn item_of<'a>(&self, ctx: &Ctx<'a>, table: &Table, row: usize) -> Result<&'a Node, GridError> {
        let r = table.rows.get(row).ok_or_else(|| GridError(format!("there is no row {}", row + 1)))?;
        let index = r.path.last().copied().ok_or_else(|| GridError("the row has no place".to_owned()))?;
        let node = ctx.container.children.get(index).ok_or_else(|| GridError("that row is not there any more".to_owned()))?;
        if node.span != r.span {
            return fail("the table is out of date; read it again");
        }
        Ok(node)
    }

    /// Where a value written into the empty cell `column` of `item` would go (none when it cannot be written there).
    pub(crate) fn insertion_point(&self, ctx: &Ctx<'_>, item: &Node, column: usize) -> Option<usize> {
        let edits = self.create_cells(ctx, item, &[(column, "x".to_owned())], false).ok()?;
        edits.last().map(|e| e.range.start)
    }

    // --- cells ---------------------------------------------------------------------------------------------------------------

    /// The edits that make the cell say `value`: the scalar replaced (a data value keeps its type when it reads as one, else becomes a
    /// string), or an empty cell created (the attribute, element or member written after the column before it that the row has). An
    /// empty value in an empty cell changes nothing; a nested cell is opened, not overwritten.
    pub fn set_cell(&self, text: &str, table: &Table, row: usize, column: usize, value: &str) -> Result<Vec<Edit>, GridError> {
        let ctx = self.table_ctx(text, table)?;
        normalize(self.row_edits(&ctx, table, row, &[(column, value.to_owned())], false, None)?)
    }

    /// [`Grid::set_cell`] with a value written as it is (`12`, `true`, `null`, `"text"`): JSON and YAML.
    pub fn set_cell_literal(&self, text: &str, table: &Table, row: usize, column: usize, literal: &str) -> Result<Vec<Edit>, GridError> {
        if self.family == Family::Xml {
            return fail("a value written as it is is for JSON and YAML; XML cells hold text");
        }
        let ctx = self.table_ctx(text, table)?;
        normalize(self.row_edits(&ctx, table, row, &[(column, literal.to_owned())], true, None)?)
    }

    fn row_edits(
        &self,
        ctx: &Ctx<'_>,
        table: &Table,
        row: usize,
        pairs: &[(usize, String)],
        raw: bool,
        mut skipped: Option<&mut usize>,
    ) -> Result<Vec<Edit>, GridError> {
        let item = self.item_of(ctx, table, row)?;
        let cells = &table.rows.get(row).ok_or_else(|| GridError("no such row".to_owned()))?.cells;
        let mut edits = Vec::new();
        let mut new: Vec<(usize, String)> = Vec::new();
        for (column, value) in pairs {
            let cell = cells.get(*column).ok_or_else(|| GridError(format!("there is no column {}", column + 1)))?;
            match cell.kind {
                CellKind::Scalar => {
                    if !raw && *value == cell.text {
                        continue;
                    }
                    edits.extend(self.replace_scalar(ctx, item, *column, cell.value_kind, value, raw)?);
                }
                CellKind::Nested => match skipped.as_deref_mut() {
                    Some(n) => *n += 1,
                    None => return fail(format!("this cell holds {}: open it to change what is in it", cell.text)),
                },
                CellKind::Missing => {
                    if !value.is_empty() || raw {
                        new.push((*column, value.clone()));
                    }
                }
            }
        }
        if !new.is_empty() {
            edits.extend(self.create_cells(ctx, item, &new, raw)?);
        }
        Ok(edits)
    }

    fn replace_scalar(&self, ctx: &Ctx<'_>, item: &Node, column: usize, kind: RowKind, value: &str, raw: bool) -> Result<Vec<Edit>, GridError> {
        let col = ctx.columns.get(column).ok_or_else(|| GridError("no such column".to_owned()))?;
        let text = ctx.text;
        match col.kind {
            ColumnKind::Attribute => {
                let a = item
                    .children
                    .iter()
                    .find(|c| c.kind == NodeKind::Attribute && c.name == col.name)
                    .ok_or_else(|| GridError("the attribute is not there any more".to_owned()))?;
                self.set_xml_value(text, a, value)
            }
            ColumnKind::Text => self.set_xml_value(text, item, value),
            ColumnKind::Element => {
                let e = item
                    .children
                    .iter()
                    .find(|c| c.kind == NodeKind::Element && c.name == col.name)
                    .ok_or_else(|| GridError("the element is not there any more".to_owned()))?;
                self.set_xml_value(text, e, value)
            }
            ColumnKind::Member | ColumnKind::Value => {
                let node = if col.kind == ColumnKind::Value {
                    item
                } else {
                    item.children.iter().find(|m| m.name == col.name).ok_or_else(|| GridError("the member is not there any more".to_owned()))?
                };
                let inner = wrapped(ctx.flavor, node);
                if ctx.family == Family::Yaml && yaml_block_scalar(text, inner) {
                    return fail("a block scalar is edited in the text");
                }
                let target = if kind == RowKind::Null { col.value_kind.unwrap_or(RowKind::String) } else { kind };
                let literal = if raw { raw_literal(ctx.family, value)? } else { data_literal(ctx.family, value, target)? };
                Ok(vec![Edit::replace(inner.span.clone(), literal)])
            }
        }
    }

    /// The edits that write the empty cells `new` (column, value) of `item`, each after the nearest column to its left that the row has,
    /// those after the same one together in column order.
    fn create_cells(&self, ctx: &Ctx<'_>, item: &Node, new: &[(usize, String)], raw: bool) -> Result<Vec<Edit>, GridError> {
        let text = ctx.text;
        let eol = ctx.eol;
        let column = |i: usize| ctx.columns.get(i).ok_or_else(|| GridError("no such column".to_owned()));
        // The nearest column to the left of `i` of the kinds `kinds` that `item` has, as the index of its (last) node.
        let anchor = |i: usize, kinds: &[ColumnKind]| -> Option<usize> {
            ctx.columns.get(..i)?.iter().rev().filter(|c| kinds.contains(&c.kind)).find_map(|c| {
                item.children.iter().enumerate().rev().find_map(|(k, n)| {
                    let hit = match c.kind {
                        ColumnKind::Attribute => n.kind == NodeKind::Attribute && n.name == c.name,
                        ColumnKind::Element => n.kind == NodeKind::Element && n.name == c.name,
                        ColumnKind::Member => n.name == c.name,
                        _ => false,
                    };
                    hit.then_some(k)
                })
            })
        };
        // Pieces after the same anchor, in the order they come.
        let mut groups: Vec<(ColumnKind, Option<usize>, Vec<String>)> = Vec::new();
        let mut push = |kind: ColumnKind, at: Option<usize>, piece: String| match groups.iter_mut().find(|(k, a, _)| *k == kind && *a == at) {
            Some((_, _, pieces)) => pieces.push(piece),
            None => groups.push((kind, at, vec![piece])),
        };
        let mut edits = Vec::new();
        for (i, value) in new {
            let col = column(*i)?;
            match col.kind {
                ColumnKind::Attribute => {
                    if raw {
                        return fail("XML cells hold text");
                    }
                    if !is_xml_name(&col.name) {
                        return fail(format!("{:?} is not a name an attribute can have", col.name));
                    }
                    push(ColumnKind::Attribute, anchor(*i, &[ColumnKind::Attribute]), format!(" {}=\"{}\"", col.name, escape_xml(value, Some('"'))));
                }
                ColumnKind::Text => {
                    if raw {
                        return fail("XML cells hold text");
                    }
                    if text_only(item).is_none() {
                        return fail("this item holds elements: its text is edited in the text view");
                    }
                    edits.extend(self.set_xml_value(text, item, value)?);
                }
                ColumnKind::Element => {
                    if raw {
                        return fail("XML cells hold text");
                    }
                    if !is_xml_name(&col.name) {
                        return fail(format!("{:?} is not a name an element can have", col.name));
                    }
                    if matches!(text_only(item), Some(Some(t)) if !t.text.trim().is_empty()) {
                        return fail("this item holds text; an element in it would mix the two (edit it in the text view)");
                    }
                    push(ColumnKind::Element, anchor(*i, &[ColumnKind::Element]), format!("<{0}>{1}</{0}>", col.name, escape_xml(value, None)));
                }
                ColumnKind::Member => {
                    if item.kind != NodeKind::Object {
                        return fail("the item is not an object");
                    }
                    let literal = if raw {
                        raw_literal(ctx.family, value)?
                    } else {
                        data_literal(ctx.family, value, col.value_kind.unwrap_or(RowKind::String))?
                    };
                    let key = data_key(ctx.family, &col.name)?;
                    push(ColumnKind::Member, anchor(*i, &[ColumnKind::Member]), format!("{key}: {literal}"));
                }
                ColumnKind::Value => return fail("an item of an array of values is added as a row"),
            }
        }
        for (kind, at, pieces) in groups {
            match kind {
                ColumnKind::Attribute => {
                    let offset = match at.and_then(|k| item.children.get(k)) {
                        Some(a) => a.span.end,
                        None => item.name_span.end,
                    };
                    edits.push(Edit::insert(offset, pieces.concat()));
                }
                ColumnKind::Element => edits.extend(self.insert_elements(ctx, item, at, &pieces)?),
                ColumnKind::Member => {
                    if ctx.family == Family::Json || ctx.is_yaml_flow(item) {
                        let entries: Vec<Range<usize>> = item.children.iter().map(|m| entry_start(m)..ctx.extent(m).end).collect();
                        edits.extend(comma_insert(text, eol, &entries, at, &item.span, &pieces)?);
                    } else {
                        let first = item.children.first().ok_or_else(|| GridError("the item has no members to follow".to_owned()))?;
                        let key_start = entry_start(first);
                        let pad = " ".repeat(text.get(line_start(text, key_start)..key_start).unwrap_or_default().chars().count());
                        match at.and_then(|k| item.children.get(k)) {
                            Some(m) => {
                                let end = ctx.extent(m).end.max(m.name_span.end);
                                edits.push(Edit::insert(line_end(text, end), pieces.iter().map(|p| format!("{eol}{pad}{p}")).collect::<String>()));
                            }
                            None => edits.push(Edit::insert(key_start, pieces.iter().map(|p| format!("{p}{eol}{pad}")).collect::<String>())),
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(edits)
    }

    /// The edits that write the elements `pieces` into `item`: after its child `at`, else before its first element, else as its content.
    fn insert_elements(&self, ctx: &Ctx<'_>, item: &Node, at: Option<usize>, pieces: &[String]) -> Result<Vec<Edit>, GridError> {
        let (text, eol) = (ctx.text, ctx.eol);
        if let Some(a) = at.and_then(|k| item.children.get(k)) {
            let piece = |p: &String| match indent_before(text, a.span.start) {
                Some(i) => format!("{eol}{i}{p}"),
                None => p.clone(),
            };
            return Ok(vec![Edit::insert(a.span.end, pieces.iter().map(piece).collect::<String>())]);
        }
        if let Some(f) = item.children.iter().find(|c| c.kind == NodeKind::Element) {
            let piece = |p: &String| match indent_before(text, f.span.start) {
                Some(i) => format!("{p}{eol}{i}"),
                None => p.clone(),
            };
            return Ok(vec![Edit::insert(f.span.start, pieces.iter().map(piece).collect::<String>())]);
        }
        // No element yet: the pieces are the content, one level deeper when the item is on a line of its own.
        let content = match indent_before(text, item.span.start) {
            Some(i) => {
                let inner = format!("{i}{}", ctx.unit);
                format!("{eol}{inner}{}{eol}{i}", pieces.join(&format!("{eol}{inner}")))
            }
            None => pieces.concat(),
        };
        let whole = self.slice(text, &item.span)?;
        if whole.ends_with("/>") {
            let close = item.span.end.saturating_sub(2);
            return Ok(vec![Edit::replace(close..item.span.end, format!(">{content}</{}>", item.name))]);
        }
        let after_attributes = item.children.iter().filter(|c| c.kind == NodeKind::Attribute).map(|a| a.span.end).max().unwrap_or(item.name_span.end);
        let open_end = text
            .get(after_attributes..item.span.end)
            .and_then(|r| r.find('>'))
            .map(|i| after_attributes + i + 1)
            .ok_or_else(|| GridError("the start tag cannot be found in the text".to_owned()))?;
        let close_start = whole
            .rfind("</")
            .map(|i| item.span.start + i)
            .filter(|c| *c >= open_end)
            .ok_or_else(|| GridError("the end tag cannot be found in the text".to_owned()))?;
        if text.get(open_end..close_start).is_none_or(|c| !c.trim().is_empty()) {
            return fail("the item holds text or comments; add the element in the text view");
        }
        Ok(vec![Edit::replace(open_end..close_start, content)])
    }

    /// The edits that remove the cell `column` of `item` (the attribute with the space before it; the elements, each with its line when it
    /// is alone on it; the member with its comma, or its line).
    fn clear_edits(&self, ctx: &Ctx<'_>, item: &Node, column: usize) -> Result<Vec<Edit>, GridError> {
        let text = ctx.text;
        let col = ctx.columns.get(column).ok_or_else(|| GridError("no such column".to_owned()))?;
        let ranges: Vec<Range<usize>> = match col.kind {
            ColumnKind::Attribute => item
                .children
                .iter()
                .filter(|c| c.kind == NodeKind::Attribute && c.name == col.name)
                .map(|a| {
                    let before = text.get(..a.span.start).unwrap_or_default();
                    (a.span.start - (before.len() - before.trim_end().len()))..a.span.end
                })
                .collect(),
            ColumnKind::Element => {
                item.children.iter().filter(|c| c.kind == NodeKind::Element && c.name == col.name).map(|e| element_removal(text, &e.span)).collect()
            }
            ColumnKind::Text => match text_only(item) {
                Some(Some(t)) => vec![t.span.clone()],
                _ => Vec::new(),
            },
            ColumnKind::Value => return fail("an item of an array of values is deleted as a row"),
            ColumnKind::Member => {
                let Some(k) = item.children.iter().position(|m| m.name == col.name) else { return Ok(Vec::new()) };
                let member = item.children.get(k).ok_or_else(|| GridError("no such member".to_owned()))?;
                if ctx.family == Family::Json || ctx.is_yaml_flow(item) {
                    let entries: Vec<Range<usize>> = item.children.iter().map(|m| entry_start(m)..ctx.extent(m).end).collect();
                    let deleted: Vec<bool> = (0..entries.len()).map(|i| i == k).collect();
                    let inner = item.span.start + 1..item.span.end.saturating_sub(1).max(item.span.start + 1);
                    comma_removal(&entries, &deleted, inner)
                } else {
                    let key_start = entry_start(member);
                    let start = line_start(text, key_start);
                    let end = ctx.extent(member).end.max(member.name_span.end);
                    if text.get(start..key_start).is_some_and(|p| p.trim().is_empty()) {
                        vec![Range { start, end: after_line(text, end) }]
                    } else {
                        // The first member, after `- `: the next one takes its place, or the item becomes `{}`.
                        match item.children.get(k + 1) {
                            Some(next) => vec![Range { start: key_start, end: entry_start(next) }],
                            None => return Ok(vec![Edit::replace(key_start..end, "{}")]),
                        }
                    }
                }
            }
        };
        Ok(merge(ranges).into_iter().map(Edit::delete).collect())
    }

    /// The edits that remove the cell (the attribute, the elements of the column, the member). An empty cell changes nothing.
    pub fn clear_cell(&self, text: &str, table: &Table, row: usize, column: usize) -> Result<Vec<Edit>, GridError> {
        let ctx = self.table_ctx(text, table)?;
        let item = self.item_of(&ctx, table, row)?;
        normalize(self.clear_edits(&ctx, item, column)?)
    }

    // --- rows ----------------------------------------------------------------------------------------------------------------

    /// The text of a new item like the item of row `neighbour`: the same element name, the columns every row has (empty) and the columns
    /// `values` gives a value, written in the neighbour's layout.
    fn new_item(&self, ctx: &Ctx<'_>, table: &Table, neighbour: usize, values: &[(usize, String)]) -> Result<String, GridError> {
        let (text, eol) = (ctx.text, ctx.eol);
        let nb = self.item_of(ctx, table, neighbour)?;
        let value_of = |c: usize| values.iter().find(|(k, _)| *k == c).map(|(_, v)| v.as_str()).filter(|v| !v.is_empty());
        let every = |c: usize| table.rows.iter().all(|r| r.cells.get(c).is_some_and(|x| x.kind != CellKind::Missing));
        let chosen: Vec<(&Column, Option<&str>)> =
            ctx.columns.iter().enumerate().filter(|(c, _)| every(*c) || value_of(*c).is_some()).map(|(c, col)| (col, value_of(c))).collect();
        match ctx.family {
            Family::Xml => {
                let name = ctx.item_name.unwrap_or(&nb.name);
                let mut attributes = String::new();
                let mut elements = Vec::new();
                let mut own_text = None;
                for (col, value) in &chosen {
                    let v = value.unwrap_or_default();
                    match col.kind {
                        ColumnKind::Attribute => attributes.push_str(&format!(" {}=\"{}\"", col.name, escape_xml(v, Some('"')))),
                        ColumnKind::Element => elements.push(format!("<{0}>{1}</{0}>", col.name, escape_xml(v, None))),
                        ColumnKind::Text => own_text = Some(escape_xml(v, None)),
                        _ => {}
                    }
                }
                if elements.is_empty() {
                    return Ok(match own_text {
                        Some(t) => format!("<{name}{attributes}>{t}</{name}>"),
                        None => format!("<{name}{attributes}/>"),
                    });
                }
                let child_indent = nb.children.iter().find(|c| c.kind == NodeKind::Element).and_then(|c| indent_before(text, c.span.start));
                Ok(match (indent_before(text, nb.span.start), child_indent) {
                    (Some(i), Some(ci)) => {
                        format!("<{name}{attributes}>{eol}{ci}{}{eol}{i}</{name}>", elements.join(&format!("{eol}{ci}")))
                    }
                    _ => format!("<{name}{attributes}>{}</{name}>", elements.concat()),
                })
            }
            Family::Json | Family::Yaml => {
                let literal = |col: &Column, value: Option<&str>| match value {
                    Some(v) => data_literal(ctx.family, v, col.value_kind.unwrap_or(RowKind::String)),
                    None => Ok(empty_literal(ctx.family, col.value_kind)),
                };
                if let Some(col) = ctx.columns.iter().find(|c| c.kind == ColumnKind::Value) {
                    return literal(col, value_of(0));
                }
                let members = chosen
                    .iter()
                    .map(|(col, value)| Ok(format!("{}: {}", data_key(ctx.family, &col.name)?, literal(col, *value)?)))
                    .collect::<Result<Vec<String>, GridError>>()?;
                if members.is_empty() {
                    return Ok("{}".to_owned());
                }
                let flow = ctx.family == Family::Json || ctx.is_yaml_flow(nb) || ctx.layout == Layout::YamlFlow;
                if !flow {
                    let prefix_width = text.get(line_start(text, nb.span.start)..nb.span.start).unwrap_or_default().chars().count();
                    return Ok(members.join(&format!("{eol}{}", " ".repeat(prefix_width))));
                }
                let member_indent = nb.children.first().and_then(|m| indent_before(text, entry_start(m)));
                match member_indent {
                    Some(mi) if ctx.layout != Layout::JsonLines => {
                        let outer = line_indent(text, nb.span.start);
                        Ok(format!("{{{eol}{mi}{}{eol}{outer}}}", members.join(&format!(",{eol}{mi}"))))
                    }
                    _ => Ok(format!("{{{}}}", members.join(", "))),
                }
            }
        }
    }

    /// The edits that insert the items `items` (texts) after row `after`, or before the first row.
    fn insert_items(&self, ctx: &Ctx<'_>, table: &Table, after: Option<usize>, items: &[String]) -> Result<Vec<Edit>, GridError> {
        let (text, eol) = (ctx.text, ctx.eol);
        let node = self.item_of(ctx, table, after.unwrap_or(0))?;
        let extent = ctx.extent(node);
        let edit = match ctx.layout {
            Layout::XmlElements => {
                let indent = indent_before(text, node.span.start);
                match (after, indent) {
                    (Some(_), Some(i)) => Edit::insert(extent.end, items.iter().map(|n| format!("{eol}{i}{n}")).collect::<String>()),
                    (Some(_), None) => Edit::insert(extent.end, items.concat()),
                    (None, Some(i)) => Edit::insert(extent.start, items.iter().map(|n| format!("{n}{eol}{i}")).collect::<String>()),
                    (None, None) => Edit::insert(extent.start, items.concat()),
                }
            }
            Layout::JsonArray | Layout::YamlFlow => {
                let entries: Vec<Range<usize>> = ctx.container.children.iter().map(|c| ctx.extent(c)).collect();
                let anchor = match after {
                    Some(r) => Some(table.rows.get(r).and_then(|r| r.path.last().copied()).ok_or_else(|| GridError("no such row".to_owned()))?),
                    None => None,
                };
                return comma_insert(text, eol, &entries, anchor, &ctx.container.span, items);
            }
            Layout::JsonLines => match after {
                Some(_) => Edit::insert(line_end(text, extent.end), items.iter().map(|n| format!("{eol}{n}")).collect::<String>()),
                None => Edit::insert(line_start(text, extent.start), items.iter().map(|n| format!("{n}{eol}")).collect::<String>()),
            },
            Layout::YamlBlock => {
                let prefix = ctx.dash_prefix(node)?;
                match after {
                    Some(_) => Edit::insert(line_end(text, extent.end), items.iter().map(|n| format!("{eol}{prefix}{n}")).collect::<String>()),
                    None => Edit::insert(extent.start, items.iter().map(|n| format!("{n}{eol}{prefix}")).collect::<String>()),
                }
            }
            Layout::YamlDocuments => return fail("the documents of a YAML stream are added in the text"),
        };
        Ok(vec![edit])
    }

    /// The edits that insert a new row after row `after` (or before the first row): an item with the shape of its neighbour (the same
    /// element name, the columns every row has, with empty values).
    pub fn insert_row(&self, text: &str, table: &Table, after: Option<usize>) -> Result<Vec<Edit>, GridError> {
        let ctx = self.table_ctx(text, table)?;
        let item = self.new_item(&ctx, table, after.unwrap_or(0), &[])?;
        normalize(self.insert_items(&ctx, table, after, &[item])?)
    }

    /// The edits that insert a copy of row `row` after it.
    pub fn duplicate_row(&self, text: &str, table: &Table, row: usize) -> Result<Vec<Edit>, GridError> {
        let ctx = self.table_ctx(text, table)?;
        let node = self.item_of(&ctx, table, row)?;
        let copy = self.slice(text, &ctx.extent(node))?.to_owned();
        normalize(self.insert_items(&ctx, table, Some(row), &[copy])?)
    }

    /// The edits that delete the rows `rows` (each item with its line or its comma).
    pub fn delete_rows(&self, text: &str, table: &Table, rows: &[usize]) -> Result<Vec<Edit>, GridError> {
        let ctx = self.table_ctx(text, table)?;
        let mut items = Vec::new();
        for &r in rows {
            items.push((r, self.item_of(&ctx, table, r)?));
        }
        let ranges: Vec<Range<usize>> = match ctx.layout {
            Layout::XmlElements => items.iter().map(|(_, n)| element_removal(text, &n.span)).collect(),
            Layout::JsonArray | Layout::YamlFlow => {
                let entries: Vec<Range<usize>> = ctx.container.children.iter().map(|c| ctx.extent(c)).collect();
                let mut deleted = vec![false; entries.len()];
                for (r, _) in &items {
                    if let Some(d) = table.rows.get(*r).and_then(|row| row.path.last()).and_then(|i| deleted.get_mut(*i)) {
                        *d = true;
                    }
                }
                let span = &ctx.container.span;
                comma_removal(&entries, &deleted, span.start + 1..span.end.saturating_sub(1).max(span.start + 1))
            }
            Layout::JsonLines => items
                .iter()
                .map(|(_, n)| {
                    let start = line_start(text, n.span.start);
                    let end = after_line(text, n.span.end);
                    // The last line, with no line break after it, takes the break before it.
                    if end == line_end(text, n.span.end) && start > 0 {
                        let before = text.get(..start).unwrap_or_default();
                        let cut = if before.ends_with("\r\n") { 2 } else { 1 };
                        start.saturating_sub(cut)..end
                    } else {
                        start..end
                    }
                })
                .collect(),
            Layout::YamlBlock => {
                if items.len() == ctx.container.children.len() {
                    // All of them: the sequence becomes empty, not null.
                    let whole = ctx.extent(ctx.container);
                    return Ok(vec![Edit::replace(whole, "[]")]);
                }
                let mut out = Vec::new();
                for (_, n) in &items {
                    ctx.dash_prefix(n)?;
                    out.push(line_start(text, n.span.start)..after_line(text, ctx.extent(n).end));
                }
                out
            }
            Layout::YamlDocuments => return fail("the documents of a YAML stream are removed in the text"),
        };
        normalize(merge(ranges).into_iter().map(Edit::delete).collect())
    }

    /// The edits that put the items in `order` (order[i] is the row that goes to place i): each place whose item changes gets the other
    /// item's text; what is between the items (white space, comments, other elements) stays where it is. One undoable step.
    pub fn apply_order(&self, text: &str, table: &Table, order: &[usize]) -> Result<Vec<Edit>, GridError> {
        let ctx = self.table_ctx(text, table)?;
        if !table.is_whole() {
            return fail(format!("the table is cut at {} of {} rows, so its rows are put in order in the text", table.rows.len(), table.total_rows));
        }
        if order.len() != table.rows.len() {
            return fail("the order names every row once");
        }
        let mut seen = vec![false; order.len()];
        for &r in order {
            match seen.get_mut(r) {
                Some(s) if !*s => *s = true,
                _ => return fail("the order names every row once"),
            }
        }
        let nodes = (0..table.rows.len()).map(|r| self.item_of(&ctx, table, r)).collect::<Result<Vec<_>, _>>()?;
        if ctx.layout == Layout::YamlBlock {
            let width = |n: &Node| n.span.start - line_start(text, n.span.start);
            if nodes.windows(2).any(|w| matches!(w, [a, b] if width(a) != width(b))) {
                return fail("the items do not all start in the same column; put them in order in the text");
            }
        }
        let mut edits = Vec::new();
        for (place, &from) in order.iter().enumerate() {
            if place == from {
                continue;
            }
            let (Some(slot), Some(source)) = (nodes.get(place), nodes.get(from)) else { continue };
            edits.push(Edit::replace(ctx.extent(slot), self.slice(text, &ctx.extent(source))?.to_owned()));
        }
        normalize(edits)
    }

    /// The edits that move row `row` to before row `before` (to the end when none).
    pub fn move_row(&self, text: &str, table: &Table, row: usize, before: Option<usize>) -> Result<Vec<Edit>, GridError> {
        let n = table.rows.len();
        if row >= n || before.is_some_and(|b| b >= n) {
            return fail("there is no such row");
        }
        if before == Some(row) {
            return Ok(Vec::new());
        }
        let mut order: Vec<usize> = (0..n).filter(|&r| r != row).collect();
        let at = before.and_then(|b| order.iter().position(|&r| r == b)).unwrap_or(order.len());
        order.insert(at, row);
        self.apply_order(text, table, &order)
    }

    // --- columns -------------------------------------------------------------------------------------------------------------

    /// The table with a new column `name` after column `after` (first when none). Nothing in the text changes until a cell of it gets a
    /// value. For XML `@name` is an attribute, `#text` the text, anything else an element.
    pub fn insert_column(&self, text: &str, table: &Table, name: &str, after: Option<usize>) -> Result<Table, GridError> {
        let (kind, bare) = match self.family {
            Family::Xml => match name.strip_prefix('@') {
                Some(a) => (ColumnKind::Attribute, a),
                None if name == "#text" => (ColumnKind::Text, name),
                None => (ColumnKind::Element, name),
            },
            Family::Json | Family::Yaml => {
                if table.columns.iter().any(|c| c.kind == ColumnKind::Value) {
                    return fail("an array of values has one column");
                }
                (ColumnKind::Member, name)
            }
        };
        if kind != ColumnKind::Text && kind != ColumnKind::Member && !is_xml_name(bare) {
            return fail(format!("{bare:?} is not a name an XML attribute or element can have"));
        }
        if bare.is_empty() {
            return fail("a column needs a name");
        }
        if table.columns.iter().any(|c| c.kind == kind && c.name == bare) {
            return fail(format!("there is a column {name} already"));
        }
        let mut columns = table.columns.clone();
        let at = after.map_or(0, |a| a.saturating_add(1)).min(columns.len());
        columns.insert(at, Column { name: bare.to_owned(), kind, numeric: false, value_kind: None });
        self.table_with_columns(text, table, columns)
    }

    /// The edits that rename column `column` in every row (one undoable step). A row that has the new name already is refused.
    pub fn rename_column(&self, text: &str, table: &Table, column: usize, new_name: &str) -> Result<Vec<Edit>, GridError> {
        let ctx = self.table_ctx(text, table)?;
        let col = table.columns.get(column).ok_or_else(|| GridError("there is no such column".to_owned()))?;
        let new_name = if col.kind == ColumnKind::Attribute { new_name.strip_prefix('@').unwrap_or(new_name) } else { new_name };
        if new_name == col.name {
            return Ok(Vec::new());
        }
        if matches!(col.kind, ColumnKind::Text | ColumnKind::Value) {
            return fail("this column has no name in the text");
        }
        let mut edits = Vec::new();
        for r in 0..table.rows.len() {
            let item = self.item_of(&ctx, table, r)?;
            let matches = |n: &Node, name: &str| match col.kind {
                ColumnKind::Attribute => n.kind == NodeKind::Attribute && n.name == name,
                ColumnKind::Element => n.kind == NodeKind::Element && n.name == name,
                _ => n.name == name,
            };
            if item.children.iter().any(|n| matches(n, new_name)) {
                return fail(format!("row {} has {new_name} already", r + 1));
            }
            for n in item.children.iter().filter(|n| matches(n, &col.name)) {
                edits.extend(self.rename_node(text, n, new_name)?);
            }
        }
        normalize(edits)
    }

    /// The edits that delete column `column` from every row (one undoable step).
    pub fn delete_column(&self, text: &str, table: &Table, column: usize) -> Result<Vec<Edit>, GridError> {
        let ctx = self.table_ctx(text, table)?;
        let mut edits = Vec::new();
        for r in 0..table.rows.len() {
            let item = self.item_of(&ctx, table, r)?;
            edits.extend(self.clear_edits(&ctx, item, column)?);
        }
        normalize(edits)
    }

    // --- paste ---------------------------------------------------------------------------------------------------------------

    /// The edits that paste tab-separated text with its first cell at (`row`, `column`): cells set as typed values are, rows added at the
    /// end as needed (like the last row); cells past the last column and nested cells are left alone and counted. One undoable step.
    pub fn paste_tsv(&self, text: &str, table: &Table, row: usize, column: usize, tsv: &str) -> Result<Pasted, GridError> {
        let ctx = self.table_ctx(text, table)?;
        let grid = parse_tsv(tsv)?;
        if row > table.rows.len() || column >= table.columns.len() {
            return fail("the paste starts outside the table");
        }
        let mut pasted = Pasted { edits: Vec::new(), cells: 0, skipped: 0, appended: 0 };
        let mut new_items = Vec::new();
        for (i, values) in grid.iter().enumerate() {
            let mut pairs = Vec::new();
            for (j, value) in values.iter().enumerate() {
                match column.checked_add(j).filter(|c| *c < table.columns.len()) {
                    Some(c) => pairs.push((c, value.clone())),
                    None => pasted.skipped += 1,
                }
            }
            let target = row.saturating_add(i);
            if target < table.rows.len() {
                let before = pasted.skipped;
                pasted.edits.extend(self.row_edits(&ctx, table, target, &pairs, false, Some(&mut pasted.skipped))?);
                pasted.cells += pairs.len().saturating_sub(pasted.skipped - before);
            } else {
                if table.rows.is_empty() {
                    return fail("the table has no row to follow");
                }
                pasted.cells += pairs.iter().filter(|(_, v)| !v.is_empty()).count();
                new_items.push(self.new_item(&ctx, table, table.rows.len() - 1, &pairs)?);
            }
        }
        if !new_items.is_empty() {
            if !table.is_whole() {
                return fail("the table is cut, so rows are not added after its end");
            }
            pasted.appended = new_items.len();
            pasted.edits.extend(self.insert_items(&ctx, table, Some(table.rows.len() - 1), &new_items)?);
        }
        pasted.edits = normalize(pasted.edits)?;
        Ok(pasted)
    }
}
