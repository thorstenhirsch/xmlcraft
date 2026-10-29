//! Identity constraints during validation: `unique`, `key` and `keyref` (XML Schema Part 1, 3.11.4). An element that declares a
//! constraint is its context. While the document is read, the nodes its selector reaches become *rows*, and the nodes the fields
//! reach below a row become the values of the row; when the context element ends, the rows are checked against each other (and a
//! keyref against the keys its context and the elements below it have made).

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use xmlcraft_datatypes::{NamespaceContext, Value};
use xmlcraft_syntax::Code;
use xmlcraft_xsd::model::{IdentityKind, QName};
use xmlcraft_xsd::schema::{ConstraintId, IdentityConstraint};

use crate::xsd_validator::{Content, Frame, XsdAttribute, XsdValidator};

/// A constraint of the element being read, with the nodes its selector reached.
pub(crate) struct Run {
    constraint: ConstraintId,
    rows: Vec<Row>,
}

/// A node the selector reached, and the values its fields found.
struct Row {
    /// How deep the node is (the index of its frame).
    depth: usize,
    /// Where its name is written, for the problems found in the row.
    range: Range<usize>,
    fields: Vec<Field>,
}

#[derive(Clone, Default)]
struct Field {
    /// How many nodes the field selected.
    count: usize,
    /// The value of the one node: the key that equality is looked up by, and its canonical form for messages.
    value: Option<(String, String)>,
}

/// A row of a run of a frame: (the index of the frame, the run, the row).
pub(crate) type RowRef = (usize, usize, usize);

/// What a field found in an element.
enum Found {
    Value(String, String),
    /// No value: nilled, not valid, or not assessed.
    Absent,
    /// The element has a complex type, and a field needs a simple value.
    NotSimple,
}

/// The primitive type of a value, so that `1` as a string and `1` as a number are not equal.
fn class(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "string",
        Value::AnyUri(_) => "anyURI",
        Value::Boolean(_) => "boolean",
        Value::Decimal(_) => "decimal",
        Value::Float(_) => "float",
        Value::Double(_) => "double",
        Value::Duration(_) => "duration",
        Value::Temporal(_) => "temporal",
        Value::HexBinary(_) => "hexBinary",
        Value::Base64Binary(_) => "base64Binary",
        Value::QName(_) => "QName",
        Value::Notation(_) => "NOTATION",
        Value::List(_) => "list",
    }
}

fn key_of(value: &Value) -> (String, String) {
    (format!("{}|{}", class(value), value.canonical()), value.canonical())
}

