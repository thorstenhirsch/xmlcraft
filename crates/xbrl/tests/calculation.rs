//! The calculation check: summation-item relationships of a calculation linkbase against the facts of an instance (XBRL 2.1, 5.2.5.2).

use std::collections::HashMap;

use xmlcraft_xbrl::taxonomy::{Loader, check_taxonomy, load};
use xmlcraft_xbrl::{Finding, read};

struct Files(HashMap<&'static str, String>);

impl Loader for Files {
    fn load(&mut self, location: &str, _base: &str) -> Result<(String, String), String> {
        let key = location.rsplit('/').next().unwrap_or(location);
        self.0.get(key).map(|t| (format!("mem:{key}"), t.clone())).ok_or_else(|| format!("{location}: no such document"))
    }
}

const LINK_HEAD: &str = r#"<link:linkbase xmlns:link="http://www.xbrl.org/2003/linkbase" xmlns:xlink="http://www.w3.org/1999/xlink">"#;

/// A calculation link in the default role: `Total` is the sum of the children (name, weight, extra attributes of the arc).
fn calc_link(children: &[(&str, &str, &str)]) -> String {
    let mut s = String::from(
        r#"<link:calculationLink xlink:type="extended" xlink:role="http://www.xbrl.org/2003/role/link"><link:loc xlink:type="locator" xlink:href="eg.xsd#eg_Total" xlink:label="total"/>"#,
    );
    for (i, (name, weight, extra)) in children.iter().enumerate() {
        s.push_str(&format!(
            r#"<link:loc xlink:type="locator" xlink:href="eg.xsd#eg_{name}" xlink:label="c{i}"/><link:calculationArc xlink:type="arc" xlink:arcrole="http://www.xbrl.org/2003/arcrole/summation-item" xlink:from="total" xlink:to="c{i}" weight="{weight}" order="{}" {extra}/>"#,
            i + 1
        ));
    }
    s.push_str("</link:calculationLink>");
    s
}

fn files(links: &str, embed: bool) -> Files {
    let linkbase_ref = if embed {
        format!("<xs:annotation><xs:appinfo>{LINK_HEAD}{links}</link:linkbase></xs:appinfo></xs:annotation>")
    } else {
        r#"<xs:annotation><xs:appinfo><link:linkbaseRef xlink:type="simple" xlink:href="calc.xml" xlink:role="http://www.xbrl.org/2003/role/calculationLinkbaseRef" xlink:arcrole="http://www.w3.org/1999/xlink/properties/linkbase"/></xs:appinfo></xs:annotation>"#.to_owned()
    };
    let eg = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xbrli="http://www.xbrl.org/2003/instance" xmlns:link="http://www.xbrl.org/2003/linkbase" xmlns:xlink="http://www.w3.org/1999/xlink" targetNamespace="urn:eg">
  {linkbase_ref}
  <xs:element id="eg_Total" name="Total" type="xbrli:monetaryItemType" substitutionGroup="xbrli:item" xbrli:periodType="instant"/>
  <xs:element id="eg_A" name="A" type="xbrli:monetaryItemType" substitutionGroup="xbrli:item" xbrli:periodType="instant"/>
  <xs:element id="eg_B" name="B" type="xbrli:monetaryItemType" substitutionGroup="xbrli:item" xbrli:periodType="instant"/>
</xs:schema>"#
    );
    let calc = format!("{LINK_HEAD}{links}</link:linkbase>");
    Files(HashMap::from([("eg.xsd", eg), ("calc.xml", calc)]))
}

const CONTEXT: &str = r#"<xbrli:entity><xbrli:identifier scheme="s">x</xbrli:identifier></xbrli:entity><xbrli:period><xbrli:instant>2025-12-31</xbrli:instant></xbrli:period>"#;

fn instance(facts: &str) -> String {
    format!(
        r#"<xbrli:xbrl xmlns:xbrli="http://www.xbrl.org/2003/instance" xmlns:link="http://www.xbrl.org/2003/linkbase" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:iso4217="http://www.xbrl.org/2003/iso4217" xmlns:eg="urn:eg" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><link:schemaRef xlink:type="simple" xlink:href="eg.xsd"/>
<xbrli:context id="c1">{CONTEXT}</xbrli:context><xbrli:context id="c2">{CONTEXT}</xbrli:context>
<xbrli:unit id="u"><xbrli:measure>iso4217:EUR</xbrli:measure></xbrli:unit><xbrli:unit id="v"><xbrli:measure>iso4217:EUR</xbrli:measure></xbrli:unit>{facts}</xbrli:xbrl>"#
    )
}

