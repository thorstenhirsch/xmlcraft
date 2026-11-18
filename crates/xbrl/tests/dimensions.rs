//! XBRL Dimensions 1.0 with a small synthetic taxonomy: the dimensional model read from its definition linkbase, and the dimensional
//! validation of facts and contexts. Each rule has an instance that keeps it and one that breaks it with exactly one finding.
//!
//! The taxonomy (namespace `urn:eg`):
//!
//! - role `main`: `Sales` has the closed hypercube `SalesCube` (segment) with the explicit dimension `RegionDim` (domain `RegionDomain`
//!   with `North`, `South` and `East`, whose arc a second link prohibits; default `RegionDomain`) and the typed dimension `CustomerDim`;
//!   `Revenue` is a child of `Sales` and inherits its hypercube; `Cost` has the open hypercube `OpenCube` (scenario) with `RegionDim`.
//! - role `units`: `Units` has `OpenCube` (through `targetRole` to `main`) and, as `notAll`, `ExclCube` (through `targetRole` to `excl`).
//! - role `excl`: `ExclCube` has `RegionDim` with the domain `South` alone.
//! - `OtherDim` is an explicit dimension of no hypercube.

use std::collections::HashMap;

use xmlcraft_xbrl::dims::ContextElement;
use xmlcraft_xbrl::taxonomy::{Loader, Taxonomy, check_taxonomy, load};
use xmlcraft_xbrl::{Finding, Level, read};

struct Files(HashMap<&'static str, String>);

impl Loader for Files {
    fn load(&mut self, location: &str, _base: &str) -> Result<(String, String), String> {
        let key = location.rsplit('/').next().unwrap_or(location);
        self.0.get(key).map(|t| (format!("mem:{key}"), t.clone())).ok_or_else(|| format!("{location}: no such document"))
    }
}

const MAIN: &str = "http://example.com/role/main";
const UNITS: &str = "http://example.com/role/units";
const EXCL: &str = "http://example.com/role/excl";

fn schema(extra_elements: &str) -> String {
    format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xbrli="http://www.xbrl.org/2003/instance" xmlns:xbrldt="http://xbrl.org/2005/xbrldt" xmlns:link="http://www.xbrl.org/2003/linkbase" xmlns:xlink="http://www.w3.org/1999/xlink" targetNamespace="urn:eg">
  <xs:annotation><xs:appinfo><link:linkbaseRef xlink:type="simple" xlink:href="def.xml" xlink:role="http://www.xbrl.org/2003/role/definitionLinkbaseRef" xlink:arcrole="http://www.w3.org/1999/xlink/properties/linkbase"/></xs:appinfo></xs:annotation>
  <xs:element id="eg_Sales" name="Sales" type="xbrli:monetaryItemType" substitutionGroup="xbrli:item" xbrli:periodType="duration"/>
  <xs:element id="eg_Revenue" name="Revenue" type="xbrli:monetaryItemType" substitutionGroup="xbrli:item" xbrli:periodType="duration"/>
  <xs:element id="eg_Cost" name="Cost" type="xbrli:monetaryItemType" substitutionGroup="xbrli:item" xbrli:periodType="duration"/>
  <xs:element id="eg_Units" name="Units" type="xbrli:monetaryItemType" substitutionGroup="xbrli:item" xbrli:periodType="duration"/>
  <xs:element id="eg_Plain" name="Plain" type="xbrli:monetaryItemType" substitutionGroup="xbrli:item" xbrli:periodType="duration"/>
  <xs:element id="eg_SalesCube" name="SalesCube" type="xbrli:stringItemType" substitutionGroup="xbrldt:hypercubeItem" abstract="true" xbrli:periodType="duration"/>
  <xs:element id="eg_OpenCube" name="OpenCube" type="xbrli:stringItemType" substitutionGroup="xbrldt:hypercubeItem" abstract="true" xbrli:periodType="duration"/>
  <xs:element id="eg_ExclCube" name="ExclCube" type="xbrli:stringItemType" substitutionGroup="xbrldt:hypercubeItem" abstract="true" xbrli:periodType="duration"/>
  <xs:element id="eg_RegionDim" name="RegionDim" type="xbrli:stringItemType" substitutionGroup="xbrldt:dimensionItem" abstract="true" xbrli:periodType="duration"/>
  <xs:element id="eg_OtherDim" name="OtherDim" type="xbrli:stringItemType" substitutionGroup="xbrldt:dimensionItem" abstract="true" xbrli:periodType="duration"/>
  <xs:element id="eg_CustomerDim" name="CustomerDim" type="xbrli:stringItemType" substitutionGroup="xbrldt:dimensionItem" abstract="true" xbrli:periodType="duration" xbrldt:typedDomainRef="eg.xsd#eg_CustomerId"/>
  <xs:element id="eg_CustomerId" name="CustomerId" type="xs:string"/>
  <xs:element id="eg_RegionDomain" name="RegionDomain" type="xbrli:stringItemType" substitutionGroup="xbrli:item" abstract="true" xbrli:periodType="duration"/>
  <xs:element id="eg_North" name="North" type="xbrli:stringItemType" substitutionGroup="xbrli:item" abstract="true" xbrli:periodType="duration"/>
  <xs:element id="eg_South" name="South" type="xbrli:stringItemType" substitutionGroup="xbrli:item" abstract="true" xbrli:periodType="duration"/>
  <xs:element id="eg_East" name="East" type="xbrli:stringItemType" substitutionGroup="xbrli:item" abstract="true" xbrli:periodType="duration"/>
  <xs:element id="eg_OtherDomain" name="OtherDomain" type="xbrli:stringItemType" substitutionGroup="xbrli:item" abstract="true" xbrli:periodType="duration"/>
  {extra_elements}
</xs:schema>"#
    )
}

