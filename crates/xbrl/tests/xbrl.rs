//! XBRL instances and Inline XBRL: read, and checked without a taxonomy.

use xmlcraft_xbrl::{Kind, Level, Period, check, read};

const HEAD: &str = "<xbrli:xbrl xmlns:xbrli=\"http://www.xbrl.org/2003/instance\" xmlns:link=\"http://www.xbrl.org/2003/linkbase\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" xmlns:iso4217=\"http://www.xbrl.org/2003/iso4217\" xmlns:xbrldi=\"http://xbrl.org/2006/xbrldi\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xmlns:gaap=\"http://example.com/gaap\">";

fn instance(body: &str) -> String {
    format!("{HEAD}<link:schemaRef xlink:type=\"simple\" xlink:href=\"gaap.xsd\"/>{body}</xbrli:xbrl>")
}

const CONTEXT: &str = "<xbrli:context id=\"c1\"><xbrli:entity><xbrli:identifier scheme=\"http://example.com/id\">ACME</xbrli:identifier></xbrli:entity><xbrli:period><xbrli:instant>2025-12-31</xbrli:instant></xbrli:period></xbrli:context>";
const FLOW: &str = "<xbrli:context id=\"c2\"><xbrli:entity><xbrli:identifier scheme=\"http://example.com/id\">ACME</xbrli:identifier><xbrli:segment><xbrldi:explicitMember dimension=\"gaap:Region\">gaap:Europe</xbrldi:explicitMember></xbrli:segment></xbrli:entity><xbrli:period><xbrli:startDate>2025-01-01</xbrli:startDate><xbrli:endDate>2025-12-31</xbrli:endDate></xbrli:period></xbrli:context>";
const EUR: &str = "<xbrli:unit id=\"eur\"><xbrli:measure>iso4217:EUR</xbrli:measure></xbrli:unit>";

fn codes(text: &str) -> Vec<(&'static str, Level)> {
    check(&read(text).expect("reads")).into_iter().map(|f| (f.code, f.level)).collect()
}

#[test]
fn contexts_units_and_facts_are_read_with_their_places_and_dimensions() {
    let text = instance(&format!(
        "{CONTEXT}{FLOW}{EUR}<xbrli:unit id=\"rate\"><xbrli:divide><xbrli:unitNumerator><xbrli:measure>iso4217:EUR</xbrli:measure></xbrli:unitNumerator><xbrli:unitDenominator><xbrli:measure>xbrli:shares</xbrli:measure></xbrli:unitDenominator></xbrli:divide></xbrli:unit><gaap:Assets contextRef=\"c1\" unitRef=\"eur\" decimals=\"0\" id=\"f1\">1000</gaap:Assets><gaap:Name contextRef=\"c1\">ACME Ltd</gaap:Name><gaap:Revenue contextRef=\"c2\" unitRef=\"eur\" decimals=\"-3\">250</gaap:Revenue><gaap:Price contextRef=\"c2\" unitRef=\"rate\" decimals=\"2\">0.5</gaap:Price>"
    ));
    let i = read(&text).expect("reads");
    assert_eq!(
        (i.kind, i.schema_refs.as_slice(), i.contexts.len(), i.units.len(), i.facts.len()),
        (Kind::Instance, ["gaap.xsd".to_owned()].as_slice(), 2, 2, 4)
    );
    assert_eq!(i.contexts[0].period, Some(Period::Instant("2025-12-31".to_owned())));
    assert_eq!(i.contexts[1].period.as_ref().map(Period::label).as_deref(), Some("2025-01-01 – 2025-12-31"));
    assert_eq!(
        (i.contexts[1].dimensions[0].dimension.as_str(), i.contexts[1].dimensions[0].member.as_str()),
        ("{http://example.com/gaap}Region", "{http://example.com/gaap}Europe")
    );
    assert_eq!((i.units[0].measure.as_str(), i.units[1].measure.contains('/')), ("EUR", true), "{:?}", i.units);
    let assets = &i.facts[0];
    assert_eq!(
        (assets.concept.as_str(), assets.name.as_str(), assets.value.as_str(), assets.decimals.as_deref(), assets.id.as_deref()),
        ("{http://example.com/gaap}Assets", "gaap:Assets", "1000", Some("0"), Some("f1"))
    );
    assert_eq!(&text[assets.span.clone()], "<gaap:Assets contextRef=\"c1\" unitRef=\"eur\" decimals=\"0\" id=\"f1\">1000</gaap:Assets>");
    assert!(check(&i).is_empty(), "{:?}", check(&i));
}

