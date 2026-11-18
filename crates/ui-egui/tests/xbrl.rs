//! The XBRL tool window in the real shell: the rail and the command bar for an instance, the facts table (filter, click to select), and
//! the findings of a broken instance, with a synthetic dimensional taxonomy beside it on disk. Asynchronous work is waited for by
//! polling, never by counting frames.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::{Panel, XmlCraftApp};

const SCHEMA: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xbrli="http://www.xbrl.org/2003/instance" xmlns:xbrldt="http://xbrl.org/2005/xbrldt" xmlns:link="http://www.xbrl.org/2003/linkbase" xmlns:xlink="http://www.w3.org/1999/xlink" targetNamespace="urn:eg">
<xs:annotation><xs:appinfo><link:linkbaseRef xlink:type="simple" xlink:href="eg-def.xml" xlink:role="http://www.xbrl.org/2003/role/definitionLinkbaseRef" xlink:arcrole="http://www.w3.org/1999/xlink/properties/linkbase"/></xs:appinfo></xs:annotation>
<xs:element id="eg_Sales" name="Sales" type="xbrli:monetaryItemType" substitutionGroup="xbrli:item" xbrli:periodType="instant"/>
<xs:element id="eg_Cost" name="Cost" type="xbrli:monetaryItemType" substitutionGroup="xbrli:item" xbrli:periodType="instant"/>
<xs:element id="eg_Cube" name="Cube" type="xbrli:stringItemType" substitutionGroup="xbrldt:hypercubeItem" abstract="true" xbrli:periodType="instant"/>
<xs:element id="eg_Region" name="Region" type="xbrli:stringItemType" substitutionGroup="xbrldt:dimensionItem" abstract="true" xbrli:periodType="instant"/>
<xs:element id="eg_AllRegions" name="AllRegions" type="xbrli:stringItemType" substitutionGroup="xbrli:item" abstract="true" xbrli:periodType="instant"/>
<xs:element id="eg_North" name="North" type="xbrli:stringItemType" substitutionGroup="xbrli:item" abstract="true" xbrli:periodType="instant"/>
<xs:element id="eg_South" name="South" type="xbrli:stringItemType" substitutionGroup="xbrli:item" abstract="true" xbrli:periodType="instant"/>
<xs:element id="eg_East" name="East" type="xbrli:stringItemType" substitutionGroup="xbrli:item" abstract="true" xbrli:periodType="instant"/>
</xs:schema>"#;

