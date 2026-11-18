//! XBRL Dimensions 1.0 with a small synthetic taxonomy: the dimensional model read from its definition linkbase.
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
use xmlcraft_xbrl::taxonomy::{Loader, Taxonomy, load};

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
}
