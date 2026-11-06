//! The taxonomy an instance names: what its schema documents declare, and the checks of the facts that need it.

use std::collections::HashMap;

use xmlcraft_xbrl::taxonomy::{Loader, check_taxonomy, load};
use xmlcraft_xbrl::{Level, read};

struct Files(HashMap<&'static str, String>);

impl Loader for Files {
    fn load(&mut self, location: &str, base: &str) -> Result<(String, String), String> {
        let key = location.rsplit('/').next().unwrap_or(location);
        let _ = base;
        self.0.get(key).map(|t| (format!("mem:{key}"), t.clone())).ok_or_else(|| format!("{location}: no such document"))
    }
}

const XSD_HEAD: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xbrli="http://www.xbrl.org/2003/instance" xmlns:eg="urn:eg" targetNamespace="urn:eg" elementFormDefault="qualified">"#;

fn taxonomy_files() -> Files {
    let eg = format!(
        r#"{XSD_HEAD}
  <xs:import namespace="urn:base" schemaLocation="base.xsd"/>
  <xs:complexType name="euroType"><xs:simpleContent><xs:restriction base="xbrli:monetaryItemType"/></xs:simpleContent></xs:complexType>
  <xs:element name="Revenue" type="xbrli:monetaryItemType" substitutionGroup="xbrli:item" xbrli:periodType="duration"/>
  <xs:element name="Assets" type="eg:euroType" substitutionGroup="xbrli:item" xbrli:periodType="instant"/>
  <xs:element name="Remark" type="xbrli:stringItemType" substitutionGroup="xbrli:item" xbrli:periodType="duration"/>
  <xs:element name="Heading" type="xbrli:stringItemType" substitutionGroup="xbrli:item" abstract="true" xbrli:periodType="duration"/>
  <xs:element name="Group" substitutionGroup="xbrli:tuple"/>
</xs:schema>"#
    );
    let base = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xbrli="http://www.xbrl.org/2003/instance" targetNamespace="urn:base"><xs:element name="Count" type="xbrli:integerItemType" substitutionGroup="xbrli:item" xbrli:periodType="instant"/></xs:schema>"#.to_owned();
    Files(HashMap::from([("eg.xsd", eg), ("base.xsd", base)]))
}

fn instance(facts: &str) -> String {
    format!(
        r#"<xbrli:xbrl xmlns:xbrli="http://www.xbrl.org/2003/instance" xmlns:link="http://www.xbrl.org/2003/linkbase" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:iso4217="http://www.xbrl.org/2003/iso4217" xmlns:eg="urn:eg" xmlns:b="urn:base"><link:schemaRef xlink:type="simple" xlink:href="eg.xsd"/>
<xbrli:context id="i"><xbrli:entity><xbrli:identifier scheme="s">x</xbrli:identifier></xbrli:entity><xbrli:period><xbrli:instant>2025-12-31</xbrli:instant></xbrli:period></xbrli:context>
<xbrli:context id="d"><xbrli:entity><xbrli:identifier scheme="s">x</xbrli:identifier></xbrli:entity><xbrli:period><xbrli:startDate>2025-01-01</xbrli:startDate><xbrli:endDate>2025-12-31</xbrli:endDate></xbrli:period></xbrli:context>
<xbrli:unit id="u"><xbrli:measure>iso4217:EUR</xbrli:measure></xbrli:unit>{facts}</xbrli:xbrl>"#
    )
}

fn codes(facts: &str) -> Vec<&'static str> {
    let text = instance(facts);
    let inst = read(&text).expect("reads");
    let taxonomy = load(&inst.schema_refs, "", &mut taxonomy_files());
    check_taxonomy(&inst, &taxonomy).into_iter().map(|f| f.code).collect()
}

#[test]
fn the_concepts_of_the_schemas_an_instance_names_and_what_they_import_are_read_with_their_types() {
    let inst = read(&instance("")).expect("reads");
    let t = load(&inst.schema_refs, "", &mut taxonomy_files());
    assert_eq!(t.documents.len(), 2, "the schemaRef and its import");
    assert!(t.namespaces.contains("urn:eg") && t.namespaces.contains("urn:base") && t.not_loaded.is_empty(), "{:?}", t.not_loaded);
    assert_eq!(t.concepts.len(), 6);
    let assets = &t.concepts["{urn:eg}Assets"];
    assert_eq!(assets.period_type, Some(xmlcraft_xbrl::taxonomy::PeriodType::Instant));
    assert_eq!(t.is_numeric_type(assets.type_name.as_deref().expect("a type")), Some(true), "a type derived from monetaryItemType is a number");
    assert_eq!(t.is_numeric_type(t.concepts["{urn:eg}Remark"].type_name.as_deref().expect("a type")), Some(false));
    assert_eq!(t.is_item(assets), Some(true));
    assert_eq!(t.is_item(&t.concepts["{urn:eg}Group"]), Some(false), "a tuple");
}