fn definitions() -> String {
    let locs: String = ["Sales", "Cube", "Region", "AllRegions", "North", "South"]
        .iter()
        .map(|c| format!(r#"<link:loc xlink:type="locator" xlink:href="eg.xsd#eg_{c}" xlink:label="{c}"/>"#))
        .collect();
    let arc = |role: &str, from: &str, to: &str, extra: &str| {
        format!(
            r#"<link:definitionArc xlink:type="arc" xlink:arcrole="http://xbrl.org/int/dim/arcrole/{role}" xlink:from="{from}" xlink:to="{to}" {extra}/>"#
        )
    };
    format!(
        r#"<link:linkbase xmlns:link="http://www.xbrl.org/2003/linkbase" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:xbrldt="http://xbrl.org/2005/xbrldt"><link:definitionLink xlink:type="extended" xlink:role="http://www.xbrl.org/2003/role/link">{locs}{}{}{}{}{}{}</link:definitionLink></link:linkbase>"#,
        arc("all", "Sales", "Cube", r#"xbrldt:contextElement="segment" xbrldt:closed="true""#),
        arc("hypercube-dimension", "Cube", "Region", ""),
        arc("dimension-domain", "Region", "AllRegions", ""),
        arc("domain-member", "AllRegions", "North", ""),
        arc("domain-member", "AllRegions", "South", ""),
        arc("dimension-default", "Region", "AllRegions", ""),
    )
}

fn context(id: &str, member: Option<&str>) -> String {
    let segment = member
        .map(|m| format!(r#"<xbrli:segment><xbrldi:explicitMember dimension="eg:Region">eg:{m}</xbrldi:explicitMember></xbrli:segment>"#))
        .unwrap_or_default();
    format!(
        r#"<xbrli:context id="{id}"><xbrli:entity><xbrli:identifier scheme="http://example.com/id">ACME</xbrli:identifier>{segment}</xbrli:entity><xbrli:period><xbrli:instant>2025-12-31</xbrli:instant></xbrli:period></xbrli:context>"#
    )
}

/// The instance: Sales by region (North, South and the total, which is the default) and Cost; `east` puts one Sales fact in a region
/// that is not in the domain.
fn instance(east: bool) -> String {
    let (last_context, last_fact) = if east {
        (context("c_east", Some("East")), r#"<eg:Sales contextRef="c_east" unitRef="EUR" decimals="0">5</eg:Sales>"#)
    } else {
        (String::new(), "")
    };
    format!(
        r#"<xbrli:xbrl xmlns:xbrli="http://www.xbrl.org/2003/instance" xmlns:link="http://www.xbrl.org/2003/linkbase" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:xbrldi="http://xbrl.org/2006/xbrldi" xmlns:iso4217="http://www.xbrl.org/2003/iso4217" xmlns:eg="urn:eg">
<link:schemaRef xlink:type="simple" xlink:href="eg.xsd"/>
{}
{}
{}
{last_context}
<xbrli:unit id="EUR"><xbrli:measure>iso4217:EUR</xbrli:measure></xbrli:unit>
<eg:Sales contextRef="c_north" unitRef="EUR" decimals="0">100</eg:Sales>
<eg:Sales contextRef="c_south" unitRef="EUR" decimals="0">80</eg:Sales>
<eg:Sales contextRef="c_total" unitRef="EUR" decimals="0">180</eg:Sales>
<eg:Cost contextRef="c_total" unitRef="EUR" decimals="0">50</eg:Cost>
{last_fact}
</xbrli:xbrl>"#,
        context("c_north", Some("North")),
        context("c_south", Some("South")),
        context("c_total", None),
    )
}

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn files(dir: &Path, east: bool) -> PathBuf {
    std::fs::write(dir.join("eg.xsd"), SCHEMA).expect("writes");
    std::fs::write(dir.join("eg-def.xml"), definitions()).expect("writes");
    let path = dir.join(if east { "broken.xbrl" } else { "report.xbrl" });
    std::fs::write(&path, instance(east)).expect("writes");
    path
}

/// Step the shell until `done` holds (or fail after `secs` seconds).
fn wait_for(h: &mut Harness<'static, XmlCraftApp>, secs: u64, what: &str, mut done: impl FnMut(&mut Harness<'static, XmlCraftApp>) -> bool) {
    let started = Instant::now();
    while !done(h) {
        assert!(started.elapsed() < Duration::from_secs(secs), "waiting for {what}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn open(h: &mut Harness<'static, XmlCraftApp>, path: &Path) {
    assert!(h.state_mut().open_path(path));
    wait_for(h, 20, "the first analysis", |h| h.state().problems().is_some());
    h.run_steps(2);
}

/// Show the tool and wait until its table lists the facts.
fn show_facts(h: &mut Harness<'static, XmlCraftApp>) {
    assert!(h.state_mut().execute("xml.xbrl"));
    wait_for(h, 30, "the facts", |h| h.query_all_by_label_contains("eg:Cost = 50").next().is_some());
    h.run_steps(2);
}

fn rows(h: &Harness<'static, XmlCraftApp>, start: &str) -> usize {
    h.query_all_by_label_contains(start).count()
}

/// The selected text of the document being shown.
fn selected(h: &Harness<'static, XmlCraftApp>) -> String {
    let doc = h.state().session.active_document().expect("shown");
    let text = doc.editor().text();
    let s = doc.editor().selections()[0];
    let (start, end) = (s.start().min(s.end()), s.start().max(s.end()));
    text.get(start..end).unwrap_or_default().to_owned()
}

#[test]
fn an_instance_lights_the_rail_icon_and_offers_its_facts_and_other_xml_does_not() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    let plain = dir.path().join("plain.xml");
    std::fs::write(&plain, "<library><book/></library>").expect("writes");
    open(&mut h, &plain);
    assert!(!h.state_mut().panel_relevant(Panel::Xbrl), "plain XML: the rail icon is dimmed");
    open(&mut h, &files(dir.path(), false));
    assert!(h.state_mut().panel_relevant(Panel::Xbrl), "an instance: the rail icon is lit");
    assert!(!h.state().panel_visible(Panel::Xbrl), "hidden by default");
    // The command bar's action for an instance (the tool's own tab of the same name is not there yet).
    h.get_by_label("Facts").click();
    h.run_steps(3);
    assert!(h.state().panel_visible(Panel::Xbrl) && h.query_by_label("Close XBRL").is_some(), "the tool window is up");
    assert!(h.query_by_label("→ report.xbrl").is_some(), "it says which document it works on");
    wait_for(&mut h, 30, "the facts", |h| h.query_all_by_label_contains("eg:Cost = 50").next().is_some());
    assert!(
        h.query_all_by_label_contains("dimensions: 1 hypercube, 1 dimension, 3 members, 1 primary item").next().is_some(),
        "the summary has the dimensional model"
    );
}

#[test]
fn the_table_lists_the_facts_and_the_filter_narrows_them() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &files(dir.path(), false));
    show_facts(&mut h);
    assert_eq!(rows(&h, "eg:Sales = "), 3, "three Sales facts");
    assert_eq!(rows(&h, "eg:Cost = "), 1);
    assert!(
        h.query_all_by_label_contains("eg:Sales = 100 · c_north · 2025-12-31 · Region=North").next().is_some(),
        "with its context and dimensions"
    );
    h.get_by_label("Filter the facts").focus();
    h.run_steps(1);
    h.get_by_label("Filter the facts").type_text("north");
    h.run_steps(3);
    assert_eq!(rows(&h, "eg:Sales = "), 1, "only the fact whose dimensions say North");
    assert_eq!(rows(&h, "eg:Cost = "), 0);
    assert!(h.query_by_label_contains("1 of 4 facts").is_some());
}

#[test]
fn clicking_a_row_selects_the_fact_in_the_text_and_an_edit_makes_the_table_stale() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &files(dir.path(), false));
    show_facts(&mut h);
    // (A pointer is over a row for a frame before it clicks it, as a hand does.)
    h.get_by_label_contains("eg:Cost = 50").hover();
    h.run_steps(2);
    h.get_by_label_contains("eg:Cost = 50").click();
    h.run_steps(3);
    assert_eq!(selected(&h), r#"<eg:Cost contextRef="c_total" unitRef="EUR" decimals="0">50</eg:Cost>"#);
    if let Some(doc) = h.state_mut().session.active_document_mut() {
        doc.editor_mut().place_caret(0, false);
        doc.editor_mut().type_text(" ");
    }
    h.run_steps(2);
    assert!(h.query_by_label_contains("The text changed since").is_some(), "the table says it is out of date");
    wait_for(&mut h, 30, "the new reading", |h| h.query_by_label_contains("The text changed since").is_none());
}

#[test]
fn a_broken_instance_shows_its_finding_in_the_findings_tab_and_a_click_selects_the_fact() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &files(dir.path(), true));
    assert!(h.state_mut().execute("xml.xbrl"));
    wait_for(&mut h, 30, "the facts", |h| h.query_all_by_label_contains("eg:Sales = 5").next().is_some());
    h.run_steps(2);
    let tab = h.query_all_by_label("Findings").next().expect("the Findings tab");
    tab.click();
    h.run_steps(3);
    let row = "xbrl.dimension-invalid: The fact eg:Sales is not dimensionally valid in the context c_east";
    wait_for(&mut h, 10, "the finding", |h| h.query_all_by_label_contains(row).next().is_some());
    assert_eq!(h.query_all_by_label_contains("xbrl.dimension-invalid: ").count(), 1, "one finding");
    assert!(h.query_all_by_label("xbrl.dimension-invalid 1").next().is_some(), "counted by code");
    h.get_by_label_contains(row).hover();
    h.run_steps(2);
    h.get_by_label_contains(row).click();
    h.run_steps(3);
    assert_eq!(selected(&h), r#"<eg:Sales contextRef="c_east" unitRef="EUR" decimals="0">5</eg:Sales>"#);
}
