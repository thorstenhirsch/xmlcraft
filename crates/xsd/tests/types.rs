//! The simple types of a schema: resolved by name in any order, derived, listed and united; what is wrong is reported.

use xmlcraft_datatypes::{NoNamespaces, Value};
use xmlcraft_xsd::model::QName;
use xmlcraft_xsd::schema::{Schema, TypeKind};
use xmlcraft_xsd::{Assembly, LoadRequest, Loaded, NoLoader, SchemaLoader, XSD_NAMESPACE, assemble, load};

const NS: &str = "urn:t";

fn schema_text(body: &str) -> String {
    format!("<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" xmlns:t=\"{NS}\" targetNamespace=\"{NS}\">{body}</xs:schema>")
}

fn build(body: &str) -> Assembly {
    let documents = load(&schema_text(body), "/s/root.xsd", &mut NoLoader);
    // The mistakes of reading are not what these tests are about.
    assert!(documents.diagnostics.is_empty(), "{:?}", documents.diagnostics.iter().map(|d| d.diagnostic.message()).collect::<Vec<_>>());
    assemble(&documents)
}

fn keys(a: &Assembly) -> Vec<&'static str> {
    a.diagnostics.iter().map(|d| d.diagnostic.code.key()).collect()
}

fn messages(a: &Assembly) -> Vec<String> {
    a.diagnostics.iter().map(|d| d.diagnostic.message()).collect()
}

fn valid(schema: &Schema, local: &str, lexical: &str) -> bool {
    let id = schema.type_by_name(&QName::new(Some(NS), local)).unwrap_or_else(|| panic!("no type {local}"));
    match &schema.type_entry(id).expect("an entry").kind {
        TypeKind::Simple(t) => t.validate(lexical, &NoNamespaces).is_ok(),
        TypeKind::Complex(_) => panic!("{local} is complex"),
    }
}

#[test]
fn the_built_in_types_are_there() {
    let a = build("");
    assert!(a.diagnostics.is_empty());
    let xs = |l: &str| QName::new(Some(XSD_NAMESPACE), l);
    let any = a.schema.type_by_name(&xs("anyType")).expect("anyType");
    assert!(matches!(a.schema.type_entry(any).map(|t| &t.kind), Some(TypeKind::Complex(_))));
    let simple = a.schema.type_by_name(&xs("anySimpleType")).expect("anySimpleType");
    assert_eq!(a.schema.type_entry(simple).and_then(|t| t.base), Some(any), "anySimpleType is derived from anyType");
    let integer = a.schema.type_by_name(&xs("integer")).expect("integer");
    assert_eq!(a.schema.type_entry(integer).and_then(|t| t.base), a.schema.type_by_name(&xs("decimal")));
    let string = a.schema.type_by_name(&xs("string")).expect("string");
    assert_eq!(a.schema.type_entry(string).and_then(|t| t.base), Some(simple));
    for name in ["boolean", "date", "ID", "NMTOKENS", "unsignedByte", "token", "QName"] {
        assert!(a.schema.type_by_name(&xs(name)).is_some(), "{name}");
    }
}