fn run(mut taxonomy_files: Files, facts: &str) -> Vec<Finding> {
    let text = instance(facts);
    let inst = read(&text).expect("reads");
    let taxonomy = load(&inst.schema_refs, "", &mut taxonomy_files);
    assert!(taxonomy.not_loaded.is_empty(), "{:?}", taxonomy.not_loaded);
    check_taxonomy(&inst, &taxonomy).into_iter().filter(|f| f.code == "xbrl.calculation-inconsistent").collect()
}

fn fact(name: &str, value: &str, extra: &str) -> String {
    format!(r#"<eg:{name} contextRef="c1" unitRef="u" decimals="0" {extra}>{value}</eg:{name}>"#)
}

#[test]
fn a_sum_that_adds_up_is_silent_and_one_that_does_not_is_found_at_its_fact() {
    let links = calc_link(&[("A", "1", ""), ("B", "1", "")]);
    let ok = format!("{}{}{}", fact("A", "40", ""), fact("B", "2", ""), fact("Total", "42", ""));
    assert_eq!(run(files(&links, false), &ok), []);
    let bad = format!("{}{}{}", fact("A", "40", ""), fact("B", "2", ""), fact("Total", "43", ""));
    let found = run(files(&links, false), &bad);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].message.contains("eg:Total") && found[0].message.contains("is 43") && found[0].message.contains("add up to 42"),
        "{}",
        found[0].message
    );
    assert!(instance(&bad)[found[0].span.clone()].starts_with("<eg:Total"), "the finding is at the summation item");
    // The same calculation embedded in the schema's appinfo.
    assert_eq!(run(files(&links, true), &bad).len(), 1);
}

#[test]
fn weights_rounding_and_decimals_follow_the_specification() {
    let net = calc_link(&[("A", "1", ""), ("B", "-1", "")]);
    let with = |a: &str, b: &str, t: &str| format!("{}{}{}", fact("A", a, ""), fact("B", b, ""), fact("Total", t, ""));
    assert_eq!(run(files(&net, false), &with("50", "8", "42")), [], "a weight of -1 subtracts");
    assert_eq!(run(files(&net, false), &with("50", "8", "58")).len(), 1);
    // Each item is rounded to its own decimals before the sum; the sum is rounded to the decimals of the total.
    let links = calc_link(&[("A", "1", ""), ("B", "1", "")]);
    let rounded = r#"<eg:A contextRef="c1" unitRef="u" decimals="-2">1250</eg:A><eg:B contextRef="c1" unitRef="u" decimals="-2">1350</eg:B><eg:Total contextRef="c1" unitRef="u" decimals="-2">2600</eg:Total>"#;
    assert_eq!(run(files(&links, false), rounded), [], "1250 rounds to 1200 (ties to even), 1350 to 1400: 2600");
    let off_by_a_hundred = rounded.replace(">2600<", ">2700<");
    assert_eq!(run(files(&links, false), &off_by_a_hundred).len(), 1);
    // The total is rounded to its own decimals: 0.4 + 0.4 = 0.8 is 1 at 0 decimals.
    let fine = r#"<eg:A contextRef="c1" unitRef="u" decimals="1">0.4</eg:A><eg:B contextRef="c1" unitRef="u" decimals="1">0.4</eg:B><eg:Total contextRef="c1" unitRef="u" decimals="0">1</eg:Total>"#;
    assert_eq!(run(files(&links, false), fine), []);
    // A precision is turned into decimals: 1234 with precision 2 is 1200.
    let precision = r#"<eg:A contextRef="c1" unitRef="u" precision="2">1234</eg:A><eg:B contextRef="c1" unitRef="u" precision="2">1266</eg:B><eg:Total contextRef="c1" unitRef="u" precision="2">2500</eg:Total>"#;
    assert_eq!(run(files(&links, false), precision), [], "1200 + 1300 = 2500");
    let zero = precision.replace("precision=\"2\">2500", "precision=\"0\">2500");
    assert_eq!(run(files(&links, false), &zero).len(), 1, "a precision of 0 makes it inconsistent");
}