#[test]
fn what_the_instance_says_wrong_about_itself_is_found_at_the_fact_or_context() {
    let text = instance(&format!(
        "{CONTEXT}<xbrli:context id=\"c1\"><xbrli:entity><xbrli:identifier scheme=\"s\">A</xbrli:identifier></xbrli:entity><xbrli:period><xbrli:instant>2025-13-40</xbrli:instant></xbrli:period></xbrli:context>{EUR}\
         <gaap:A contextRef=\"nope\" unitRef=\"eur\" decimals=\"0\">1</gaap:A>\
         <gaap:B contextRef=\"c1\" unitRef=\"gone\" decimals=\"0\">1</gaap:B>\
         <gaap:C unitRef=\"eur\" decimals=\"0\">1</gaap:C>\
         <gaap:D contextRef=\"c1\" unitRef=\"eur\" decimals=\"0\" precision=\"3\">1</gaap:D>\
         <gaap:E contextRef=\"c1\" unitRef=\"eur\" decimals=\"two\">1</gaap:E>\
         <gaap:F contextRef=\"c1\" unitRef=\"eur\" decimals=\"0\">one</gaap:F>\
         <gaap:G contextRef=\"c1\" xsi:nil=\"true\">oops</gaap:G>\
         <gaap:H contextRef=\"c1\" decimals=\"0\">12</gaap:H>"
    ));
    let got: Vec<&str> = codes(&text).into_iter().map(|(c, _)| c).collect();
    for wanted in [
        "xbrl.duplicate-context",
        "xbrl.bad-period",
        "xbrl.unknown-context",
        "xbrl.unknown-unit",
        "xbrl.no-context",
        "xbrl.decimals-and-precision",
        "xbrl.bad-decimals",
        "xbrl.bad-number",
        "xbrl.nil-with-value",
        "xbrl.number-without-unit",
    ] {
        assert!(got.contains(&wanted), "{wanted} in {got:?}");
    }
    // Each is at its own element.
    let findings = check(&read(&text).expect("reads"));
    let at = |code: &str| findings.iter().find(|f| f.code == code).map(|f| f.span.clone()).expect("a finding");
    assert!(text[at("xbrl.unknown-context")].starts_with("<gaap:A "));
    assert!(text[at("xbrl.no-context")].starts_with("<gaap:C "));
    assert!(text[at("xbrl.nil-with-value")].starts_with("<gaap:G "));
}

#[test]
fn periods_duplicates_and_what_is_never_used() {
    let text = instance(&format!(
        "{CONTEXT}<xbrli:context id=\"back\"><xbrli:entity><xbrli:identifier scheme=\"s\">A</xbrli:identifier></xbrli:entity><xbrli:period><xbrli:startDate>2025-12-31</xbrli:startDate><xbrli:endDate>2025-01-01</xbrli:endDate></xbrli:period></xbrli:context>\
         <xbrli:context id=\"noid\"><xbrli:entity><xbrli:identifier scheme=\"\"></xbrli:identifier></xbrli:entity><xbrli:period><xbrli:forever/></xbrli:period></xbrli:context>{EUR}<xbrli:unit id=\"idle\"><xbrli:measure>iso4217:USD</xbrli:measure></xbrli:unit>\
         <gaap:X contextRef=\"c1\" unitRef=\"eur\" decimals=\"0\">5</gaap:X><gaap:X contextRef=\"c1\" unitRef=\"eur\" decimals=\"0\">5</gaap:X><gaap:Y contextRef=\"c1\" unitRef=\"eur\" decimals=\"0\">1</gaap:Y><gaap:Y contextRef=\"c1\" unitRef=\"eur\" decimals=\"0\">2</gaap:Y>"
    ));
    let got = codes(&text);
    for wanted in [
        ("xbrl.period-order", Level::Error),
        ("xbrl.bad-entity", Level::Error),
        ("xbrl.duplicate-fact", Level::Warning),
        ("xbrl.inconsistent-duplicate", Level::Error),
        ("xbrl.unused-context", Level::Warning),
        ("xbrl.unused-unit", Level::Warning),
    ] {
        assert!(got.contains(&wanted), "{wanted:?} in {got:?}");
    }
    assert!(codes(&format!("{HEAD}{CONTEXT}</xbrli:xbrl>")).contains(&("xbrl.no-schema-ref", Level::Warning)));
    // Leap days.
    let leap = instance(&CONTEXT.replace("2025-12-31", "2024-02-29"));
    assert!(!codes(&leap).iter().any(|(c, _)| *c == "xbrl.bad-period"));
    assert!(codes(&instance(&CONTEXT.replace("2025-12-31", "2025-02-29"))).iter().any(|(c, _)| *c == "xbrl.bad-period"));
}

