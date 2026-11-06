//! Checking an instance against a schema.
//!
//! The schema is read as it is (no compiled form): each keyword is looked at when the instance reaches its schema. What a schema
//! says about the places it has covered (`properties`, `items`, `contains` …) is carried up as annotations for `unevaluatedProperties`
//! and `unevaluatedItems`.

use std::collections::{BTreeSet, HashMap, HashSet};

use regex::{Regex, RegexBuilder};
use xmlcraft_json_syntax::{Data, Member, Value};

use crate::draft::Draft;
use crate::number::Num;
use crate::schema::{Place, SchemaSet};
use crate::uri;

/// Validation stops after this many violations.
pub const MAX_VIOLATIONS: usize = 1_000;
/// Schemas nested (or referred to) deeper than this are not followed.
const MAX_DEPTH: usize = 200;
/// Validation stops after this many schema evaluations (a schema can multiply work through `$ref`s).
const MAX_STEPS: u64 = 5_000_000;
/// A pattern that compiles to more than this is not used.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// Something in the instance that the schema does not allow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// Where in the instance, as a JSON Pointer (`/items/0/name`).
    pub instance: String,
    /// The keyword that was not met, as a JSON Pointer into the schema document that holds it (`/properties/name/type`).
    pub schema: String,
    pub keyword: &'static str,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub violations: Vec<Violation>,
    /// More than [`MAX_VIOLATIONS`] were found, or the work was cut off: the list is not everything.
    pub truncated: bool,
}

impl Outcome {
    pub fn is_valid(&self) -> bool {
        self.violations.is_empty() && !self.truncated
    }
}

impl SchemaSet {
    /// The place a full reference (`uri#fragment`) names, and the URI of the resource that holds it.
    pub(crate) fn resolve(&self, target: &str) -> Option<(Place, String)> {
        let resource = uri::without_fragment(target);
        let fragment = uri::fragment(target).unwrap_or_default();
        let decoded = uri::percent_decode(fragment)?;
        let found = self.resources.get(resource);
        if decoded.is_empty() {
            return found.map(|p| (p.clone(), resource.to_owned()));
        }
        if decoded.starts_with('/') {
            let at = found?;
            let place = Place { doc: at.doc, pointer: format!("{}{decoded}", at.pointer) };
            let base = self.base_inside(at, &decoded, resource);
            return Some((place, base));
        }
        let anchor = self.anchors.get(&format!("{resource}#{decoded}"))?;
        Some((anchor.clone(), resource.to_owned()))
    }

    /// The base URI in force at `place`: that of the closest resource around it, changed by each `$id` between.
    pub(crate) fn base_of(&self, place: &Place) -> String {
        let around = self
            .resources
            .iter()
            .filter(|(_, at)| {
                at.doc == place.doc
                    && (at.pointer.is_empty() || place.pointer == at.pointer || place.pointer.starts_with(&format!("{}/", at.pointer)))
            })
            .max_by_key(|(_, at)| at.pointer.len());
        match around {
            Some((resource, at)) => {
                let rest = place.pointer.get(at.pointer.len()..).unwrap_or_default();
                self.base_inside(at, rest, resource)
            }
            None => String::new(),
        }
    }

    /// The base URI in force at `rest` (a pointer) below the resource `at`, which is known as `resource`: each schema on the way down
    /// that has an `$id` changes it. The schema at the end of the pointer is not counted; evaluating it does that.
    pub(crate) fn base_inside(&self, at: &Place, rest: &str, resource: &str) -> String {
        let draft = self.docs.get(at.doc).map_or(Draft::D2020_12, |d| d.draft);
        let mut base = resource.to_owned();
        let Some(mut value) = self.docs.get(at.doc).and_then(|d| d.value.pointer(&at.pointer)) else { return base };
        let tokens: Vec<&str> = rest.split('/').skip(1).collect();
        for (i, token) in tokens.iter().enumerate() {
            let Some(raw) = uri::unescape_token(token) else { break };
            let next = match &value.data {
                Data::Object(members) => find(members, &raw),
                Data::Array(items) => raw.parse::<usize>().ok().and_then(|n| items.get(n)),
                _ => None,
            };
            let Some(next) = next else { break };
            value = next;
            if i + 1 < tokens.len()
                && let Data::Object(members) = &value.data
                && !(draft.ref_hides_siblings() && find(members, "$ref").is_some())
                && let Some(id) = find(members, draft.id_keyword()).and_then(text_of)
                && !id.starts_with('#')
            {
                base = uri::without_fragment(&uri::join(&base, id)).to_owned();
            }
        }
        base
    }

    /// Check `instance` against the first document of the set.
    pub fn validate(&self, instance: &Value) -> Outcome {
        let Some(doc) = self.docs.first() else { return Outcome::default() };
        let mut run = Run { set: self, violations: Vec::new(), regexes: HashMap::new(), steps: 0, truncated: false, scope: vec![doc.base.clone()] };
        let base = doc.base.clone();
        run.eval(0, &doc.value, "", instance, "", &base, 0);
        Outcome { violations: run.violations, truncated: run.truncated }
    }
}