#[test]
fn only_facts_equal_in_context_and_unit_by_value_contribute_and_some_calculations_do_not_bind() {
    let links = calc_link(&[("A", "1", ""), ("B", "1", "")]);
    // A different context id with the same value is the same context; another unit id with the same measure is the same unit.
    let by_value = r#"<eg:A contextRef="c1" unitRef="u" decimals="0">1</eg:A><eg:B contextRef="c2" unitRef="v" decimals="0">2</eg:B><eg:Total contextRef="c1" unitRef="u" decimals="0">3</eg:Total>"#;
    assert_eq!(run(files(&links, false), by_value), [], "B contributes: 1 + 2 is 3");
    assert_eq!(run(files(&links, false), &by_value.replace(">3<", ">4<")).len(), 1, "1 + 2 is not 4");
    // Facts of another period are not contributing items.
    let other_period = format!("{}{}", fact("A", "1", ""), fact("Total", "9", ""));
    assert_eq!(run(files(&links, false), &other_period).len(), 1, "only A: 1 is not 9");
    // No contributing item: it does not bind. A nil total or a nil item does not count.
    assert_eq!(run(files(&links, false), &fact("Total", "9", "")), []);
    let nil_total = format!("{}{}", fact("A", "1", ""), r#"<eg:Total contextRef="c1" unitRef="u" decimals="0" xsi:nil="true"/>"#);
    assert_eq!(run(files(&links, false), &nil_total), []);
    let nil_item =
        format!("{}{}{}", fact("A", "1", ""), r#"<eg:B contextRef="c1" unitRef="u" decimals="0" xsi:nil="true"/>"#, fact("Total", "1", ""));
    assert_eq!(run(files(&links, false), &nil_item), [], "B is left out: 1 is 1");
    // A contributing concept twice (a duplicate): it does not bind.
    let duplicate = format!("{}{}{}", fact("A", "1", ""), fact("A", "5", ""), fact("Total", "9", ""));
    assert_eq!(run(files(&links, false), &duplicate), []);
    // An item whose value is not a number leaves the calculation unchecked.
    let text = format!("{}{}", fact("A", "many", ""), fact("Total", "9", ""));
    assert_eq!(run(files(&links, false), &text), []);
}

#[test]
fn a_prohibited_relationship_is_not_calculated_and_unreadable_linkbases_are_said() {
    let both = calc_link(&[("A", "1", ""), ("B", "1", "")]);
    // A later arc with a higher priority that prohibits A leaves only B.
    let prohibited = both.replace("</link:calculationLink>", r#"<link:calculationArc xlink:type="arc" xlink:arcrole="http://www.xbrl.org/2003/arcrole/summation-item" xlink:from="total" xlink:to="c0" weight="1" order="1" priority="1" use="prohibited"/></link:calculationLink>"#);
    let facts = format!("{}{}{}", fact("A", "40", ""), fact("B", "2", ""), fact("Total", "2", ""));
    assert_eq!(run(files(&prohibited, false), &facts), [], "Total is B alone");
    assert_eq!(run(files(&both, false), &facts).len(), 1);
    // A linkbase that cannot be read is said by the taxonomy, not guessed.
    let mut missing = files(&both, false);
    missing.0.remove("calc.xml");
    let text = instance(&facts);
    let inst = read(&text).expect("reads");
    let taxonomy = load(&inst.schema_refs, "", &mut missing);
    assert_eq!(taxonomy.not_loaded.len(), 1);
    assert!(taxonomy.calculations.is_empty());
}

#[test]
fn nothing_the_linkbase_says_makes_it_panic() {
    for broken in [
        "<link:calculationLink/>",
        r#"<link:calculationLink><link:calculationArc xlink:from="x" xlink:to="y"/></link:calculationLink>"#,
        r##"<link:calculationLink><link:loc xlink:href="#" xlink:label="x"/><link:loc xlink:href="eg.xsd#nope" xlink:label="y"/><link:calculationArc xlink:from="x" xlink:to="y" weight="abc" order="z" priority="q"/></link:calculationLink>"##,
        r##"<link:calculationLink><link:loc xlink:href="eg.xsd#eg_Total" xlink:label="x"/><link:calculationArc xlink:from="x" xlink:to="x" weight="1e9999"/></link:calculationLink>"##,
    ] {
        let facts = format!("{}{}", fact("A", "1", ""), fact("Total", "9", ""));
        let _ = run(files(broken, false), &facts);
    }
}
