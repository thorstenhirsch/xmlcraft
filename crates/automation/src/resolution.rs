//! `schema_resolution`: how the schema or DTD a document names was looked up, place by place, found or not.

use serde_json::{Value, json};
use xmlcraft_engine::schema_fetch::fetch_traced;
use xmlcraft_engine::schema_link::schema_link;
use xmlcraft_resolve::{Outcome, Step};

use crate::documents::{analysis_of, fail, json_result, source, source_properties};
use crate::load_schema::link_json;
use crate::tools::ToolDef;
use crate::validate::schema_resolver;
use crate::{Automation, Content, Result};

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "schema_resolution",
        title: "Show how a document's schema or DTD was resolved",
        description: "Look up the grammar a document names and return the trail: every place the resolver looked in (the open documents, the user and bundled \
                      catalogs, the schemas that ship with the app, files, the cache and the network) and what each said. The document is an open document \
                      `id`, a file `path` or `text`; `link` is what it names. `found` is false with a `message` when no place had it, and the trail says why \
                      each one did not. The steps have a `stage`, what was asked (`what`), an `outcome` (found, missed, skipped or failed) and the `detail` of \
                      a skip or failure. Looking the grammar up reads it, as validation would: when the session works online that may download it \
                      (and keep it in the cache); offline, only the local places are tried. This is the top-level link; the documents a schema \
                      includes or imports are not in the trail.",
        input_schema: json!({ "type": "object", "properties": source_properties(), "additionalProperties": false }),
        read_only: true,
        destructive: false,
        handler: schema_resolution,
    }]
}

fn step_json(step: &Step) -> Value {
    let (outcome, detail) = match &step.outcome {
        Outcome::Found => ("found", Value::Null),
        Outcome::Missed => ("missed", Value::Null),
        Outcome::Skipped(why) => ("skipped", json!(why)),
        Outcome::Failed(why) => ("failed", json!(why)),
    };
    json!({ "stage": step.stage.to_string(), "what": step.what, "outcome": outcome, "detail": detail })
}

fn schema_resolution(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let document = source(a, args)?;
    let analysis = analysis_of(&document)?;
    let Some(link) = schema_link(&analysis) else { return fail("the document names no schema or DTD") };
    let base = document.path.as_deref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let resolver = schema_resolver(a);
    let (result, trail) = fetch_traced(&resolver, &link, &base);
    let steps: Vec<Value> = trail.steps.iter().map(step_json).collect();
    let mut value = json!({
        "link": link_json(&link),
        "steps": steps,
        "trail": trail.to_string(),
    });
    match result {
        Ok(found) => {
            value["found"] = json!(true);
            value["location"] = json!(found.location);
            value["origin"] = json!(found.origin_name());
            value["lossy"] = json!(found.lossy);
        }
        Err(message) => {
            value["found"] = json!(false);
            value["message"] = json!(message);
        }
    }
    json_result(value)
}