#[test]
fn restrictions_derive_in_any_order() {
    let a = build(
        "<xs:simpleType name=\"short3\"><xs:restriction base=\"t:code\"><xs:maxLength value=\"3\"/></xs:restriction></xs:simpleType>\
         <xs:simpleType name=\"code\"><xs:restriction base=\"xs:string\"><xs:minLength value=\"2\"/><xs:pattern value=\"[A-Z]+\"/></xs:restriction></xs:simpleType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    assert!(valid(&a.schema, "short3", "AB") && valid(&a.schema, "short3", "ABC"));
    assert!(!valid(&a.schema, "short3", "A") && !valid(&a.schema, "short3", "ABCD") && !valid(&a.schema, "short3", "ab"));
    let short = a.schema.type_by_name(&QName::new(Some(NS), "short3")).expect("short3");
    let code = a.schema.type_by_name(&QName::new(Some(NS), "code")).expect("code");
    assert_eq!(a.schema.type_entry(short).and_then(|t| t.base), Some(code));
    assert_eq!(a.schema.type_entry(short).and_then(|t| t.name.clone()), Some(QName::new(Some(NS), "short3")));
}

#[test]
fn an_inline_base_and_a_whole_tower_of_restrictions() {
    let a = build(
        "<xs:simpleType name=\"pct\"><xs:restriction><xs:simpleType><xs:restriction base=\"xs:integer\"><xs:minInclusive value=\"0\"/></xs:restriction></xs:simpleType><xs:maxInclusive value=\"100\"/></xs:restriction></xs:simpleType>\
         <xs:simpleType name=\"half\"><xs:restriction base=\"t:pct\"><xs:maxInclusive value=\"50\"/></xs:restriction></xs:simpleType>\
         <xs:simpleType name=\"enum\"><xs:restriction base=\"xs:token\"><xs:enumeration value=\"a\"/><xs:enumeration value=\"b c\"/></xs:restriction></xs:simpleType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    assert!(valid(&a.schema, "pct", "100") && !valid(&a.schema, "pct", "101") && !valid(&a.schema, "pct", "-1"));
    assert!(valid(&a.schema, "half", "50") && !valid(&a.schema, "half", "51"));
    assert!(valid(&a.schema, "enum", " b   c ") && !valid(&a.schema, "enum", "c"));
}

#[test]
fn lists_and_unions() {
    let a = build(
        "<xs:simpleType name=\"ints\"><xs:list itemType=\"xs:int\"/></xs:simpleType>\
         <xs:simpleType name=\"codes\"><xs:list><xs:simpleType><xs:restriction base=\"xs:string\"><xs:length value=\"2\"/></xs:restriction></xs:simpleType></xs:list></xs:simpleType>\
         <xs:simpleType name=\"num\"><xs:union memberTypes=\"xs:int t:codes\"><xs:simpleType><xs:restriction base=\"xs:boolean\"/></xs:simpleType></xs:union></xs:simpleType>\
         <xs:simpleType name=\"threeints\"><xs:restriction base=\"t:ints\"><xs:length value=\"3\"/></xs:restriction></xs:simpleType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    assert!(valid(&a.schema, "ints", "1 2 3") && valid(&a.schema, "ints", "") && !valid(&a.schema, "ints", "1 x"));
    assert!(valid(&a.schema, "codes", "ab cd") && !valid(&a.schema, "codes", "abc"));
    assert!(valid(&a.schema, "num", "5") && valid(&a.schema, "num", "true") && valid(&a.schema, "num", "ab cd") && !valid(&a.schema, "num", "abc"));
    assert!(valid(&a.schema, "threeints", "1 2 3") && !valid(&a.schema, "threeints", "1 2"));
}

#[test]
fn a_qualified_name_in_an_enumeration_is_read_in_the_scope_of_the_facet() {
    let a = build("<xs:simpleType name=\"q\"><xs:restriction base=\"xs:QName\"><xs:enumeration value=\"t:a\"/></xs:restriction></xs:simpleType>");
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let id = a.schema.type_by_name(&QName::new(Some(NS), "q")).expect("q");
    let TypeKind::Simple(t) = &a.schema.type_entry(id).expect("entry").kind else { panic!("simple") };
    struct Ns;
    impl xmlcraft_datatypes::NamespaceContext for Ns {
        fn resolve(&self, prefix: Option<&str>) -> Option<String> {
            (prefix == Some("p")).then(|| NS.to_owned())
        }
    }
    assert!(matches!(t.validate("p:a", &Ns), Ok(Value::QName(_))));
    assert!(t.validate("p:b", &Ns).is_err());
}

#[test]
fn facet_mistakes_are_reported_and_the_type_stays_usable() {
    let a = build(
        "<xs:simpleType name=\"bad\"><xs:restriction base=\"xs:string\"><xs:minLength value=\"5\"/><xs:maxLength value=\"2\"/></xs:restriction></xs:simpleType>\
         <xs:simpleType name=\"user\"><xs:restriction base=\"t:bad\"><xs:pattern value=\"a\"/></xs:restriction></xs:simpleType>",
    );
    assert_eq!(keys(&a), ["xsd.facet"]);
    assert!(messages(&a)[0].contains("bad") && messages(&a)[0].contains("greater than maxLength"), "{:?}", messages(&a));
    assert!(valid(&a.schema, "user", "a"), "the type that uses it is not reported for it as well");
    let a = build("<xs:simpleType name=\"t\"><xs:restriction base=\"xs:int\"><xs:length value=\"2\"/></xs:restriction></xs:simpleType>");
    assert!(messages(&a)[0].contains("length does not apply"), "{:?}", messages(&a));
    let a = build(
        "<xs:simpleType name=\"t\"><xs:restriction base=\"xs:int\"><xs:minInclusive value=\"1\"/><xs:minInclusive value=\"2\"/></xs:restriction></xs:simpleType>",
    );
    assert!(messages(&a)[0].contains("minInclusive is given more than once"), "{:?}", messages(&a));
    let a = build("<xs:simpleType name=\"t\"><xs:restriction base=\"xs:int\"><xs:pattern value=\"(\"/></xs:restriction></xs:simpleType>");
    assert!(messages(&a)[0].contains("pattern '('"), "{:?}", messages(&a));
    assert_eq!(a.diagnostics[0].source, 0);
}

#[test]
fn names_that_do_not_resolve() {
    let a = build("<xs:simpleType name=\"t\"><xs:restriction base=\"t:nothing\"/></xs:simpleType>");
    assert_eq!(keys(&a), ["xsd.unresolved-reference"]);
    assert!(messages(&a)[0].contains("{urn:t}nothing"), "{:?}", messages(&a));
    let a = build("<xs:simpleType name=\"t\"><xs:restriction base=\"xs:nothing\"/></xs:simpleType>");
    assert_eq!(keys(&a), ["xsd.unresolved-reference"]);
    let a = build(
        "<xs:simpleType name=\"t\"><xs:list itemType=\"t:nothing\"/></xs:simpleType><xs:simpleType name=\"u\"><xs:union memberTypes=\"xs:int t:nothing\"/></xs:simpleType>",
    );
    assert_eq!(keys(&a), ["xsd.unresolved-reference", "xsd.unresolved-reference"]);
    // A complex type where a simple type is needed.
    let a = build("<xs:simpleType name=\"t\"><xs:restriction base=\"xs:anyType\"/></xs:simpleType>");
    assert_eq!(keys(&a), ["xsd.wrong-kind-of-type"]);
}

#[test]
fn definitions_that_go_round_in_a_circle_are_reported_once() {
    let a = build(
        "<xs:simpleType name=\"a\"><xs:restriction base=\"t:b\"/></xs:simpleType><xs:simpleType name=\"b\"><xs:restriction base=\"t:a\"/></xs:simpleType>\
         <xs:simpleType name=\"c\"><xs:list itemType=\"t:c\"/></xs:simpleType>",
    );
    assert_eq!(keys(&a), ["xsd.circular-definition", "xsd.circular-definition"], "once for the pair a and b, once for c: {:?}", messages(&a));
}

#[test]
fn a_type_defined_twice_is_reported() {
    let a = build(
        "<xs:simpleType name=\"t\"><xs:restriction base=\"xs:string\"/></xs:simpleType><xs:simpleType name=\"t\"><xs:restriction base=\"xs:int\"/></xs:simpleType>",
    );
    assert_eq!(keys(&a), ["xsd.duplicate-definition"]);
    assert!(valid(&a.schema, "t", "abc"), "the first definition counts");
}

#[test]
fn what_is_final_cannot_be_derived() {
    let a = build(
        "<xs:simpleType name=\"f\" final=\"restriction\"><xs:restriction base=\"xs:string\"/></xs:simpleType>\
         <xs:simpleType name=\"r\"><xs:restriction base=\"t:f\"/></xs:simpleType>\
         <xs:simpleType name=\"l\" final=\"list\"><xs:restriction base=\"xs:string\"/></xs:simpleType>\
         <xs:simpleType name=\"ll\"><xs:list itemType=\"t:l\"/></xs:simpleType>\
         <xs:simpleType name=\"u\" final=\"union\"><xs:restriction base=\"xs:string\"/></xs:simpleType>\
         <xs:simpleType name=\"uu\"><xs:union memberTypes=\"t:u xs:int\"/></xs:simpleType>\
         <xs:simpleType name=\"ok\" final=\"list union\"><xs:restriction base=\"xs:string\"/></xs:simpleType><xs:simpleType name=\"okr\"><xs:restriction base=\"t:ok\"/></xs:simpleType>",
    );
    assert_eq!(keys(&a), ["xsd.derivation-not-allowed", "xsd.derivation-not-allowed", "xsd.derivation-not-allowed"]);
    // finalDefault of the schema element.
    let text = format!(
        "<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" targetNamespace=\"{NS}\" xmlns:t=\"{NS}\" finalDefault=\"restriction\"><xs:simpleType name=\"a\"><xs:restriction base=\"xs:string\"/></xs:simpleType><xs:simpleType name=\"b\"><xs:restriction base=\"t:a\"/></xs:simpleType><xs:simpleType name=\"c\" final=\"\"><xs:restriction base=\"xs:string\"/></xs:simpleType><xs:simpleType name=\"d\"><xs:restriction base=\"t:c\"/></xs:simpleType></xs:schema>"
    );
    let docs = load(&text, "/s/r.xsd", &mut NoLoader);
    let a = assemble(&docs);
    assert_eq!(keys(&a), ["xsd.derivation-not-allowed"], "a is final by default, c says it is not");
}

struct Files(Vec<(&'static str, String)>);

impl SchemaLoader for Files {
    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String> {
        let wanted = request.location.unwrap_or_default();
        self.0
            .iter()
            .find(|(l, _)| l.ends_with(wanted))
            .map(|(l, t)| Loaded { text: t.clone(), location: (*l).to_owned() })
            .ok_or_else(|| "no".to_owned())
    }
}

#[test]
fn names_from_other_namespaces_need_an_import_and_included_documents_share_the_namespace() {
    let other = format!(
        "<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" targetNamespace=\"urn:o\"><xs:simpleType name=\"o\"><xs:restriction base=\"xs:string\"><xs:maxLength value=\"1\"/></xs:restriction></xs:simpleType></xs:schema>"
    );
    let included = format!(
        "<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" targetNamespace=\"{NS}\"><xs:simpleType name=\"inc\"><xs:restriction base=\"xs:string\"/></xs:simpleType></xs:schema>"
    );
    let chameleon = format!(
        "<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\"><xs:simpleType name=\"cham\"><xs:restriction base=\"inc\"><xs:maxLength value=\"2\"/></xs:restriction></xs:simpleType></xs:schema>"
    );
    let uses = |import: &str| {
        format!(
            "<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" xmlns:o=\"urn:o\" xmlns:t=\"{NS}\" targetNamespace=\"{NS}\">{import}<xs:include schemaLocation=\"inc.xsd\"/><xs:include schemaLocation=\"cham.xsd\"/>\
             <xs:simpleType name=\"x\"><xs:restriction base=\"o:o\"/></xs:simpleType><xs:simpleType name=\"y\"><xs:restriction base=\"t:cham\"/></xs:simpleType></xs:schema>"
        )
    };
    let mut files = Files(vec![("/s/o.xsd", other.clone()), ("/s/inc.xsd", included.clone()), ("/s/cham.xsd", chameleon.clone())]);
    let docs = load(&uses("<xs:import namespace=\"urn:o\" schemaLocation=\"o.xsd\"/>"), "/s/root.xsd", &mut files);
    let a = assemble(&docs);
    assert!(docs.diagnostics.is_empty() && a.diagnostics.is_empty(), "{:?} {:?}", docs.diagnostics, messages(&a));
    assert!(valid(&a.schema, "x", "a") && !valid(&a.schema, "x", "ab"));
    assert!(
        valid(&a.schema, "y", "ab") && !valid(&a.schema, "y", "abc"),
        "the chameleon's reference to `inc` is in the namespace it was included into"
    );
    // Without the import the namespace is not visible.
    let mut files = Files(vec![("/s/o.xsd", other), ("/s/inc.xsd", included), ("/s/cham.xsd", chameleon)]);
    let docs = load(&uses(""), "/s/root.xsd", &mut files);
    let a = assemble(&docs);
    assert_eq!(keys(&a), ["xsd.namespace-not-imported"]);
}

#[test]
fn a_very_long_chain_of_derivations_is_refused_not_followed_to_the_end() {
    // The last one is defined first, so that building it means building all the others first.
    let mut body = String::new();
    for i in (1..2000).rev() {
        body.push_str(&format!("<xs:simpleType name=\"t{i}\"><xs:restriction base=\"t:t{}\"/></xs:simpleType>", i - 1));
    }
    body.push_str("<xs:simpleType name=\"t0\"><xs:restriction base=\"xs:string\"/></xs:simpleType>");
    let a = build(&body);
    assert!(keys(&a).contains(&"xsd.too-deep"), "{} diagnostics", keys(&a).len());
}