impl<'s> XsdValidator<'s> {
    /// The element just opened may declare constraints (it is their context), be reached by the selectors of the constraints of the
    /// elements it is in, and be reached by the fields of the rows they made.
    pub(crate) fn identity_start(&mut self, range: &Range<usize>, attributes: &[XsdAttribute<'_>], ns: &dyn NamespaceContext) {
        let schema = self.schema;
        let Some(top) = self.stack.len().checked_sub(1) else { return };
        if let Some(frame) = self.stack.get_mut(top) {
            for &id in frame.constraints {
                if schema.constraints.get(id.0).is_some_and(|c| c.paths.is_some()) {
                    frame.runs.push(Run { constraint: id, rows: Vec::new() });
                }
            }
        }
        // The selectors of the constraints of this element and of those it is in.
        let mut selected: Vec<(usize, usize)> = Vec::new();
        for index in 0..=top {
            let Some(frame) = self.stack.get(index) else { continue };
            if frame.runs.is_empty() {
                continue;
            }
            let chain = self.chain(index + 1, top);
            for (r, run) in frame.runs.iter().enumerate() {
                let Some(paths) = schema.constraints.get(run.constraint.0).and_then(|c| c.paths.as_ref()) else { continue };
                if paths.selector.alternatives.iter().any(|a| a.matches_elements(&chain)) {
                    selected.push((index, r));
                }
            }
        }
        for (index, r) in selected {
            let Some(run) = self.stack.get_mut(index).and_then(|f| f.runs.get_mut(r)) else { continue };
            let fields = schema.constraints.get(run.constraint.0).and_then(|c| c.paths.as_ref()).map_or(0, |p| p.fields.len());
            run.rows.push(Row { depth: top, range: range.clone(), fields: vec![Field::default(); fields] });
            self.open_rows.push((index, r, run.rows.len() - 1));
        }
        // The fields of the rows that are open: this element may be a value of one, or have an attribute that is.
        let open: Vec<RowRef> = self.open_rows.clone();
        for row_ref in open {
            self.fields_at(row_ref, top, attributes, ns);
        }
    }

    /// The names of the elements from the frame `from` to the frame `to`.
    fn chain(&self, from: usize, to: usize) -> Vec<&QName> {
        (from..=to).filter_map(|i| self.stack.get(i)).map(|f| &f.name).collect()
    }

    fn fields_at(&mut self, row_ref: RowRef, top: usize, attributes: &[XsdAttribute<'_>], ns: &dyn NamespaceContext) {
        let schema = self.schema;
        let (frame_index, run_index, row_index) = row_ref;
        let Some(row_depth) = self.stack.get(frame_index).and_then(|f| f.runs.get(run_index)).and_then(|r| r.rows.get(row_index)).map(|r| r.depth)
        else {
            return;
        };
        let Some(constraint) = self.stack.get(frame_index).and_then(|f| f.runs.get(run_index)).and_then(|r| schema.constraints.get(r.constraint.0))
        else {
            return;
        };
        let Some(paths) = constraint.paths.as_ref() else { return };
        let chain = self.chain(row_depth + 1, top);
        // What this node gives each field: whether it is a value of it, and which attributes are.
        let mut element_fields: Vec<usize> = Vec::new();
        let mut attribute_hits: Vec<(usize, usize)> = Vec::new();
        for (f, field) in paths.fields.iter().enumerate() {
            let here = field.alternatives.iter().filter(|a| a.matches_elements(&chain));
            let mut as_element = false;
            let mut hit_attributes: HashSet<usize> = HashSet::new();
            for alternative in here {
                match &alternative.attribute {
                    None => as_element = true,
                    Some(test) => {
                        for (a, attribute) in attributes.iter().enumerate() {
                            if test.matches(&attribute.name) {
                                hit_attributes.insert(a);
                            }
                        }
                    }
                }
            }
            if as_element {
                element_fields.push(f);
            }
            attribute_hits.extend(hit_attributes.into_iter().map(|a| (f, a)));
        }
        attribute_hits.sort_unstable();
        // The values of the attributes.
        let mut values: Vec<(usize, Option<(String, String)>)> = Vec::new();
        for &(f, a) in &attribute_hits {
            let Some(attribute) = attributes.get(a) else { continue };
            values.push((f, self.attribute_key(top, attribute, ns)));
        }
        let Some(row) = self.stack.get_mut(frame_index).and_then(|f| f.runs.get_mut(run_index)).and_then(|r| r.rows.get_mut(row_index)) else {
            return;
        };
        for (f, value) in values {
            if let Some(field) = row.fields.get_mut(f) {
                field.count += 1;
                if field.count == 1 {
                    field.value = value;
                }
            }
        }
        for &f in &element_fields {
            if let Some(field) = row.fields.get_mut(f) {
                field.count += 1;
            }
        }
        if let Some(frame) = self.stack.get_mut(top) {
            frame.targets.extend(element_fields.into_iter().map(|f| (row_ref, f)));
        }
    }

    /// The value an attribute gives a field, by its declared type; an attribute that is not declared has a string.
    fn attribute_key(&self, top: usize, attribute: &XsdAttribute<'_>, ns: &dyn NamespaceContext) -> Option<(String, String)> {
        let schema = self.schema;
        let frame = self.stack.get(top)?;
        let declared = frame.uses.and_then(|(uses, wildcard)| {
            let used = uses.iter().find_map(|u| schema.attribute(u.attribute).filter(|d| d.name == attribute.name));
            used.or_else(|| {
                let allowed: bool = wildcard.is_some_and(|w| w.constraint.allows(attribute.name.namespace.as_deref()));
                if allowed { schema.attribute_by_name(&attribute.name).and_then(|id| schema.attribute(id)) } else { None }
            })
        });
        match declared {
            Some(decl) => decl.type_.validate(attribute.value, ns).ok().map(|v| key_of(&v)),
            None => Some(key_of(&Value::String(attribute.value.to_owned()))),
        }
    }

    /// What the open element gives a field that selected it.
    fn found(&self, frame: &Frame<'s>, ns: &dyn NamespaceContext) -> Found {
        match &frame.content {
            _ if frame.nil => Found::Absent,
            Content::Simple(ty) => {
                if frame.unknown_text || frame.reported {
                    return Found::Absent;
                }
                if frame.text.is_empty()
                    && let Some(c) = frame.constraint
                {
                    let (key, shown) = key_of(&c.value);
                    return Found::Value(key, shown);
                }
                match ty.validate(&frame.text, ns) {
                    Ok(value) => {
                        let (key, shown) = key_of(&value);
                        Found::Value(key, shown)
                    }
                    Err(_) => Found::Absent,
                }
            }
            Content::Empty | Content::Elements { .. } => Found::NotSimple,
            Content::Skip | Content::Lax => Found::Absent,
        }
    }

    /// The open element ends: it gives the fields that selected it their value, and the rows that are about it are complete.
    pub(crate) fn identity_node_end(&mut self, ns: &dyn NamespaceContext) {
        let Some(top) = self.stack.len().checked_sub(1) else { return };
        let Some(frame) = self.stack.get(top) else { return };
        if !frame.targets.is_empty() {
            let found = self.found(frame, ns);
            let targets = frame.targets.clone();
            let shown = frame.shown.clone();
            let range = frame.range.clone();
            for ((frame_index, run_index, row_index), f) in targets {
                let Some(run) = self.stack.get_mut(frame_index).and_then(|f| f.runs.get_mut(run_index)) else { continue };
                let constraint = run.constraint;
                let Some(field) = run.rows.get_mut(row_index).and_then(|r| r.fields.get_mut(f)) else { continue };
                match &found {
                    Found::Value(key, display) => {
                        if field.count == 1 {
                            field.value = Some((key.clone(), display.clone()));
                        }
                    }
                    Found::Absent => {}
                    Found::NotSimple => {
                        let name = self.schema.constraints.get(constraint.0).map(|c| c.name.to_string()).unwrap_or_default();
                        self.report(Code::CvcFieldNotSimple, range.clone(), &[&name, &shown]);
                    }
                }
            }
        }
        // The rows about this node are complete.
        while let Some(&(frame_index, run_index, row_index)) = self.open_rows.last() {
            let depth = self.stack.get(frame_index).and_then(|f| f.runs.get(run_index)).and_then(|r| r.rows.get(row_index)).map(|r| r.depth);
            if depth == Some(top) {
                self.open_rows.pop();
            } else {
                break;
            }
        }
    }

    /// An element that declared constraints ends: its rows are checked, and the keys it made are known to the elements above.
    pub(crate) fn identity_context_end(&mut self, mut frame: Frame<'s>) {
        let schema = self.schema;
        let mut problems: Vec<(Code, Range<usize>, Vec<String>)> = Vec::new();
        for run in &frame.runs {
            let Some(constraint) = schema.constraints.get(run.constraint.0) else { continue };
            if constraint.kind == IdentityKind::Keyref {
                continue;
            }
            let mut seen: HashSet<String> = HashSet::new();
            for row in &run.rows {
                check_row(constraint, row, &mut problems);
                match row_tuple(row) {
                    Some((key, shown)) => {
                        if !seen.insert(key) {
                            problems.push((Code::CvcKeyDuplicate, row.range.clone(), vec![constraint.name.to_string(), shown]));
                        }
                    }
                    None if constraint.kind == IdentityKind::Key && row.fields.iter().all(|f| f.count <= 1) => {
                        let missing = row.fields.iter().position(|f| f.value.is_none()).map_or(1, |i| i + 1);
                        problems.push((
                            Code::CvcKeyMissing,
                            row.range.clone(),
                            vec![constraint.name.to_string(), format!("a value for its field {missing}")],
                        ));
                    }
                    None => {}
                }
            }
            frame.tables.entry(run.constraint).or_default().extend(seen);
        }
        for run in &frame.runs {
            let Some(constraint) = schema.constraints.get(run.constraint.0) else { continue };
            if constraint.kind != IdentityKind::Keyref {
                continue;
            }
            let table = constraint.refer.and_then(|refer| frame.tables.get(&refer));
            for row in &run.rows {
                check_row(constraint, row, &mut problems);
                if let Some((key, shown)) = row_tuple(row)
                    && !table.is_some_and(|t| t.contains(&key))
                {
                    problems.push((Code::CvcKeyrefNoKey, row.range.clone(), vec![constraint.name.to_string(), shown]));
                }
            }
        }
        for (code, range, args) in problems {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            self.report(code, range, &args);
        }
        // What the element knows of keys is known to the element it is in.
        let tables: HashMap<ConstraintId, HashSet<String>> = std::mem::take(&mut frame.tables);
        if let Some(parent) = self.stack.last_mut() {
            for (id, keys) in tables {
                parent.tables.entry(id).or_default().extend(keys);
            }
        }
    }
}

/// A field that selected more than one node is a mistake of the row.
fn check_row(constraint: &IdentityConstraint, row: &Row, problems: &mut Vec<(Code, Range<usize>, Vec<String>)>) {
    if row.fields.iter().any(|f| f.count > 1) {
        problems.push((Code::CvcFieldMultiple, row.range.clone(), vec![constraint.name.to_string(), "more than one node".to_owned()]));
    }
}

/// The values of the fields of a row as one key and one text, when every field found exactly one value.
fn row_tuple(row: &Row) -> Option<(String, String)> {
    let mut keys = Vec::with_capacity(row.fields.len());
    let mut shown = Vec::with_capacity(row.fields.len());
    for field in &row.fields {
        if field.count != 1 {
            return None;
        }
        let (key, display) = field.value.as_ref()?;
        keys.push(key.as_str());
        shown.push(display.as_str());
    }
    if keys.is_empty() {
        return None;
    }
    Some((keys.join("\u{1}"), shown.join(", ")))
}