const INLINE: &str = "<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:ix=\"http://www.xbrl.org/2013/inlineXBRL\" xmlns:xbrli=\"http://www.xbrl.org/2003/instance\" xmlns:link=\"http://www.xbrl.org/2003/linkbase\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" xmlns:iso4217=\"http://www.xbrl.org/2003/iso4217\" xmlns:gaap=\"http://example.com/gaap\"><body>\
    <div style=\"display:none\"><ix:header><ix:references><link:schemaRef xlink:type=\"simple\" xlink:href=\"gaap.xsd\"/></ix:references><ix:resources>\
    <xbrli:context id=\"c1\"><xbrli:entity><xbrli:identifier scheme=\"s\">ACME</xbrli:identifier></xbrli:entity><xbrli:period><xbrli:instant>2025-12-31</xbrli:instant></xbrli:period></xbrli:context>\
    <xbrli:unit id=\"eur\"><xbrli:measure>iso4217:EUR</xbrli:measure></xbrli:unit></ix:resources></ix:header></div>\
    <p>Assets: <ix:nonFraction name=\"gaap:Assets\" contextRef=\"c1\" unitRef=\"eur\" decimals=\"-3\" scale=\"3\">1,234.5</ix:nonFraction> and \
    <ix:nonFraction name=\"gaap:Loss\" contextRef=\"c1\" unitRef=\"eur\" decimals=\"0\" sign=\"-\">80</ix:nonFraction> and \
    <ix:nonNumeric name=\"gaap:Name\" contextRef=\"c1\">ACME Ltd</ix:nonNumeric> and \
    <ix:nonFraction name=\"gaap:Odd\" contextRef=\"c1\" unitRef=\"eur\" decimals=\"0\">twelve</ix:nonFraction></p></body></html>";

#[test]
fn inline_xbrl_facts_are_read_with_scale_and_sign_applied_and_checked_like_an_instance() {
    let i = read(INLINE).expect("reads");
    assert_eq!(
        (i.kind, i.schema_refs.as_slice(), i.contexts.len(), i.units.len(), i.facts.len()),
        (Kind::Inline, ["gaap.xsd".to_owned()].as_slice(), 1, 1, 4)
    );
    assert_eq!((i.facts[0].concept.as_str(), i.facts[0].value.as_str()), ("{http://example.com/gaap}Assets", "1234500"), "1,234.5 scaled by 3");
    assert_eq!(i.facts[1].value, "-80");
    assert_eq!((i.facts[2].name.as_str(), i.facts[2].value.as_str()), ("gaap:Name", "ACME Ltd"));
    let findings = check(&i);
    assert_eq!(findings.iter().map(|f| f.code).collect::<Vec<_>>(), ["xbrl.bad-number"], "{findings:?}");
    assert!(INLINE[findings[0].span.clone()].contains("twelve"));
}

#[test]
fn other_documents_are_refused_and_no_text_panics() {
    for (text, why) in [("<a/>", "not an XBRL document"), ("<xbrli:xbrl", "not well-formed"), ("", "not well-formed")] {
        let e = read(text).expect_err(text);
        assert!(e.0.contains(why), "{text:?}: {e}");
    }
    assert!(xmlcraft_xbrl::is_xbrl(&instance("")) && xmlcraft_xbrl::is_xbrl(INLINE) && !xmlcraft_xbrl::is_xbrl("<a/>"));
}

proptest::proptest! {
    #[test]
    fn any_text_is_read_and_checked_or_refused_without_a_panic(text in ".{0,300}") {
        if let Ok(i) = read(&text) {
            let _ = check(&i);
        }
    }

    #[test]
    fn any_prefix_of_a_real_instance_is_read_or_refused(cut in 0usize..1500) {
        let doc = instance(&format!("{CONTEXT}{EUR}<gaap:A contextRef=\"c1\" unitRef=\"eur\" decimals=\"0\">1</gaap:A>"));
        let at = (0..=cut.min(doc.len())).rev().find(|i| doc.is_char_boundary(*i)).unwrap_or(0);
        if let Ok(i) = read(&doc[..at]) {
            let _ = check(&i);
        }
    }
}
