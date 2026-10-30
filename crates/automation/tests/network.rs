//! Schemas and DTDs that are not files: a session downloads them only when the program gave it a network that does. The web here is a
//! mock; nothing goes to the real one.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, Network};
use xmlcraft_resolve::{Conditions, FetchError, FetchLimits, FetchOutcome, Fetched, FixedClock, MemoryCache};

const SCHEMA: &str = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"><xs:element name=\"n\" type=\"xs:int\"/></xs:schema>";
const DTD: &str = "<!ELEMENT r (a+)><!ELEMENT a EMPTY>";
const DOCUMENT: &str =
    "<n xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"http://schemas.test/n.xsd\">seven</n>";

#[derive(Default)]
struct Web {
    asked: Mutex<Vec<String>>,
}

impl xmlcraft_resolve::Fetcher for Web {
    fn fetch(&self, url: &str, _: &Conditions, _: &FetchLimits) -> Result<FetchOutcome, FetchError> {
        self.asked.lock().expect("lock").push(url.to_owned());
        let body = if url.ends_with("schemas.test/n.xsd") {
            SCHEMA
        } else if url.ends_with("schemas.test/r.dtd") {
            DTD
        } else {
            return Err(FetchError::Status(404));
        };
        Ok(FetchOutcome::Fresh(Fetched { body: body.as_bytes().to_vec(), final_url: url.to_owned(), etag: None, last_modified: None }))
    }
}

fn session(web: &Arc<Web>) -> Automation {
    Automation::new().with_network(Network::new(Some(web.clone()), Some(Arc::new(MemoryCache::new())), Arc::new(FixedClock(1_000))))
}

fn validate(a: &mut Automation, text: &str) -> Value {
    match a.call("xml_validate", &json!({ "text": text })).expect("runs").into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    }
}

#[test]
fn a_session_with_a_network_downloads_the_schema_a_document_names() {
    let web = Arc::new(Web::default());
    let mut a = session(&web);
    let result = validate(&mut a, DOCUMENT);
    assert_eq!((result["validated"].clone(), result["valid"].clone(), result["errors"].clone()), (json!(true), json!(false), json!(1)), "{result}");
    assert!(result["problems"].to_string().contains("seven"), "{result}");
    assert!(!web.asked.lock().expect("lock").is_empty());
}

#[test]
fn a_dtd_that_is_not_a_file_is_downloaded_too() {
    let web = Arc::new(Web::default());
    let mut a = session(&web);
    let result = validate(&mut a, "<!DOCTYPE r SYSTEM \"http://schemas.test/r.dtd\"><r><b/></r>");
    assert_eq!(result["validated"], json!(true), "{result}");
    assert!(result["problems"].to_string().contains("vc.element-not-declared"), "{result}");
}

#[test]
fn a_session_without_one_downloads_nothing_and_says_why() {
    let web = Arc::new(Web::default());
    let mut a = Automation::new();
    let result = validate(&mut a, DOCUMENT);
    assert_eq!(result["validated"], json!(false), "{result}");
    assert!(result["note"].to_string().contains("offline"), "{result}");
    assert!(web.asked.lock().expect("lock").is_empty());
    // And one that has a network but works offline.
    let mut offline = session(&web);
    offline.network().set_offline(true);
    let result = validate(&mut offline, DOCUMENT);
    assert_eq!(result["validated"], json!(false), "{result}");
    assert!(web.asked.lock().expect("lock").is_empty());
}
