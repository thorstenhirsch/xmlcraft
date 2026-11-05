//! `xbrl_facts`: the facts, contexts and units of an XBRL 2.1 instance or an Inline XBRL document, and what the checks that need no
//! taxonomy find in them.
//!
//! The document is an open document (`id`), a file (`path`, through the root confinement) or `text`. Nothing is changed.

use serde_json::{Value, json};
use xmlcraft_text::Buffer;
use xmlcraft_xbrl::{Instance, Kind, Level, check, read};

use crate::documents::{fail, json_result, position_json, source, source_properties};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

/// The most facts one answer lists (the counts are complete).
const MAX_FACTS: usize = 2000;

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut properties = source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert("concept".into(), json!({ "type": "string", "description": "Only the facts of concepts whose name contains this text" }));
        map.insert(
            "max_facts".into(),
            json!({ "type": "integer", "minimum": 1, "maximum": MAX_FACTS, "description": "The most facts listed (default 200)" }),
        );
    }
    vec![ToolDef {
        name: "xbrl_facts",
        title: "List the facts of an XBRL instance",
        description: "Read an XBRL 2.1 instance (`xbrli:xbrl`) or an Inline XBRL document (`ix:` tags): the `kind`, the taxonomy `schema_refs`, the `contexts` (id, entity, period, \
                      dimensions), the `units`, the `facts` (concept, value with Inline scale and sign applied, context, unit, decimals, precision, nil, line and column) and the `findings` \
                      of the checks that need no taxonomy (a context or unit that is not defined, a period that is no date, decimals with precision, a nil fact with a value, a number \
                      that is not one, facts that say two things about one context, contexts and units never used). What needs the taxonomy is not checked. Nothing is changed.",
        input_schema: json!({ "type": "object", "properties": properties, "additionalProperties": false }),
        read_only: true,
        destructive: false,
        handler: xbrl_facts,
    }]
}

fn summary(instance: &Instance) -> Value {
    json!({
        "kind": match instance.kind { Kind::Instance => "instance", Kind::Inline => "inline" },
        "schema_refs": instance.schema_refs,
        "contexts": instance.contexts.iter().map(|c| json!({
            "id": c.id,
            "entity": { "scheme": c.entity_scheme, "id": c.entity },
            "period": c.period.as_ref().map(|p| p.label()),
            "dimensions": c.dimensions.iter().map(|d| json!({ "dimension": d.dimension, "member": d.member, "typed": d.typed, "scenario": d.in_scenario })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "units": instance.units.iter().map(|u| json!({ "id": u.id, "measure": u.measure })).collect::<Vec<_>>(),
    })
}

fn xbrl_facts(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let s = source(a, args)?;
    if !s.language.is_xml() {
        return fail("xbrl_facts reads XML documents");
    }
    let filter = match args.get("concept") {
        None => None,
        Some(Value::String(c)) => Some(c.as_str()),
        Some(_) => return Err(ToolError::InvalidArgs("concept is a string".into())),
    };
    let listed = match args.get("max_facts") {
        None => 200,
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| (1..=MAX_FACTS).contains(n))
            .ok_or_else(|| ToolError::InvalidArgs(format!("max_facts is a whole number from 1 to {MAX_FACTS}")))?,
    };
    let instance = read(&s.text).map_err(|e| ToolError::Failed(e.to_string()))?;
    let buffer = Buffer::from_text(&s.text);
    let place = |offset: usize| {
        let (line, column) = position_json(&buffer, offset.min(buffer.len()));
        (line, column)
    };
    let matching: Vec<&xmlcraft_xbrl::Fact> =
        instance.facts.iter().filter(|f| filter.is_none_or(|c| f.name.contains(c) || f.concept.contains(c))).collect();
    let facts: Vec<Value> = matching
        .iter()
        .take(listed)
        .map(|f| {
            let (line, column) = place(f.span.start);
            json!({
                "concept": f.concept, "name": f.name, "value": f.value, "context": f.context_ref, "unit": f.unit_ref,
                "decimals": f.decimals, "precision": f.precision, "nil": f.nil, "id": f.id, "line": line, "column": column,
            })
        })
        .collect();
    let findings: Vec<Value> = check(&instance)
        .into_iter()
        .take(MAX_FACTS)
        .map(|f| {
            let (line, column) = place(f.span.start);
            json!({ "code": f.code, "level": match f.level { Level::Error => "error", Level::Warning => "warning" }, "message": f.message, "line": line, "column": column })
        })
        .collect();
    let mut out = summary(&instance);
    out["fact_count"] = json!(matching.len());
    out["truncated"] = json!(matching.len() > facts.len());
    out["facts"] = Value::Array(facts);
    out["findings"] = Value::Array(findings);
    json_result(out)
}