/// What a schema established about the instance it was applied to.
#[derive(Default)]
struct Ann {
    valid: bool,
    /// Names of properties that a keyword evaluated.
    props: BTreeSet<String>,
    /// Every item was evaluated (`items` with a schema, `additionalItems`, `unevaluatedItems`).
    items_all: bool,
    /// Items before this index were evaluated.
    items_to: usize,
    /// Indexes `contains` matched (counted since 2020-12).
    contains: BTreeSet<usize>,
}

impl Ann {
    fn valid() -> Ann {
        Ann { valid: true, ..Ann::default() }
    }

    fn merge(&mut self, other: Ann) {
        self.props.extend(other.props);
        self.items_all |= other.items_all;
        self.items_to = self.items_to.max(other.items_to);
        self.contains.extend(other.contains);
    }

    fn covers_item(&self, index: usize) -> bool {
        self.items_all || index < self.items_to || self.contains.contains(&index)
    }
}

struct Run<'a> {
    set: &'a SchemaSet,
    violations: Vec<Violation>,
    regexes: HashMap<String, Option<Regex>>,
    steps: u64,
    truncated: bool,
    /// The resources entered, outermost first (what `$dynamicRef` looks through).
    scope: Vec<String>,
}

fn text_of(value: &Value) -> Option<&str> {
    if let Data::String(s) = &value.data { Some(s) } else { None }
}

fn number_of(value: &Value) -> Option<Num> {
    if let Data::Number(n) = &value.data { Num::parse(n) } else { None }
}

fn find<'v>(members: &'v [Member], key: &str) -> Option<&'v Value> {
    members.iter().find(|m| m.key == key).map(|m| &m.value)
}

/// The JSON type name of `value` for messages.
fn type_name(value: &Value) -> &'static str {
    match &value.data {
        Data::Null => "null",
        Data::Bool(_) => "boolean",
        Data::String(_) => "string",
        Data::Number(n) if Num::parse(n).is_some_and(Num::is_integer) => "integer",
        Data::Number(_) => "number",
        Data::Array(_) => "array",
        Data::Object(_) => "object",
    }
}

fn is_type(value: &Value, wanted: &str, draft: Draft) -> bool {
    match (&value.data, wanted) {
        (Data::Null, "null") | (Data::Bool(_), "boolean") | (Data::String(_), "string") | (Data::Array(_), "array") | (Data::Object(_), "object") => {
            true
        }
        (Data::Number(_), "number") => true,
        (Data::Number(n), "integer") => {
            if draft.float_integers() {
                Num::parse(n).is_some_and(Num::is_integer)
            } else {
                // Draft 4: a number with no fraction or exponent in the way it is written.
                !n.contains(['.', 'e', 'E'])
            }
        }
        _ => false,
    }
}

/// The members of an object once each (the first of a repeated name), by name.
fn entries(members: &[Member]) -> Vec<(&str, &Value)> {
    let mut seen = HashSet::new();
    let mut list: Vec<(&str, &Value)> = members.iter().filter(|m| seen.insert(m.key.as_str())).map(|m| (m.key.as_str(), &m.value)).collect();
    list.sort_by_key(|(k, _)| *k);
    list
}

/// Whether two values are equal in JSON Schema: numbers by value (`1.0` is `1`), objects by their members whatever the order.
fn equal(a: &Value, b: &Value) -> bool {
    match (&a.data, &b.data) {
        (Data::Null, Data::Null) => true,
        (Data::Bool(x), Data::Bool(y)) => x == y,
        (Data::String(x), Data::String(y)) => x == y,
        (Data::Number(x), Data::Number(y)) => match (Num::parse(x), Num::parse(y)) {
            (Some(x), Some(y)) => x.compare(y) == Some(std::cmp::Ordering::Equal),
            _ => x == y,
        },
        (Data::Array(x), Data::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| equal(p, q)),
        (Data::Object(x), Data::Object(y)) => {
            let (x, y) = (entries(x), entries(y));
            x.len() == y.len() && x.iter().zip(&y).all(|((kx, vx), (ky, vy))| kx == ky && equal(vx, vy))
        }
        _ => false,
    }
}