#[test]
fn a_correct_instance_has_nothing_to_say_and_each_mistake_is_its_own_finding() {
    assert_eq!(
        codes(
            r#"<eg:Revenue contextRef="d" unitRef="u" decimals="0">1</eg:Revenue><eg:Assets contextRef="i" unitRef="u" decimals="0">2</eg:Assets><eg:Remark contextRef="d">text</eg:Remark><b:Count contextRef="i" unitRef="u" decimals="0">3</b:Count>"#
        ),
        Vec::<&str>::new()
    );
    assert_eq!(codes(r#"<eg:Nothing contextRef="d">x</eg:Nothing>"#), ["xbrl.unknown-concept"]);
    assert_eq!(codes(r#"<eg:Heading contextRef="d">x</eg:Heading>"#), ["xbrl.abstract-fact"]);
    assert_eq!(codes(r#"<eg:Revenue contextRef="d" decimals="0">1</eg:Revenue>"#), ["xbrl.numeric-needs-unit"]);
    assert_eq!(codes(r#"<eg:Remark contextRef="d" unitRef="u">x</eg:Remark>"#), ["xbrl.unit-on-non-numeric"]);
    assert_eq!(codes(r#"<eg:Revenue contextRef="d" unitRef="u">1</eg:Revenue>"#), ["xbrl.numeric-needs-precision"]);
    assert_eq!(
        codes(r#"<eg:Assets contextRef="d" unitRef="u" decimals="0">1</eg:Assets>"#),
        ["xbrl.period-type"],
        "an instant concept in a duration"
    );
    assert_eq!(
        codes(r#"<eg:Revenue contextRef="i" unitRef="u" decimals="0">1</eg:Revenue>"#),
        ["xbrl.period-type"],
        "a duration concept at an instant"
    );
    // A nil number needs no unit or precision.
    assert_eq!(codes(r#"<eg:Revenue contextRef="d" xsi:nil="true" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"/>"#), Vec::<&str>::new());
}

#[test]
fn what_cannot_be_read_of_the_taxonomy_is_a_warning_and_what_it_does_not_cover_is_not_judged() {
    let text = instance(r#"<eg:Revenue contextRef="d" unitRef="u" decimals="0">1</eg:Revenue>"#);
    let inst = read(&text).expect("reads");
    // No document at all: the schemaRef is not loaded, and the namespace is said once and not judged.
    let t = load(&inst.schema_refs, "", &mut Files(HashMap::new()));
    let findings = check_taxonomy(&inst, &t);
    let codes: Vec<&str> = findings.iter().map(|f| f.code).collect();
    assert_eq!(codes, ["xbrl.taxonomy-not-loaded"], "said once, not again for each namespace");
    assert!(findings.iter().all(|f| f.level == Level::Warning));
    // A taxonomy that was read but has not the namespace of a fact: that namespace is said once and not judged.
    let other = instance(
        r#"<other:Thing xmlns:other="urn:other" contextRef="d">x</other:Thing><other:More xmlns:other="urn:other" contextRef="d">y</other:More>"#,
    );
    let inst2 = read(&other).expect("reads");
    let t2 = load(&inst2.schema_refs, "", &mut taxonomy_files());
    let codes2: Vec<&str> = check_taxonomy(&inst2, &t2).iter().map(|f| f.code).collect();
    assert_eq!(codes2, ["xbrl.taxonomy-namespace-missing"]);
    // A document that is not a schema, a broken one, and a loop of imports end.
    let mut files = Files(HashMap::from([("eg.xsd", "<a/>".to_owned())]));
    assert_eq!(load(&inst.schema_refs, "", &mut files).not_loaded.len(), 1);
    let mut files = Files(HashMap::from([("eg.xsd", "<xs:schema".to_owned())]));
    assert_eq!(load(&inst.schema_refs, "", &mut files).not_loaded.len(), 1);
    let looping = format!("{XSD_HEAD}<xs:import namespace=\"urn:eg\" schemaLocation=\"eg.xsd\"/></xs:schema>");
    let mut files = Files(HashMap::from([("eg.xsd", looping)]));
    let t = load(&inst.schema_refs, "", &mut files);
    assert_eq!(t.documents.len(), 1, "a document is read once");
}