/// One definition link in `role`: arcs as (arcrole, from, to, extra attributes).
fn link(role: &str, arcs: &[(&str, &str, &str, &str)]) -> String {
    let mut locs: Vec<&str> = Vec::new();
    for (_, from, to, _) in arcs {
        for c in [from, to] {
            if !locs.contains(c) {
                locs.push(c);
            }
        }
    }
    let mut s = format!(r#"<link:definitionLink xlink:type="extended" xlink:role="{role}">"#);
    for c in &locs {
        s.push_str(&format!(r#"<link:loc xlink:type="locator" xlink:href="eg.xsd#eg_{c}" xlink:label="{c}"/>"#));
    }
    for (arcrole, from, to, extra) in arcs {
        s.push_str(&format!(
            r#"<link:definitionArc xlink:type="arc" xlink:arcrole="http://xbrl.org/int/dim/arcrole/{arcrole}" xlink:from="{from}" xlink:to="{to}" {extra}/>"#
        ));
    }
    s.push_str("</link:definitionLink>");
    s
}

fn linkbase(links: &[String]) -> String {
    format!(
        r#"<link:linkbase xmlns:link="http://www.xbrl.org/2003/linkbase" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:xbrldt="http://xbrl.org/2005/xbrldt">{}</link:linkbase>"#,
        links.concat()
    )
}

fn good_links() -> Vec<String> {
    let target_main = format!(r#"xbrldt:contextElement="scenario" xbrldt:targetRole="{MAIN}""#);
    let target_excl = format!(r#"xbrldt:contextElement="scenario" xbrldt:targetRole="{EXCL}""#);
    vec![
        link(
            MAIN,
            &[
                ("all", "Sales", "SalesCube", r#"xbrldt:contextElement="segment" xbrldt:closed="true""#),
                ("domain-member", "Sales", "Revenue", ""),
                ("hypercube-dimension", "SalesCube", "RegionDim", ""),
                ("hypercube-dimension", "SalesCube", "CustomerDim", ""),
                ("dimension-domain", "RegionDim", "RegionDomain", ""),
                ("domain-member", "RegionDomain", "North", r#"order="1""#),
                ("domain-member", "RegionDomain", "South", r#"order="2""#),
                ("domain-member", "RegionDomain", "East", r#"order="3""#),
                ("all", "Cost", "OpenCube", r#"xbrldt:contextElement="scenario""#),
                ("hypercube-dimension", "OpenCube", "RegionDim", ""),
                ("dimension-default", "RegionDim", "RegionDomain", ""),
                ("dimension-domain", "OtherDim", "OtherDomain", ""),
            ],
        ),
        // A later link takes East out of the domain.
        link(MAIN, &[("domain-member", "RegionDomain", "East", r#"use="prohibited" priority="1""#)]),
        link(UNITS, &[("all", "Units", "OpenCube", &target_main), ("notAll", "Units", "ExclCube", &target_excl)]),
        link(EXCL, &[("hypercube-dimension", "ExclCube", "RegionDim", ""), ("dimension-domain", "RegionDim", "South", "")]),
    ]
}

fn files_with(extra_elements: &str, links: &[String]) -> Files {
    Files(HashMap::from([("eg.xsd", schema(extra_elements)), ("def.xml", linkbase(links))]))
}

fn taxonomy() -> Taxonomy {
    load(&["eg.xsd".to_owned()], "", &mut files_with("", &good_links()))
}

const PERIOD: &str = r#"<xbrli:period><xbrli:startDate>2025-01-01</xbrli:startDate><xbrli:endDate>2025-12-31</xbrli:endDate></xbrli:period>"#;

/// A context `c` with `segment` (the content of the segment) and `scenario`.
fn context(segment: &str, scenario: &str) -> String {
    let segment = if segment.is_empty() { String::new() } else { format!("<xbrli:segment>{segment}</xbrli:segment>") };
    let scenario = if scenario.is_empty() { String::new() } else { format!("<xbrli:scenario>{scenario}</xbrli:scenario>") };
    format!(
        r#"<xbrli:context id="c"><xbrli:entity><xbrli:identifier scheme="s">x</xbrli:identifier>{segment}</xbrli:entity>{PERIOD}{scenario}</xbrli:context>"#
    )
}

fn instance(context: &str, concept: &str) -> String {
    format!(
        r#"<xbrli:xbrl xmlns:xbrli="http://www.xbrl.org/2003/instance" xmlns:link="http://www.xbrl.org/2003/linkbase" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:iso4217="http://www.xbrl.org/2003/iso4217" xmlns:xbrldi="http://xbrl.org/2006/xbrldi" xmlns:eg="urn:eg"><link:schemaRef xlink:type="simple" xlink:href="eg.xsd"/>
{context}
<xbrli:unit id="u"><xbrli:measure>iso4217:EUR</xbrli:measure></xbrli:unit>
<eg:{concept} contextRef="c" unitRef="u" decimals="0">1</eg:{concept}></xbrli:xbrl>"#
    )
}

fn region(member: &str) -> String {
    format!(r#"<xbrldi:explicitMember dimension="eg:RegionDim">eg:{member}</xbrldi:explicitMember>"#)
}

fn customer(id: &str) -> String {
    format!(r#"<xbrldi:typedMember dimension="eg:CustomerDim"><eg:CustomerId>{id}</eg:CustomerId></xbrldi:typedMember>"#)
}

fn findings_with(t: &Taxonomy, text: &str) -> Vec<Finding> {
    let inst = read(text).expect("reads");
    check_taxonomy(&inst, t)
}

fn findings(segment: &str, scenario: &str, concept: &str) -> (String, Vec<Finding>) {
    let text = instance(&context(segment, scenario), concept);
    let found = findings_with(&taxonomy(), &text);
    (text, found)
}

fn codes(segment: &str, scenario: &str, concept: &str) -> Vec<&'static str> {
    findings(segment, scenario, concept).1.into_iter().map(|f| f.code).collect()
}

/// Exactly one finding with `code`, at the element that starts with `at` (the fact or the context).
fn one(segment: &str, scenario: &str, concept: &str, code: &str, at: &str) -> String {
    let (text, found) = findings(segment, scenario, concept);
    let [f] = found.as_slice() else { panic!("one finding, not {found:#?}") };
    assert_eq!(f.code, code, "{}", f.message);
    assert_eq!(f.level, Level::Error);
    assert!(text.get(f.span.clone()).unwrap_or_default().starts_with(at), "{:?} is not at {at}", text.get(f.span.clone()));
    f.message.clone()
}

#[test]
fn the_model_has_the_hypercubes_dimensions_domains_defaults_and_the_primary_items_with_what_they_inherit() {
    let t = taxonomy();
    assert!(t.not_loaded.is_empty(), "{:?}", t.not_loaded);
    let d = &t.dimensions;
    assert!(d.problems.is_empty() && !d.truncated, "{:?}", d.problems);
    assert_eq!(d.hypercubes.iter().map(String::as_str).collect::<Vec<_>>(), ["{urn:eg}ExclCube", "{urn:eg}OpenCube", "{urn:eg}SalesCube"]);
    assert_eq!(d.dimensions.len(), 3);
    assert_eq!(d.dimensions["{urn:eg}CustomerDim"].as_deref(), Some("eg.xsd#eg_CustomerId"), "typed, with its domain");
    assert_eq!(d.dimensions["{urn:eg}RegionDim"], None, "explicit");
    assert_eq!(d.defaults.get("{urn:eg}RegionDim").map(String::as_str), Some("{urn:eg}RegionDomain"));
    let sales = &d.primary_items["{urn:eg}Sales"];
    assert_eq!(sales.len(), 1);
    assert!(sales[0].all && sales[0].closed && sales[0].context_element == ContextElement::Segment && sales[0].inherited_from.is_none());
    let cube = &d.cubes[sales[0].cube];
    assert_eq!(cube.hypercube, "{urn:eg}SalesCube");
    let region = cube.dimensions.iter().find(|x| x.dimension == "{urn:eg}RegionDim").expect("the region");
    assert_eq!(
        region.members.iter().map(String::as_str).collect::<Vec<_>>(),
        ["{urn:eg}North", "{urn:eg}RegionDomain", "{urn:eg}South"],
        "the domain and two members: East's arc is prohibited"
    );
    assert!(cube.dimensions.iter().any(|x| x.dimension == "{urn:eg}CustomerDim" && x.typed && x.members.is_empty()));
    let revenue = &d.primary_items["{urn:eg}Revenue"];
    assert_eq!(revenue.len(), 1);
    assert_eq!(revenue[0].inherited_from.as_deref(), Some("{urn:eg}Sales"), "Revenue inherits the hypercube of its parent Sales");
    assert_eq!(revenue[0].cube, sales[0].cube);
    let cost = &d.primary_items["{urn:eg}Cost"][0];
    assert!(!cost.closed && cost.context_element == ContextElement::Scenario);
    // Units: OpenCube's dimensions come from the role its targetRole names; ExclCube is notAll, with South alone.
    let units = &d.primary_items["{urn:eg}Units"];
    assert_eq!(units.len(), 2);
    assert!(units.iter().all(|u| u.role == UNITS));
    let excl = units.iter().find(|u| !u.all).expect("the notAll");
    let excl_cube = &d.cubes[excl.cube];
    assert_eq!(excl_cube.role, EXCL);
    assert_eq!(excl_cube.dimensions[0].members.iter().map(String::as_str).collect::<Vec<_>>(), ["{urn:eg}South"]);
    let open = units.iter().find(|u| u.all).expect("the all");
    assert_eq!(d.cubes[open.cube].dimensions[0].members.len(), 3, "OpenCube's domain in main, found through targetRole");
    assert!(!d.primary_items.contains_key("{urn:eg}Plain"), "a concept with no hypercube");
    assert_eq!(d.member_count(), 3);
}

#[test]
fn an_explicit_member_is_in_the_domain_and_a_prohibited_one_is_not() {
    assert_eq!(codes(&format!("{}{}", region("North"), customer("42")), "", "Sales"), Vec::<&str>::new());
    let message = one(&format!("{}{}", region("East"), customer("42")), "", "Sales", "xbrl.dimension-invalid", "<eg:Sales");
    assert!(message.contains("East is not a usable member of the dimension RegionDim"), "{message}");
}

#[test]
fn an_absent_explicit_dimension_takes_its_default_and_a_typed_one_must_be_there() {
    assert_eq!(codes(&customer("42"), "", "Sales"), Vec::<&str>::new(), "RegionDim takes its default");
    let message = one(&region("North"), "", "Sales", "xbrl.dimension-invalid", "<eg:Sales");
    assert!(message.contains("typed dimension CustomerDim"), "{message}");
}

#[test]
fn a_closed_hypercube_forbids_another_dimension_and_an_open_one_does_not() {
    let other = r#"<xbrldi:explicitMember dimension="eg:OtherDim">eg:OtherDomain</xbrldi:explicitMember>"#;
    let message = one(&format!("{}{}{other}", region("North"), customer("1")), "", "Sales", "xbrl.dimension-invalid", "<eg:Sales");
    assert!(message.contains("is closed") && message.contains("OtherDim"), "{message}");
    assert_eq!(codes("", &format!("{}{other}", region("North")), "Cost"), Vec::<&str>::new(), "open: another dimension is allowed");
}

#[test]
fn dimensions_count_only_in_the_container_the_relationship_names() {
    // The typed dimension in the scenario does not count for a segment hypercube.
    let message = one(&region("North"), &customer("42"), "Sales", "xbrl.dimension-invalid", "<eg:Sales");
    assert!(message.contains("in the segment"), "{message}");
    // Cost's hypercube is in the scenario: East there is wrong; in the segment it is not looked at (the default applies).
    one("", &region("East"), "Cost", "xbrl.dimension-invalid", "<eg:Cost");
    assert_eq!(codes(&region("East"), "", "Cost"), Vec::<&str>::new());
}

#[test]
fn a_child_in_the_primary_item_tree_inherits_its_parents_hypercube() {
    assert_eq!(codes(&format!("{}{}", region("South"), customer("7")), "", "Revenue"), Vec::<&str>::new());
    one(&format!("{}{}", region("East"), customer("7")), "", "Revenue", "xbrl.dimension-invalid", "<eg:Revenue");
    one(&region("North"), "", "Revenue", "xbrl.dimension-invalid", "<eg:Revenue");
    // A concept with no hypercube takes any dimensions.
    assert_eq!(codes(&region("East"), "", "Plain"), Vec::<&str>::new());
}

#[test]
fn a_not_all_hypercube_makes_the_context_that_matches_it_invalid() {
    assert_eq!(codes("", &region("North"), "Units"), Vec::<&str>::new());
    let message = one("", &region("South"), "Units", "xbrl.dimension-invalid", "<eg:Units");
    assert!(message.contains("ExclCube") && message.contains("notAll"), "{message}");
}

#[test]
fn the_dimension_values_of_a_context_are_checked_each_once_and_its_facts_are_not_judged_again() {
    let ok = format!("{}{}", region("North"), customer("1"));
    assert_eq!(codes(&ok, "", "Sales"), Vec::<&str>::new());
    one(&format!("{ok}{}", region("South")), "", "Sales", "xbrl.dimension-repeated", "<xbrli:context");
    one(
        &format!(r#"{}<xbrldi:explicitMember dimension="eg:CustomerDim">eg:North</xbrldi:explicitMember>"#, region("North")),
        "",
        "Sales",
        "xbrl.dimension-not-explicit",
        "<xbrli:context",
    );
    one(
        &format!(r#"{}<xbrldi:typedMember dimension="eg:RegionDim"><eg:CustomerId>1</eg:CustomerId></xbrldi:typedMember>"#, customer("1")),
        "",
        "Sales",
        "xbrl.dimension-not-typed",
        "<xbrli:context",
    );
    let message = one(
        &format!(r#"{}<xbrldi:explicitMember dimension="eg:Revenue">eg:North</xbrldi:explicitMember>"#, ok),
        "",
        "Sales",
        "xbrl.dimension-not-explicit",
        "<xbrli:context",
    );
    assert!(message.contains("not a dimension the taxonomy declares"), "{message}");
    let message = one(&format!("{}{}", region("Nowhere"), customer("1")), "", "Sales", "xbrl.dimension-member-undefined", "<xbrli:context");
    assert!(message.contains("Nowhere"), "{message}");
    one(&format!("{}{}", region("RegionDomain"), customer("1")), "", "Sales", "xbrl.dimension-default-used", "<xbrli:context");
    one(
        &format!(
            r#"{}<xbrldi:typedMember dimension="eg:CustomerDim"><eg:CustomerId>1</eg:CustomerId><eg:CustomerId>2</eg:CustomerId></xbrldi:typedMember>"#,
            region("North")
        ),
        "",
        "Sales",
        "xbrl.dimension-typed-content",
        "<xbrli:context",
    );
    // A dimension in a namespace no schema declares is not judged.
    assert_eq!(
        codes(&format!(r#"{ok}<xbrldi:explicitMember xmlns:o="urn:other" dimension="o:Dim">o:M</xbrldi:explicitMember>"#), "", "Cost"),
        Vec::<&str>::new()
    );
}

#[test]
fn what_breaks_the_taxonomy_rules_is_said_once_and_left_out_of_the_model() {
    let mut links = good_links();
    links.push(link(MAIN, &[("all", "Plain", "OpenCube", "")]));
    links.push(link(MAIN, &[("hypercube-dimension", "OpenCube", "North", "")]));
    links.push(link(MAIN, &[("dimension-default", "RegionDim", "North", "")]));
    let t = load(
        &["eg.xsd".to_owned()],
        "",
        &mut files_with(r#"<xs:element id="eg_Loud" name="Loud" substitutionGroup="xbrldt:hypercubeItem" xbrli:periodType="duration"/>"#, &links),
    );
    let codes: Vec<&str> = t.dimensions.problems.iter().map(|p| p.code).collect();
    assert_eq!(
        codes,
        [
            "HypercubeElementIsNotAbstractError",
            "HasHypercubeMissingContextElementAttributeError",
            "HypercubeDimensionTargetError",
            "TooManyDefaultMembersError"
        ]
    );
    assert!(!t.dimensions.primary_items.contains_key("{urn:eg}Plain"), "the arc with no contextElement is left out");
    let found = findings_with(&t, &instance(&context("", &region("North")), "Cost"));
    assert_eq!(found.iter().filter(|f| f.code == "xbrl.dimension-taxonomy").count(), 4);
    assert!(found.iter().all(|f| f.code == "xbrl.dimension-taxonomy" && f.span.is_empty()), "{found:#?}");
}

#[test]
fn cycles_in_the_relationships_end() {
    let links = vec![link(
        MAIN,
        &[
            ("all", "Sales", "OpenCube", r#"xbrldt:contextElement="segment""#),
            ("hypercube-dimension", "OpenCube", "RegionDim", ""),
            ("dimension-domain", "RegionDim", "North", ""),
            ("domain-member", "North", "South", ""),
            ("domain-member", "South", "North", ""),
            ("domain-member", "Sales", "Revenue", ""),
            ("domain-member", "Revenue", "Sales", ""),
        ],
    )];
    let t = load(&["eg.xsd".to_owned()], "", &mut files_with("", &links));
    assert_eq!(t.dimensions.cubes[0].dimensions[0].members.len(), 2);
    assert_eq!(t.dimensions.primary_items.len(), 2, "Sales and Revenue, once each");
    let found = findings_with(&t, &instance(&context(&region("East"), ""), "Revenue"));
    assert_eq!(found.iter().map(|f| f.code).collect::<Vec<_>>(), ["xbrl.dimension-invalid"]);
}