/// A text that is the same for values that are equal, or nearly: numbers by the `f64` nearest to them, which two equal numbers share
/// (two values with the same text may still differ: [`equal`] decides). Written straight into `out`; `false` when the value is larger
/// than the budget (bytes) allows, and then the text is not to be used.
fn bucket(value: &Value, out: &mut String, budget: &mut usize) -> bool {
    let cost = 8 + match &value.data {
        Data::String(s) => s.len(),
        Data::Number(n) => n.len(),
        _ => 0,
    };
    if *budget < cost {
        return false;
    }
    *budget -= cost;
    match &value.data {
        Data::Null => out.push('n'),
        Data::Bool(b) => out.push(if *b { 't' } else { 'f' }),
        Data::String(s) => {
            out.push('"');
            for c in s.chars() {
                if matches!(c, '"' | '\\') {
                    out.push('\\');
                }
                out.push(c);
            }
            out.push('"');
        }
        Data::Number(n) => {
            let f = Num::parse(n).map_or(f64::NAN, Num::to_f64);
            // (Zero and minus zero are one number.)
            let bits = if f == 0.0 { 0 } else { f.to_bits() };
            out.push_str(&format!("#{bits:x}"));
        }
        Data::Array(items) => {
            out.push('[');
            for item in items {
                if !bucket(item, out, budget) {
                    return false;
                }
                out.push(',');
            }
            out.push(']');
        }
        Data::Object(members) => {
            let mut seen = HashSet::new();
            let mut entries: Vec<(&str, &Value)> =
                members.iter().filter(|m| seen.insert(m.key.as_str())).map(|m| (m.key.as_str(), &m.value)).collect();
            entries.sort_by_key(|(k, _)| *k);
            out.push('{');
            for (key, member) in entries {
                out.push_str(&format!("{key:?}:"));
                if !bucket(member, out, budget) {
                    return false;
                }
                out.push(',');
            }
            out.push('}');
        }
    }
    true
}

impl Run<'_> {
    fn fail(&mut self, ip: &str, sp: &str, keyword: &'static str, message: String) {
        if self.violations.len() >= MAX_VIOLATIONS {
            self.truncated = true;
            return;
        }
        self.violations.push(Violation { instance: ip.to_owned(), schema: format!("{sp}/{keyword}"), keyword, message });
    }

    fn regex(&mut self, pattern: &str) -> Option<&Regex> {
        self.regexes.entry(pattern.to_owned()).or_insert_with(|| RegexBuilder::new(pattern).size_limit(REGEX_SIZE_LIMIT).build().ok()).as_ref()
    }

    /// Apply the schema `s` (at pointer `sp` of document `doc`, in a resource known as `base`) to `inst` (at `ip`).
    #[allow(clippy::too_many_arguments)]
    fn eval(&mut self, doc: usize, s: &Value, sp: &str, inst: &Value, ip: &str, base: &str, depth: usize) -> Ann {
        self.steps += 1;
        if depth > MAX_DEPTH || self.steps > MAX_STEPS {
            self.truncated = true;
            return Ann::default();
        }
        let draft = self.set.docs.get(doc).map_or(Draft::D2020_12, |d| d.draft);
        let members = match &s.data {
            // (Before draft 6 a boolean is only met as the value of `additionalItems` or `additionalProperties`.)
            Data::Bool(b) => {
                if *b {
                    return Ann::valid();
                }
                let before = self.violations.len();
                self.fail(ip, sp.trim_end_matches('/'), "false", "No value is allowed here.".to_owned());
                // (A false schema has no keyword: the pointer is the schema itself.)
                if self.violations.len() > before
                    && let Some(last) = self.violations.last_mut()
                {
                    last.schema = sp.to_owned();
                }
                return Ann::default();
            }
            Data::Object(members) => members.as_slice(),
            _ => return Ann::valid(),
        };
        // A schema that says who it is starts a new resource.
        let mut base = base.to_owned();
        let mut entered = false;
        // (Up to draft 7 a `$ref` hides the `$id` beside it too; and a schema that is the resource we have just been sent to has entered it.)
        let hidden = draft.ref_hides_siblings() && find(members, "$ref").is_some();
        let arrived = self.set.resources.get(base.as_str()).is_some_and(|p| p.doc == doc && p.pointer == sp);
        if !hidden
            && !arrived
            && let Some(id) = find(members, draft.id_keyword()).and_then(text_of)
            && !id.starts_with('#')
        {
            base = uri::without_fragment(&uri::join(&base, id)).to_owned();
            self.scope.push(base.clone());
            entered = true;
        }
        let mut ann = Ann::valid();
        if draft.ref_hides_siblings() && find(members, "$ref").is_some() {
            if let Some(reference) = find(members, "$ref").and_then(text_of) {
                self.reference(reference, "$ref", sp, inst, ip, &base, depth, &mut ann);
            }
        } else {
            self.keywords(doc, members, draft, sp, inst, ip, &base, depth, &mut ann);
        }
        if entered {
            self.scope.pop();
        }
        ann
    }

    /// Follow a reference and merge what the target says into `ann`.
    #[allow(clippy::too_many_arguments)]
    fn reference(&mut self, reference: &str, keyword: &'static str, sp: &str, inst: &Value, ip: &str, base: &str, depth: usize, ann: &mut Ann) {
        let target = uri::join(base, reference);
        let Some((place, target_base)) = self.set.resolve(&target) else {
            self.fail(ip, sp, keyword, format!("The reference '{reference}' cannot be resolved."));
            ann.valid = false;
            return;
        };
        let redirected = self.dynamic_target(keyword, &target, place.clone());
        // A dynamic reference may land in another resource: the base is that resource's.
        let (place, target_base) = if redirected.doc == place.doc && redirected.pointer == place.pointer {
            (place, target_base)
        } else {
            let base = self.set.base_of(&redirected);
            (redirected, base)
        };
        let Some(value) = self.set.docs.get(place.doc).and_then(|d| d.value.pointer(&place.pointer)) else {
            self.fail(ip, sp, keyword, format!("The reference '{reference}' points at nothing."));
            ann.valid = false;
            return;
        };
        let pushed = !self.scope.contains(&target_base);
        if pushed {
            self.scope.push(target_base.clone());
        }
        let result = self.eval(place.doc, value, &place.pointer, inst, ip, &target_base, depth + 1);
        if pushed {
            self.scope.pop();
        }
        if result.valid {
            ann.merge(result);
        } else {
            ann.valid = false;
        }
    }

    /// `$dynamicRef` and `$recursiveRef` look for the outermost resource that has the same dynamic anchor.
    fn dynamic_target(&self, keyword: &str, target: &str, place: Place) -> Place {
        let name = uri::fragment(target).unwrap_or_default();
        match keyword {
            "$dynamicRef" if self.set.dynamic_anchors.contains(&format!("{}#{name}", uri::without_fragment(target))) => {
                for resource in &self.scope {
                    if self.set.dynamic_anchors.contains(&format!("{resource}#{name}"))
                        && let Some(found) = self.set.anchors.get(&format!("{resource}#{name}"))
                    {
                        return found.clone();
                    }
                }
                place
            }
            "$recursiveRef" => {
                // Only when the schema it first points at is itself a `$recursiveAnchor`.
                let anchored = |value: &Value| matches!(value.get("$recursiveAnchor").map(|v| &v.data), Some(Data::Bool(true)));
                if !self.set.docs.get(place.doc).and_then(|d| d.value.pointer(&place.pointer)).is_some_and(anchored) {
                    return place;
                }
                // The outermost resource whose root says `$recursiveAnchor: true`.
                for resource in &self.scope {
                    if let Some(root) = self.set.resources.get(resource)
                        && let Some(value) = self.set.docs.get(root.doc).and_then(|d| d.value.pointer(&root.pointer))
                        && matches!(value.get("$recursiveAnchor").map(|v| &v.data), Some(Data::Bool(true)))
                    {
                        return root.clone();
                    }
                }
                place
            }
            _ => place,
        }
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn keywords(&mut self, doc: usize, members: &[Member], draft: Draft, sp: &str, inst: &Value, ip: &str, base: &str, depth: usize, ann: &mut Ann) {
        // --- anything ---
        if let Some(t) = find(members, "type") {
            let wanted: Vec<&str> = match &t.data {
                Data::String(s) => vec![s.as_str()],
                Data::Array(list) => list.iter().filter_map(text_of).collect(),
                _ => Vec::new(),
            };
            if !wanted.is_empty() && !wanted.iter().any(|w| is_type(inst, w, draft)) {
                self.fail(ip, sp, "type", format!("Expected {}, found {}.", wanted.join(" or "), type_name(inst)));
                ann.valid = false;
            }
        }
        if let Some(Data::Array(options)) = find(members, "enum").map(|v| &v.data)
            && !options.iter().any(|o| equal(o, inst))
        {
            self.fail(ip, sp, "enum", "The value is not one of the allowed values.".to_owned());
            ann.valid = false;
        }
        if draft >= Draft::D06
            && let Some(expected) = find(members, "const")
            && !equal(expected, inst)
        {
            self.fail(ip, sp, "const", "The value is not the required constant.".to_owned());
            ann.valid = false;
        }
        // --- by type of the instance ---
        match &inst.data {
            Data::Number(text) => {
                if let Some(x) = Num::parse(text) {
                    self.numbers(members, draft, sp, ip, x, ann);
                }
            }
            Data::String(s) => self.strings(members, sp, ip, s, ann),
            Data::Array(items) => self.arrays(doc, members, draft, sp, items, ip, base, depth, ann),
            Data::Object(object) => self.objects(doc, members, draft, sp, object, ip, base, depth, ann),
            _ => {}
        }
        // --- applicators and references ---
        if draft >= Draft::D2019_09 {
            for keyword in ["$ref", "$dynamicRef", "$recursiveRef"] {
                if let Some(reference) = find(members, keyword).and_then(text_of)
                    && (keyword == "$ref" || (keyword == "$dynamicRef") == (draft >= Draft::D2020_12))
                {
                    let keyword: &'static str = match keyword {
                        "$ref" => "$ref",
                        "$dynamicRef" => "$dynamicRef",
                        _ => "$recursiveRef",
                    };
                    self.reference(reference, keyword, sp, inst, ip, base, depth, ann);
                }
            }
        }
        if let Some(Data::Array(all)) = find(members, "allOf").map(|v| &v.data) {
            for (i, sub) in all.iter().enumerate() {
                let r = self.eval(doc, sub, &format!("{sp}/allOf/{i}"), inst, ip, base, depth + 1);
                if r.valid {
                    ann.merge(r);
                } else {
                    ann.valid = false;
                }
            }
        }
        if let Some(Data::Array(any)) = find(members, "anyOf").map(|v| &v.data) {
            let mark = self.violations.len();
            let mut passed = false;
            for (i, sub) in any.iter().enumerate() {
                let r = self.eval(doc, sub, &format!("{sp}/anyOf/{i}"), inst, ip, base, depth + 1);
                if r.valid {
                    passed = true;
                    ann.merge(r);
                }
            }
            self.violations.truncate(mark);
            if !passed && !any.is_empty() {
                self.fail(ip, sp, "anyOf", format!("The value matches none of the {} alternatives.", any.len()));
                ann.valid = false;
            }
        }
        if let Some(Data::Array(one)) = find(members, "oneOf").map(|v| &v.data) {
            let mark = self.violations.len();
            let mut matched = 0usize;
            let mut merged = Ann::valid();
            for (i, sub) in one.iter().enumerate() {
                let r = self.eval(doc, sub, &format!("{sp}/oneOf/{i}"), inst, ip, base, depth + 1);
                if r.valid {
                    matched += 1;
                    merged.merge(r);
                }
            }
            self.violations.truncate(mark);
            match matched {
                1 => ann.merge(merged),
                0 if !one.is_empty() => {
                    self.fail(ip, sp, "oneOf", format!("The value matches none of the {} alternatives.", one.len()));
                    ann.valid = false;
                }
                0 => {}
                n => {
                    self.fail(ip, sp, "oneOf", format!("The value matches {n} of the alternatives; exactly one is required."));
                    ann.valid = false;
                }
            }
        }
        if let Some(sub) = find(members, "not")
            && (draft.boolean_schemas() || matches!(sub.data, Data::Object(_)))
        {
            let mark = self.violations.len();
            let r = self.eval(doc, sub, &format!("{sp}/not"), inst, ip, base, depth + 1);
            self.violations.truncate(mark);
            if r.valid {
                self.fail(ip, sp, "not", "The value matches a schema that it must not match.".to_owned());
                ann.valid = false;
            }
        }
        if draft >= Draft::D07
            && let Some(condition) = find(members, "if")
        {
            let mark = self.violations.len();
            let r = self.eval(doc, condition, &format!("{sp}/if"), inst, ip, base, depth + 1);
            self.violations.truncate(mark);
            let branch = if r.valid { "then" } else { "else" };
            if r.valid {
                ann.merge(r);
            }
            if let Some(sub) = find(members, branch) {
                let r = self.eval(doc, sub, &format!("{sp}/{branch}"), inst, ip, base, depth + 1);
                if r.valid {
                    ann.merge(r);
                } else {
                    ann.valid = false;
                }
            }
        }
        // --- what nothing else covered ---
        if draft >= Draft::D2019_09 {
            self.unevaluated(doc, members, sp, inst, ip, base, depth, ann);
        }
    }

    fn numbers(&mut self, members: &[Member], draft: Draft, sp: &str, ip: &str, x: Num, ann: &mut Ann) {
        use std::cmp::Ordering::{Greater, Less};
        let limit = |key: &str| find(members, key).and_then(number_of);
        let exclusive_flag = |key: &str| matches!(find(members, key).map(|v| &v.data), Some(Data::Bool(true)));
        if let Some(min) = limit("minimum") {
            let exclusive = draft == Draft::D04 && exclusive_flag("exclusiveMinimum");
            let bad = match x.compare(min) {
                Some(Less) => true,
                Some(std::cmp::Ordering::Equal) => exclusive,
                _ => false,
            };
            if bad {
                let word = if exclusive { "or equal to " } else { "" };
                self.fail(ip, sp, "minimum", format!("The value is less than {word}the minimum of {}.", show(members, "minimum")));
                ann.valid = false;
            }
        }
        if let Some(max) = limit("maximum") {
            let exclusive = draft == Draft::D04 && exclusive_flag("exclusiveMaximum");
            let bad = match x.compare(max) {
                Some(Greater) => true,
                Some(std::cmp::Ordering::Equal) => exclusive,
                _ => false,
            };
            if bad {
                let word = if exclusive { "or equal to " } else { "" };
                self.fail(ip, sp, "maximum", format!("The value is greater than {word}the maximum of {}.", show(members, "maximum")));
                ann.valid = false;
            }
        }
        if draft >= Draft::D06 {
            if let Some(min) = limit("exclusiveMinimum")
                && x.compare(min).is_none_or(|o| o != Greater)
            {
                self.fail(ip, sp, "exclusiveMinimum", format!("The value is not greater than {}.", show(members, "exclusiveMinimum")));
                ann.valid = false;
            }
            if let Some(max) = limit("exclusiveMaximum")
                && x.compare(max).is_none_or(|o| o != Less)
            {
                self.fail(ip, sp, "exclusiveMaximum", format!("The value is not less than {}.", show(members, "exclusiveMaximum")));
                ann.valid = false;
            }
        }
        if let Some(d) = limit("multipleOf")
            && d.sign() == Greater
            && !x.is_multiple_of(d)
        {
            self.fail(ip, sp, "multipleOf", format!("The value is not a multiple of {}.", show(members, "multipleOf")));
            ann.valid = false;
        }
    }

    fn strings(&mut self, members: &[Member], sp: &str, ip: &str, s: &str, ann: &mut Ann) {
        let count = |key: &str| find(members, key).and_then(number_of).filter(|n| n.is_integer());
        let length = Num::parse(&s.chars().count().to_string());
        if let (Some(min), Some(len)) = (count("minLength"), length)
            && len.compare(min).is_some_and(|o| o.is_lt())
        {
            self.fail(ip, sp, "minLength", format!("The text is shorter than {} characters.", show(members, "minLength")));
            ann.valid = false;
        }
        if let (Some(max), Some(len)) = (count("maxLength"), length)
            && len.compare(max).is_some_and(|o| o.is_gt())
        {
            self.fail(ip, sp, "maxLength", format!("The text is longer than {} characters.", show(members, "maxLength")));
            ann.valid = false;
        }
        if let Some(pattern) = find(members, "pattern").and_then(text_of)
            && let Some(regex) = self.regex(pattern)
            && !regex.is_match(s)
        {
            self.fail(ip, sp, "pattern", format!("The text does not match the pattern '{pattern}'."));
            ann.valid = false;
        }
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn arrays(&mut self, doc: usize, members: &[Member], draft: Draft, sp: &str, items: &[Value], ip: &str, base: &str, depth: usize, ann: &mut Ann) {
        let len = items.len();
        let count = |key: &str| find(members, key).and_then(number_of).and_then(|n| n.is_integer().then_some(n.to_f64()));
        if let Some(min) = count("minItems")
            && (len as f64) < min
        {
            self.fail(ip, sp, "minItems", format!("The list has fewer than {min} items."));
            ann.valid = false;
        }
        if let Some(max) = count("maxItems")
            && (len as f64) > max
        {
            self.fail(ip, sp, "maxItems", format!("The list has more than {max} items."));
            ann.valid = false;
        }
        if matches!(find(members, "uniqueItems").map(|v| &v.data), Some(Data::Bool(true))) {
            // Items are grouped by a text that equal items share; within a group they are compared one by one. (An item too large for
            // a text goes into the group of the empty text: compared with its peers.)
            let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
            'items: for (i, item) in items.iter().enumerate() {
                let mut key = String::new();
                let mut budget = 4 * 1024 * 1024;
                if !bucket(item, &mut key, &mut budget) {
                    key.clear();
                }
                let group = groups.entry(key).or_default();
                for &earlier in group.iter() {
                    if items.get(earlier).is_some_and(|e| equal(e, item)) {
                        self.fail(ip, sp, "uniqueItems", format!("Items {earlier} and {i} are equal; the items must be unique."));
                        ann.valid = false;
                        break 'items;
                    }
                }
                group.push(i);
            }
        }
        // Positional and general item schemas.
        let prefix: Option<(&[Value], &'static str)> = match (find(members, "prefixItems").map(|v| &v.data), find(members, "items").map(|v| &v.data))
        {
            (Some(Data::Array(list)), _) if draft >= Draft::D2020_12 => Some((list.as_slice(), "prefixItems")),
            (_, Some(Data::Array(list))) if draft < Draft::D2020_12 => Some((list.as_slice(), "items")),
            _ => None,
        };
        let mut positional = 0;
        if let Some((list, keyword)) = prefix {
            for (i, sub) in list.iter().enumerate() {
                let Some(item) = items.get(i) else { break };
                let r = self.eval(doc, sub, &format!("{sp}/{keyword}/{i}"), item, &format!("{ip}/{i}"), base, depth + 1);
                if !r.valid {
                    ann.valid = false;
                }
            }
            positional = list.len().min(len);
            ann.items_to = ann.items_to.max(positional);
        }
        let rest_schema: Option<(&Value, String)> = if draft >= Draft::D2020_12 {
            find(members, "items").filter(|v| !matches!(v.data, Data::Array(_))).map(|v| (v, format!("{sp}/items")))
        } else if prefix.is_some() {
            find(members, "additionalItems").map(|v| (v, format!("{sp}/additionalItems")))
        } else {
            find(members, "items").filter(|v| draft.boolean_schemas() || matches!(v.data, Data::Object(_))).map(|v| (v, format!("{sp}/items")))
        };
        if let Some((sub, at)) = rest_schema
            && (draft.boolean_schemas() || matches!(sub.data, Data::Object(_)) || (prefix.is_some() && matches!(sub.data, Data::Bool(_))))
        {
            for (i, item) in items.iter().enumerate().skip(positional) {
                let r = self.eval(doc, sub, &at, item, &format!("{ip}/{i}"), base, depth + 1);
                if !r.valid {
                    ann.valid = false;
                }
            }
            ann.items_all = true;
        }
        if draft >= Draft::D06
            && let Some(sub) = find(members, "contains")
        {
            let mut matches = BTreeSet::new();
            let mark = self.violations.len();
            for (i, item) in items.iter().enumerate() {
                if self.eval(doc, sub, &format!("{sp}/contains"), item, &format!("{ip}/{i}"), base, depth + 1).valid {
                    matches.insert(i);
                }
            }
            self.violations.truncate(mark);
            let min = if draft >= Draft::D2019_09 { count("minContains").unwrap_or(1.0) } else { 1.0 };
            let max = if draft >= Draft::D2019_09 { count("maxContains") } else { None };
            let found = matches.len() as f64;
            if found < min || (found == 0.0 && min > 0.0) {
                self.fail(ip, sp, "contains", format!("Fewer than {min} items match the schema (found {found})."));
                ann.valid = false;
            } else if let Some(max) = max
                && found > max
            {
                self.fail(ip, sp, "maxContains", format!("More than {max} items match the schema (found {found})."));
                ann.valid = false;
            }
            if draft >= Draft::D2020_12 {
                ann.contains.extend(matches);
            }
        }
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn objects(
        &mut self,
        doc: usize,
        members: &[Member],
        draft: Draft,
        sp: &str,
        object: &[Member],
        ip: &str,
        base: &str,
        depth: usize,
        ann: &mut Ann,
    ) {
        let mut names: Vec<&str> = Vec::new();
        {
            let mut seen = HashSet::new();
            for m in object {
                if seen.insert(m.key.as_str()) {
                    names.push(m.key.as_str());
                }
            }
        }
        let value_of = |name: &str| object.iter().find(|m| m.key == name).map(|m| &m.value);
        let count = |key: &str| find(members, key).and_then(number_of).and_then(|n| n.is_integer().then_some(n.to_f64()));
        if let Some(min) = count("minProperties")
            && (names.len() as f64) < min
        {
            self.fail(ip, sp, "minProperties", format!("The object has fewer than {min} properties."));
            ann.valid = false;
        }
        if let Some(max) = count("maxProperties")
            && (names.len() as f64) > max
        {
            self.fail(ip, sp, "maxProperties", format!("The object has more than {max} properties."));
            ann.valid = false;
        }
        if let Some(Data::Array(required)) = find(members, "required").map(|v| &v.data) {
            for name in required.iter().filter_map(text_of) {
                if !names.contains(&name) {
                    self.fail(ip, sp, "required", format!("The property '{name}' is required."));
                    ann.valid = false;
                }
            }
        }
        // properties / patternProperties / additionalProperties
        let mut covered: HashSet<&str> = HashSet::new();
        if let Some(Data::Object(properties)) = find(members, "properties").map(|v| &v.data) {
            let mut seen = HashSet::new();
            for p in properties {
                if !seen.insert(p.key.as_str()) {
                    continue;
                }
                if let Some(v) = value_of(&p.key) {
                    covered.insert(p.key.as_str());
                    ann.props.insert(p.key.clone());
                    let r = self.eval(
                        doc,
                        &p.value,
                        &format!("{sp}/properties/{}", uri::escape_token(&p.key)),
                        v,
                        &format!("{ip}/{}", uri::escape_token(&p.key)),
                        base,
                        depth + 1,
                    );
                    if !r.valid {
                        ann.valid = false;
                    }
                }
            }
        }
        if let Some(Data::Object(patterns)) = find(members, "patternProperties").map(|v| &v.data) {
            let mut seen = HashSet::new();
            for p in patterns {
                if !seen.insert(p.key.as_str()) {
                    continue;
                }
                let Some(regex) = self.regex(&p.key).cloned() else { continue };
                for name in &names {
                    if regex.is_match(name)
                        && let Some(v) = value_of(name)
                    {
                        covered.insert(name);
                        ann.props.insert((*name).to_owned());
                        let r = self.eval(
                            doc,
                            &p.value,
                            &format!("{sp}/patternProperties/{}", uri::escape_token(&p.key)),
                            v,
                            &format!("{ip}/{}", uri::escape_token(name)),
                            base,
                            depth + 1,
                        );
                        if !r.valid {
                            ann.valid = false;
                        }
                    }
                }
            }
        }
        if let Some(extra) = find(members, "additionalProperties")
            && (draft.boolean_schemas() || matches!(extra.data, Data::Object(_)) || matches!(extra.data, Data::Bool(_)))
        {
            for name in &names {
                if covered.contains(name) {
                    continue;
                }
                ann.props.insert((*name).to_owned());
                let item_path = format!("{ip}/{}", uri::escape_token(name));
                if matches!(extra.data, Data::Bool(false)) {
                    self.fail(&item_path, sp, "additionalProperties", format!("The property '{name}' is not allowed."));
                    ann.valid = false;
                } else if let (Some(v), false) = (value_of(name), matches!(extra.data, Data::Bool(true))) {
                    let r = self.eval(doc, extra, &format!("{sp}/additionalProperties"), v, &item_path, base, depth + 1);
                    if !r.valid {
                        ann.valid = false;
                    }
                }
            }
        }
        if draft >= Draft::D06
            && let Some(sub) = find(members, "propertyNames")
        {
            for m in object {
                let as_value = Value { span: m.key_span.clone(), data: Data::String(m.key.clone()) };
                let r =
                    self.eval(doc, sub, &format!("{sp}/propertyNames"), &as_value, &format!("{ip}/{}", uri::escape_token(&m.key)), base, depth + 1);
                if !r.valid {
                    ann.valid = false;
                }
            }
        }
        // dependencies (up to draft 7), dependentRequired / dependentSchemas (since 2019-09)
        let mut dependencies: Vec<(&str, &Value, &'static str)> = Vec::new();
        for (key, keyword) in [("dependencies", "dependencies"), ("dependentRequired", "dependentRequired"), ("dependentSchemas", "dependentSchemas")]
        {
            let applies = match keyword {
                "dependencies" => draft <= Draft::D07,
                _ => draft >= Draft::D2019_09,
            };
            if applies && let Some(Data::Object(list)) = find(members, key).map(|v| &v.data) {
                dependencies.extend(list.iter().map(|m| (m.key.as_str(), &m.value, keyword)));
            }
        }
        for (name, requirement, keyword) in dependencies {
            if !names.contains(&name) {
                continue;
            }
            match &requirement.data {
                Data::Array(required) => {
                    for need in required.iter().filter_map(text_of) {
                        if !names.contains(&need) {
                            self.fail(ip, sp, keyword, format!("The property '{need}' is required when '{name}' is present."));
                            ann.valid = false;
                        }
                    }
                }
                Data::Object(_) | Data::Bool(_) if keyword != "dependentRequired" => {
                    let at = format!("{sp}/{keyword}/{}", uri::escape_token(name));
                    let r = self.eval_object_as_instance(doc, requirement, &at, object, ip, base, depth + 1);
                    if r.valid {
                        ann.merge(r);
                    } else {
                        ann.valid = false;
                    }
                }
                _ => {}
            }
        }
    }

    /// Apply a schema to the object whose members are `object` (a `dependentSchemas` entry applies to the whole object).
    #[allow(clippy::too_many_arguments)]
    fn eval_object_as_instance(&mut self, doc: usize, schema: &Value, sp: &str, object: &[Member], ip: &str, base: &str, depth: usize) -> Ann {
        let whole = Value { span: 0..0, data: Data::Object(object.to_vec()) };
        self.eval(doc, schema, sp, &whole, ip, base, depth)
    }

    #[allow(clippy::too_many_arguments)]
    fn unevaluated(&mut self, doc: usize, members: &[Member], sp: &str, inst: &Value, ip: &str, base: &str, depth: usize, ann: &mut Ann) {
        if let (Some(sub), Data::Object(object)) = (find(members, "unevaluatedProperties"), &inst.data) {
            let mut seen = HashSet::new();
            for m in object {
                if !seen.insert(m.key.as_str()) || ann.props.contains(&m.key) {
                    continue;
                }
                let path = format!("{ip}/{}", uri::escape_token(&m.key));
                let r = self.eval(doc, sub, &format!("{sp}/unevaluatedProperties"), &m.value, &path, base, depth + 1);
                if !r.valid {
                    ann.valid = false;
                    if matches!(sub.data, Data::Bool(false))
                        && let Some(last) = self.violations.last_mut()
                    {
                        last.keyword = "unevaluatedProperties";
                        last.message = format!("The property '{}' is not allowed (it is not covered by the schema).", m.key);
                    }
                }
                ann.props.insert(m.key.clone());
            }
        }
        if let (Some(sub), Data::Array(items)) = (find(members, "unevaluatedItems"), &inst.data) {
            for (i, item) in items.iter().enumerate() {
                if ann.covers_item(i) {
                    continue;
                }
                let r = self.eval(doc, sub, &format!("{sp}/unevaluatedItems"), item, &format!("{ip}/{i}"), base, depth + 1);
                if !r.valid {
                    ann.valid = false;
                    if matches!(sub.data, Data::Bool(false))
                        && let Some(last) = self.violations.last_mut()
                    {
                        last.keyword = "unevaluatedItems";
                        last.message = format!("Item {i} is not allowed (it is not covered by the schema).");
                    }
                }
            }
            ann.items_all = true;
        }
    }
}

/// The number a keyword has, as written.
fn show(members: &[Member], key: &str) -> String {
    match find(members, key).map(|v| &v.data) {
        Some(Data::Number(n)) => n.clone(),
        _ => String::new(),
    }
}
