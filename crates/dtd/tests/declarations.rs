//! Reading declarations: what a valid DTD gives, and the code of every kind of mistake.

use std::collections::HashMap;

use xmlcraft_dtd::{
    AttType, ContentSpec, Default as AttDefault, DoctypeInput, EntityValue, ExternalRef, LoadRequest, Loaded, NoLoader, Occurs, Parsed, Particle,
    ParticleKind, SubsetLoader, parse_doctype, parse_external_subset, parse_internal_subset,
};
use xmlcraft_syntax::Code;

fn internal(text: &str) -> Parsed {
    parse_internal_subset(text, 0)
}

fn codes(parsed: &Parsed) -> Vec<Code> {
    parsed.diagnostics.iter().map(|d| d.diagnostic.code).collect()
}

/// Valid: no problem at all.
fn clean(text: &str) -> Parsed {
    let parsed = internal(text);
    assert!(parsed.diagnostics.is_empty(), "{text:?}: {:?}", parsed.diagnostics);
    parsed
}

/// A mistake of this kind (and, where it is told, no other).
fn mistake(text: &str, code: Code) {
    let parsed = internal(text);
    assert!(codes(&parsed).contains(&code), "{text:?}: wanted {code:?}, got {:?}", parsed.diagnostics);
}

/// A mistake of one of these kinds: the declaration's own code, or the more exact one a literal or a reference has.
fn mistake_in(text: &str, family: &[Code]) {
    let parsed = internal(text);
    assert!(codes(&parsed).iter().any(|c| family.contains(c)), "{text:?}: wanted one of {family:?}, got {:?}", parsed.diagnostics);
}

const ATTLIST: &[Code] = &[Code::DtdBadAttlist, Code::BareAmpersand, Code::UnterminatedLiteral, Code::InvalidChar];
const ENTITY: &[Code] =
    &[Code::DtdBadEntityDecl, Code::BadEntityValue, Code::InvalidCharRef, Code::UnterminatedLiteral, Code::BadPubidLiteral, Code::InvalidChar];

fn name(n: &str) -> Particle {
    Particle { kind: ParticleKind::Name(n.to_owned()), occurs: Occurs::Once }
}

#[test]
fn element_declarations_give_their_content_specifications() {
    let parsed =
        clean("<!ELEMENT a EMPTY><!ELEMENT b ANY><!ELEMENT c (#PCDATA)><!ELEMENT d (#PCDATA | x | y)*><!ELEMENT e (f , g? , (h | i)+ , j*)>");
    let spec = |n: &str| parsed.dtd.element(n).map(|e| e.spec.clone());
    assert_eq!(spec("a"), Some(ContentSpec::Empty));
    assert_eq!(spec("b"), Some(ContentSpec::Any));
    assert_eq!(spec("c"), Some(ContentSpec::Mixed(vec![])));
    assert_eq!(spec("d"), Some(ContentSpec::Mixed(vec!["x".into(), "y".into()])));
    let Some(ContentSpec::Children(top)) = spec("e") else { panic!("children") };
    let ParticleKind::Sequence(items) = top.kind else { panic!("a sequence") };
    assert_eq!(items.len(), 4);
    assert_eq!(items[0], name("f"));
    assert_eq!(items[1], Particle { kind: ParticleKind::Name("g".into()), occurs: Occurs::Optional });
    assert_eq!(items[2].occurs, Occurs::OneOrMore);
    assert!(matches!(&items[2].kind, ParticleKind::Choice(c) if c.len() == 2));
    assert_eq!(items[3].occurs, Occurs::ZeroOrMore);
    // White space may be anywhere a token ends; a group of one is a sequence; `(#PCDATA)*` is allowed.
    clean("<!ELEMENT a ( b ) ><!ELEMENT c (#PCDATA)*><!ELEMENT d ( #PCDATA | e )* >\n<!ELEMENT\tf\n(g|h)>");
}

#[test]
fn mistakes_in_element_declarations() {
    for text in [
        "<!ELEMENT>",
        "<!ELEMENT a>",
        "<!ELEMENT (b)>",
        "<!ELEMENTa EMPTY>",
        "<!ELEMENT a(b)>",
        "<!ELEMENT a EMPTY",
        "<!ELEMENT a empty>",
        "<!ELEMENT a b>",
        "<!ELEMENT a (b|c,d)>",
        "<!ELEMENT a (b,c|d)>",
        "<!ELEMENT a (b c)>",
        "<!ELEMENT a (b|)>",
        "<!ELEMENT a (|b)>",
        "<!ELEMENT a ()>",
        "<!ELEMENT a (b)(c)>",
        "<!ELEMENT a (b) ?>",
        "<!ELEMENT a (b)??>",
        "<!ELEMENT a (+b)>",
        "<!ELEMENT a (#PCDATA|b)>",
        "<!ELEMENT a (#PCDATA,b)*>",
        "<!ELEMENT a (b|#PCDATA)*>",
        "<!ELEMENT a (#pcdata)>",
        "<!ELEMENT a (#PCDATA|)*>",
        "<!ELEMENT a EMPTY EMPTY>",
        "<!ELEMENT a (b,c)*+>",
    ] {
        mistake(text, Code::DtdBadContentModel);
    }
}

#[test]
fn attribute_list_declarations_give_types_and_defaults() {
    let parsed = clean(
        "<!ATTLIST e a CDATA #IMPLIED b ID #REQUIRED c IDREF #IMPLIED d IDREFS #IMPLIED f ENTITY #IMPLIED g ENTITIES #IMPLIED h NMTOKEN \"x\" i NMTOKENS #IMPLIED \
         j (one|two | three) 'two' k NOTATION (n1|n2) #IMPLIED l CDATA #FIXED \"v &amp; &#65; &w;\">",
    );
    let def = |n: &str| parsed.dtd.attribute("e", n).cloned().expect(n);
    assert_eq!(def("a").ty, AttType::Cdata);
    assert_eq!(def("a").default, AttDefault::Implied);
    assert_eq!(def("b").ty, AttType::Id);
    assert_eq!(def("b").default, AttDefault::Required);
    assert_eq!((def("c").ty, def("d").ty, def("f").ty, def("g").ty), (AttType::Idref, AttType::Idrefs, AttType::Entity, AttType::Entities));
    assert_eq!((def("h").ty, def("i").ty), (AttType::Nmtoken, AttType::Nmtokens));
    assert_eq!(def("h").default, AttDefault::Value("x".into()));
    assert_eq!(def("j").ty, AttType::Enumeration(vec!["one".into(), "two".into(), "three".into()]));
    assert_eq!(def("k").ty, AttType::Notation(vec!["n1".into(), "n2".into()]));
    assert_eq!(def("l").default, AttDefault::Fixed("v &amp; &#65; &w;".into()));
    // No attribute at all is fine; several declarations for one element are all kept, and the first of a name counts.
    let more = clean("<!ATTLIST e>\n<!ATTLIST e a CDATA \"first\">\n<!ATTLIST e a CDATA \"second\" b CDATA #IMPLIED>");
    assert_eq!(more.dtd.attlists.len(), 3);
    assert_eq!(more.dtd.attribute("e", "a").map(|d| d.default.clone()), Some(AttDefault::Value("first".into())));
    assert!(more.dtd.attribute("e", "b").is_some());
}

#[test]
fn mistakes_in_attribute_list_declarations() {
    for text in [
        "<!ATTLIST>",
        "<!ATTLISTe a CDATA #IMPLIED>",
        "<!ATTLIST e a BOGUS #IMPLIED>",
        "<!ATTLIST e a cdata #IMPLIED>",
        "<!ATTLIST e a #CDATA #IMPLIED>",
        "<!ATTLIST e a PCDATA #IMPLIED>",
        "<!ATTLIST e a CDATA>",
        "<!ATTLIST e a CDATA #REQUIREDX>",
        "<!ATTLIST e a CDATA implied>",
        "<!ATTLIST e a CDATA #FIXED>",
        "<!ATTLIST e a CDATA #FIXED x>",
        "<!ATTLIST e a CDATA x>",
        "<!ATTLIST e a CDATA \"a<b\">",
        "<!ATTLIST e a CDATA \"&unclosed\">",
        "<!ATTLIST e aCDATA #IMPLIED>",
        "<!ATTLIST e a CDATA#IMPLIED>",
        "<!ATTLIST e a (x|y)#IMPLIED>",
        "<!ATTLIST e a (x|y #IMPLIED>",
        "<!ATTLIST e a (x y) #IMPLIED>",
        "<!ATTLIST e a () #IMPLIED>",
        "<!ATTLIST e a (x|) #IMPLIED>",
        "<!ATTLIST e a NOTATION #IMPLIED>",
        "<!ATTLIST e a NOTATION (1x) #IMPLIED>",
        "<!ATTLIST e a NOTATION(x) #IMPLIED>",
        "<!ATTLIST e a NOTATIONx (x) #IMPLIED>",
        "<!ATTLIST e a IDS #IMPLIED>",
        "<!ATTLIST 1e a CDATA #IMPLIED>",
        "<!ATTLIST e CDATA #IMPLIED>",
        "<!ATTLIST e a CDATA #IMPLIED",
    ] {
        mistake_in(text, ATTLIST);
    }
}

#[test]
fn entity_declarations_give_values_with_references_resolved() {
    let parsed = clean(
        "<!ENTITY g 'plain &#65;&#x42; &other; &amp; \"quoted\"'><!ENTITY % p \"parameter\"><!ENTITY x SYSTEM 'x.xml'><!ENTITY y PUBLIC '-//Y//EN' 'y.xml'>\
         <!ENTITY pic SYSTEM 'p.gif' NDATA gif><!ENTITY % ext SYSTEM 'e.ent'><!ENTITY empty ''>",
    );
    let value = |n: &str| parsed.dtd.general_entity(n).map(|e| e.value.clone());
    // Character references are resolved, general-entity references stay as they are written.
    assert_eq!(value("g"), Some(EntityValue::Internal("plain AB &other; &amp; \"quoted\"".into())));
    assert_eq!(parsed.dtd.parameter_entity("p").map(|e| e.value.clone()), Some(EntityValue::Internal("parameter".into())));
    assert!(matches!(value("x"), Some(EntityValue::External(id)) if id.system.as_deref() == Some("x.xml") && id.public.is_none()));
    assert!(
        matches!(value("y"), Some(EntityValue::External(id)) if id.public.as_deref() == Some("-//Y//EN") && id.system.as_deref() == Some("y.xml"))
    );
    assert_eq!(parsed.dtd.general_entity("pic").and_then(|e| e.notation.clone()).as_deref(), Some("gif"));
    assert_eq!(value("empty"), Some(EntityValue::Internal(String::new())));
    // The first declaration of a name is the one that counts.
    let twice = clean("<!ENTITY a 'first'><!ENTITY a 'second'><!ENTITY % q 'one'><!ENTITY % q 'two'>");
    assert_eq!(twice.dtd.general_entity("a").map(|e| e.value.clone()), Some(EntityValue::Internal("first".into())));
    assert_eq!(twice.dtd.parameter_entity("q").map(|e| e.value.clone()), Some(EntityValue::Internal("one".into())));
    assert_eq!(twice.dtd.general_entities.len(), 2);
}

#[test]
fn mistakes_in_entity_declarations() {
    for text in [
        "<!ENTITY>",
        "<!ENTITYa 'v'>",
        "<!ENTITY a>",
        "<!ENTITY a'v'>",
        "<!ENTITY 'v'>",
        "<!ENTITY 'v' a>",
        "<!ENTITY a 'v'",
        "<!ENTITY a '&unclosed'>",
        "<!ENTITY a '&#xZZ;'>",
        "<!ENTITY a '&#0;'>",
        "<!ENTITY a '&#xD800;'>",
        "<!ENTITY a 'bare % sign'>",
        "<!ENTITY %a 'v'>",
        "<!ENTITY % a>",
        "<!ENTITY % 'v'>",
        "<!ENTITY a system 'x'>",
        "<!ENTITY a SYSTEM>",
        "<!ENTITY a SYSTEM'x'>",
        "<!ENTITY a SYSTEM x>",
        "<!ENTITY a PUBLIC 'p'>",
        "<!ENTITY a PUBLIC 'p''s'>",
        "<!ENTITY a PUBLIC \"bad ~ char\" 's'>",
        "<!ENTITY a SYSTEM 'x' NDATA>",
        "<!ENTITY a SYSTEM 'x'NDATA n>",
        "<!ENTITY a SYSTEM 'x' NDATAn>",
        "<!ENTITY % a SYSTEM 'x' NDATA n>",
        "<!ENTITY a 'v' NDATA n>",
        "<!ENTITY a SYSTEM 'x' extra>",
    ] {
        mistake_in(text, ENTITY);
    }
}

#[test]
fn some_mistakes_in_entity_declarations_have_a_more_exact_code() {
    mistake("<!ENTITY a 'a & b'>", Code::BadEntityValue);
    mistake("<!ENTITY a '&#0;'>", Code::InvalidCharRef);
    mistake("<!ATTLIST e a CDATA \"a & b\">", Code::BareAmpersand);
    mistake("<!ENTITY a 'v", Code::UnterminatedLiteral);
    mistake("<!ENTITY a 'unclosed>", Code::UnterminatedLiteral);
    mistake("<!ATTLIST e a CDATA \"unclosed>", Code::UnterminatedLiteral);
    mistake("<!ENTITY a PUBLIC \"bad ~ char\" 's'>", Code::BadPubidLiteral);
    mistake("<!ENTITY a SYSTEM 'x\u{1}'>", Code::InvalidChar);
}

#[test]
fn notation_declarations() {
    let parsed = clean("<!NOTATION n1 SYSTEM 'viewer'><!NOTATION n2 PUBLIC '-//N//EN'><!NOTATION n3 PUBLIC '-//N//EN' 'n3.exe'>");
    assert_eq!(parsed.dtd.notations.len(), 3);
    assert_eq!(parsed.dtd.notations[1].id.system, None);
    assert_eq!(parsed.dtd.notations[2].id.system.as_deref(), Some("n3.exe"));
    for text in [
        "<!NOTATION>",
        "<!NOTATIONn SYSTEM 'x'>",
        "<!NOTATION n>",
        "<!NOTATION SYSTEM 'x'>",
        "<!NOTATION n SYSTEM>",
        "<!NOTATION n system 'x'>",
        "<!NOTATION n public '-//N//EN'>",
        "<!NOTATION n PUBLIC>",
        "<!NOTATION n PUBLIC 'p' extra>",
        "<!NOTATION n SYSTEM 'x'",
        "<!NOTATION SYSTEM 'x' n>",
    ] {
        mistake(text, Code::DtdBadNotation);
    }
}

#[test]
fn comments_and_processing_instructions_in_the_subset() {
    clean("<!-- a comment --><?target data?><?t?><!-- another -->");
    mistake("<!-- double -- hyphen -->", Code::DoubleHyphenInComment);
    mistake("<!-- ends with --->", Code::DoubleHyphenInComment);
    mistake("<!-- unclosed", Code::UnterminatedComment);
    mistake("<?target unclosed", Code::UnterminatedPi);
    mistake("<? no target?>", Code::MissingPiTarget);
    mistake("<?xml version='1.0'?>", Code::ReservedPiTarget);
    mistake("<!-- bad \u{1} char -->", Code::InvalidChar);
    // A target is a name, then white space or the end.
    mistake("<?_) not a target char?>", Code::MissingSpace);
    mistake("<?a:b x?>", Code::ColonInName);
    clean("<?a?><?a b?><?a\nb ?>");
    // Names of entities and notations have no colon (namespaces); element and attribute names may.
    mistake("<!ENTITY a:b 'v'>", Code::ColonInName);
    mistake("<!ENTITY % a:b 'v'>", Code::ColonInName);
    mistake("<!NOTATION a:b SYSTEM 'x'>", Code::ColonInName);
    clean("<!ELEMENT p:a EMPTY><!ATTLIST p:a x:y CDATA #IMPLIED>");
}

#[test]
fn what_is_not_a_declaration_is_reported_and_reading_goes_on() {
    mistake("<!FOO bar>", Code::UnknownDeclaration);
    mistake("<foo>", Code::UnknownDeclaration);
    mistake("<!>", Code::UnknownDeclaration);
    mistake("text", Code::UnexpectedCharacter);
    assert_eq!(
        codes(&internal("some words <!ELEMENT a EMPTY> here")),
        [Code::UnexpectedCharacter; 3],
        "one problem for each of the three words, not for each character"
    );
    mistake("]]>", Code::UnexpectedCharacter);
    // After a mistake the next declaration is read, and its mistakes are reported too.
    let parsed = internal("<!ELEMENT a> <!ELEMENT b EMPTY> <!ATTLIST c d BOGUS #IMPLIED> <!ENTITY e 'v'>");
    assert_eq!(codes(&parsed), [Code::DtdBadContentModel, Code::DtdBadAttlist]);
    assert!(parsed.dtd.element("b").is_some() && parsed.dtd.general_entity("e").is_some());
}

#[test]
fn spans_say_where_in_the_document_the_problem_is() {
    let subset = "<!ELEMENT a EMPTY>\n<!ELEMENT b>\n";
    let parsed = parse_internal_subset(subset, 100);
    let d = &parsed.diagnostics[0];
    assert_eq!(d.source, 0);
    // The problem is in the second declaration, offset by where the subset begins in the document.
    let start = 100 + subset.find("<!ELEMENT b").expect("there");
    assert!(d.diagnostic.span.start >= start && d.diagnostic.span.end <= 100 + subset.len(), "{:?} from {start}", d.diagnostic.span);
    let place = &clean("<!ELEMENT a EMPTY>  <!ENTITY e 'v'>").dtd.elements[0].place;
    assert_eq!((place.source, place.range.clone()), (0, 0..18));
}

#[test]
fn parameter_entities_between_declarations_bring_their_declarations() {
    let parsed = clean("<!ENTITY % decls \"<!ELEMENT a EMPTY> <!ENTITY e 'v'>\"> %decls; <!ELEMENT b ANY>");
    assert!(parsed.dtd.element("a").is_some() && parsed.dtd.element("b").is_some());
    assert!(parsed.dtd.general_entity("e").is_some());
    // The reference may not be inside a declaration of the internal subset, and the entity must be declared (nothing external here).
    mistake("<!ENTITY % e 'EMPTY'><!ELEMENT a %e;>", Code::PeInDeclaration);
    mistake("<!ENTITY % e 'x'><!ENTITY a '%e;'>", Code::PeInDeclaration);
    mistake("%nothere;", Code::DtdUndeclaredPe);
    mistake("%unterminated <!ELEMENT a ANY>", Code::DtdBadPeReference);
    mistake("%e", Code::DtdBadPeReference);
    mistake("<!ENTITY % e '%e;'> %e;", Code::PeInDeclaration);
}

#[test]
fn a_literal_does_not_run_on_from_a_replacement_text_into_the_text_that_refers_to_it() {
    // The entity's text ends inside a literal: the closing quote that follows the reference cannot belong to it.
    let parsed = internal("<!ENTITY % e \"<!ENTITY a 'x\"> %e; '> <!ELEMENT b EMPTY>");
    assert!(codes(&parsed).contains(&Code::UnterminatedLiteral), "{:?}", parsed.diagnostics);
    assert!(parsed.dtd.element("b").is_some(), "reading goes on after it");
    assert!(parsed.dtd.general_entity("a").is_none());
    // The same for a system literal and a public identifier.
    for text in
        ["<!ENTITY % e \"<!ENTITY a SYSTEM 'x\"> %e; '> <!ELEMENT b EMPTY>", "<!ENTITY % e \"<!NOTATION n PUBLIC 'x\"> %e; '> <!ELEMENT b EMPTY>"]
    {
        let parsed = internal(text);
        assert!(codes(&parsed).contains(&Code::UnterminatedLiteral), "{text}: {:?}", parsed.diagnostics);
        assert!(parsed.dtd.element("b").is_some());
    }
}

#[test]
fn a_conditional_section_is_only_for_the_external_subset() {
    mistake("<![INCLUDE[ <!ELEMENT a EMPTY> ]]>", Code::DtdConditionalInInternal);
    let external = |text: &str| parse_external_subset(text);
    let ok = external("<![INCLUDE[ <!ELEMENT a EMPTY> ]]> <![ IGNORE [ <!ELEMENT b EMPTY> <![INCLUDE[ nested ]]> junk ]]> <!ELEMENT c ANY>");
    assert!(ok.diagnostics.is_empty(), "{:?}", ok.diagnostics);
    assert!(ok.dtd.element("a").is_some() && ok.dtd.element("b").is_none() && ok.dtd.element("c").is_some());
    for text in [
        "<![NOTINCLUDE[ ]]>",
        "<![include[ ]]>",
        "<![INCLUDE[[ ]]>",
        "<![INCLUDE? ]]>",
        "<![ INCLUDE <!ELEMENT a ANY> ]]>",
        "<![INCLUDE[ <!ELEMENT a ANY> ",
        "<![IGNORE[ never closed",
        "<![IGNORE[ <![ nested ]]>",
        "<![]]>",
        "<![%undeclared;[ ]]>",
        "]]>",
    ] {
        let parsed = external(text);
        assert!(!parsed.diagnostics.is_empty(), "{text:?} should be refused");
    }
}

/// External texts by their system identifier.
#[derive(Default)]
struct Files {
    files: HashMap<String, String>,
    asked: Vec<(String, String)>,
}

impl Files {
    fn with(mut self, system: &str, text: &str) -> Self {
        self.files.insert(system.to_owned(), text.to_owned());
        self
    }
}

impl SubsetLoader for Files {
    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String> {
        self.asked.push((request.kind.to_owned(), request.system.to_owned()));
        self.files
            .get(request.system)
            .map(|text| Loaded { text: text.clone(), location: format!("file:///dtd/{}", request.system) })
            .ok_or_else(|| "not found".to_owned())
    }
}

fn doctype(external: Option<&str>, internal: Option<&str>, files: &mut Files) -> Parsed {
    parse_doctype(
        &DoctypeInput {
            external: external.map(|system| ExternalRef { public: None, system: Some(system) }),
            internal: internal.map(|text| (text, 0)),
            location: "file:///doc.xml",
        },
        files,
    )
}

#[test]
fn the_external_subset_is_read_after_the_internal_one_and_the_internal_declarations_win() {
    let mut files = Files::default().with("main.dtd", "<!ELEMENT root (a, b)> <!ENTITY e 'external'> <!ENTITY only 'there'> <!ELEMENT a EMPTY>");
    let parsed = doctype(Some("main.dtd"), Some("<!ENTITY e 'internal'>"), &mut files);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    assert_eq!(parsed.dtd.general_entity("e").map(|e| e.value.clone()), Some(EntityValue::Internal("internal".into())));
    assert!(parsed.dtd.general_entity("only").is_some() && parsed.dtd.element("root").is_some());
    assert!(parsed.dtd.has_external_parts && !parsed.dtd.incomplete);
    assert_eq!(files.asked, [("external subset".to_owned(), "main.dtd".to_owned())]);
    assert_eq!(parsed.sources, ["", "file:///dtd/main.dtd"]);
    // Where a problem is in the external text, the source says so.
    let mut files = Files::default().with("bad.dtd", "<!ELEMENT ok EMPTY>\n<!ELEMENT broken>");
    let parsed = doctype(Some("bad.dtd"), None, &mut files);
    let d = &parsed.diagnostics[0];
    assert_eq!(d.source, 1);
    assert!(d.diagnostic.span.start >= 20, "{:?}", d.diagnostic.span);
}

#[test]
fn an_external_subset_that_cannot_be_read_is_noted_and_the_dtd_is_incomplete() {
    let parsed = doctype(Some("missing.dtd"), Some("<!ENTITY e 'v'>"), &mut Files::default());
    assert_eq!(codes(&parsed), [Code::DtdExternalNotRead]);
    assert!(parsed.dtd.incomplete && parsed.dtd.has_external_parts && parsed.dtd.general_entity("e").is_some());
    // Without a loader: the same.
    let parsed = parse_doctype(
        &DoctypeInput { external: Some(ExternalRef { public: Some("-//P//EN"), system: Some("x.dtd") }), internal: None, location: "" },
        &mut NoLoader,
    );
    assert_eq!(codes(&parsed), [Code::DtdExternalNotRead]);
    // A public identifier without a system one cannot be loaded here.
    let parsed = parse_doctype(
        &DoctypeInput { external: Some(ExternalRef { public: Some("-//P//EN"), system: None }), internal: None, location: "" },
        &mut NoLoader,
    );
    assert!(parsed.dtd.incomplete && parsed.dtd.has_external_parts);
}

#[test]
fn parameter_entities_stand_for_their_text_inside_declarations_of_the_external_subset() {
    let subset = "<!ENTITY % names 'a | b'>\n<!ENTITY % model '(%names; | c)*'>\n<!ELEMENT x %model;>\n<!ENTITY % decl '<!ELEMENT y EMPTY>'>\n%decl;\n<!ELEMENT z%names;>";
    let parsed = parse_external_subset(subset);
    // A reference inside an entity value is resolved when the entity is declared, a reference in a declaration is read in place.
    assert_eq!(parsed.dtd.parameter_entity("model").map(|e| e.value.clone()), Some(EntityValue::Internal("(a | b | c)*".into())));
    assert_eq!(
        parsed.dtd.element("x").map(|e| e.spec.clone()),
        Some(ContentSpec::Children(Particle { kind: ParticleKind::Choice(vec![name("a"), name("b"), name("c")]), occurs: Occurs::ZeroOrMore }))
    );
    assert!(parsed.dtd.element("y").is_some());
    // `<!ELEMENT z%names;>`: the text starts with a space, so the name `z` is followed by white space, then `a | b` is no content
    // specification.
    assert_eq!(codes(&parsed), [Code::DtdBadContentModel]);
    // A reference stands for white space, so it may be where a token ends, and a token cannot be built from two texts.
    let ok = parse_external_subset("<!ENTITY % e 'EMPTY'><!ELEMENT a %e;><!ELEMENT b%e;>");
    assert!(ok.diagnostics.is_empty() && ok.dtd.element("b").is_some(), "{:?}", ok.diagnostics);
    let glued = parse_external_subset("<!ENTITY % e 'MPTY'><!ELEMENT a E%e;>");
    assert!(!glued.diagnostics.is_empty(), "EMPTY cannot be made of `E` and a reference");
    // The text of a reference has a space at each end: it does not run on into the text that follows it, nor from what comes before.
    let after = parse_external_subset("<!ENTITY % e 'abc'><!ELEMENT %e;x EMPTY>");
    assert!(!after.diagnostics.is_empty() && after.dtd.element("abcx").is_none(), "{:?}", after.dtd.elements);
    let before = parse_external_subset("<!ENTITY % e 'abc'><!ELEMENT x%e; EMPTY>");
    assert!(before.dtd.element("x").is_none() || !before.diagnostics.is_empty(), "{:?}", before.dtd.elements);
    assert!(before.dtd.element("xabc").is_none());
}

#[test]
fn external_parameter_entities_are_loaded_once_with_their_text_declaration_removed() {
    let mut files = Files::default()
        .with("main.dtd", "<!ENTITY % shared SYSTEM 'shared.ent'> %shared; %shared; <!ELEMENT root (a)>")
        .with("shared.ent", "<?xml version='1.0' encoding='UTF-8'?><!ELEMENT a EMPTY><!ENTITY inner 'v'>");
    let parsed = doctype(Some("main.dtd"), None, &mut files);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    assert!(parsed.dtd.element("a").is_some() && parsed.dtd.general_entity("inner").is_some());
    assert_eq!(
        files.asked,
        [("external subset".to_owned(), "main.dtd".to_owned()), ("parameter entity".to_owned(), "shared.ent".to_owned())],
        "read once"
    );
    assert_eq!(parsed.sources.len(), 3);
    // The declaration inside carries where it was read from.
    assert_eq!(parsed.dtd.general_entity("inner").and_then(|e| e.location.clone()).as_deref(), Some("file:///dtd/shared.ent"));
    // A text declaration that is wrong, or that is not first, is reported in the file it is in.
    for (text, wanted) in [
        ("<?xml version='1.0'?><!ELEMENT a EMPTY>", Some(Code::DtdBadTextDecl)),
        ("<?xml encoding='UTF-8' version='1.0'?><!ELEMENT a EMPTY>", Some(Code::DtdBadTextDecl)),
        ("<?xml version='x' encoding='UTF-8'?>", Some(Code::DtdBadTextDecl)),
        ("<?xml encoding='UTF-8' standalone='yes'?>", Some(Code::DtdBadTextDecl)),
        ("<!ELEMENT a EMPTY><?xml encoding='UTF-8'?>", Some(Code::ReservedPiTarget)),
        (" <?xml encoding='UTF-8'?>", Some(Code::ReservedPiTarget)),
        ("<?xml encoding='UTF-8'?>", None),
        ("<?xml version='1.0' encoding='UTF-8'?>", None),
    ] {
        let mut files = Files::default().with("main.dtd", "<!ENTITY % p SYSTEM 'p.ent'> %p;").with("p.ent", text);
        let parsed = doctype(Some("main.dtd"), None, &mut files);
        match wanted {
            Some(code) => assert!(codes(&parsed).contains(&code), "{text:?}: {:?}", parsed.diagnostics),
            None => assert!(parsed.diagnostics.is_empty(), "{text:?}: {:?}", parsed.diagnostics),
        }
    }
    // In the text of an external entity a reference may be inside a declaration (it cannot in the internal subset).
    let mut files = Files::default()
        .with("main.dtd", "<!ENTITY % model 'EMPTY'> <!ENTITY % p SYSTEM 'p.ent'> %p;")
        .with("p.ent", "<!ELEMENT a %model;> <!ENTITY e '%model;'>");
    let parsed = doctype(Some("main.dtd"), None, &mut files);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    assert_eq!(parsed.dtd.element("a").map(|e| e.spec.clone()), Some(ContentSpec::Empty));
    // An external parameter entity that cannot be read is noted, and the DTD is incomplete.
    let mut files = Files::default().with("main.dtd", "<!ENTITY % p SYSTEM 'gone.ent'> %p; <!ELEMENT a EMPTY>");
    let parsed = doctype(Some("main.dtd"), None, &mut files);
    assert_eq!(codes(&parsed), [Code::DtdExternalNotRead]);
    assert!(parsed.dtd.incomplete && parsed.dtd.element("a").is_some());
}

#[test]
fn a_parameter_entity_that_contains_itself_and_one_that_is_not_declared_are_reported() {
    // A reference in an entity value is resolved where the entity is declared, so a loop needs an external entity.
    let mut files = Files::default().with("main.dtd", "<!ENTITY % a SYSTEM 'a.ent'> %a; <!ELEMENT x EMPTY>").with("a.ent", "<!ENTITY % b '%a;'> %a;");
    let parsed = doctype(Some("main.dtd"), None, &mut files);
    assert!(codes(&parsed).contains(&Code::DtdRecursiveEntity), "{:?}", parsed.diagnostics);
    assert!(parsed.dtd.element("x").is_some(), "reading goes on");
    // In a DTD with an external part, an entity that is not declared may be declared in what was not read: a note, not an error.
    let note = parse_external_subset("%nowhere; <!ELEMENT x EMPTY>");
    assert_eq!(codes(&note), [Code::DtdUndeclaredPeUnread]);
    assert!(note.dtd.incomplete);
    // With no external part there is nowhere it could be declared.
    mistake("<!ELEMENT x EMPTY> %nowhere;", Code::DtdUndeclaredPe);
    let with_external = doctype(Some("e.dtd"), Some("%nowhere;"), &mut Files::default().with("e.dtd", ""));
    assert_eq!(codes(&with_external), [Code::DtdUndeclaredPeUnread]);
}

#[test]
fn limits_stop_a_hostile_dtd() {
    // Nested parameter entities that double at every level (a "billion laughs" of parameter entities).
    let mut text = String::from("<!ENTITY % e0 'ab'>");
    for i in 1..40 {
        text.push_str(&format!("<!ENTITY % e{i} '%e{}; %e{};'>", i - 1, i - 1));
    }
    text.push_str("%e39;");
    let started = std::time::Instant::now();
    let parsed = parse_external_subset(&text);
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "bounded work");
    assert!(codes(&parsed).contains(&Code::DtdLimit), "{:?}", &parsed.diagnostics[..parsed.diagnostics.len().min(3)]);
    // Entities that include each other in the markup (so that each is read inside the one before), deeper than the limit.
    let mut files = Files::default().with("main.dtd", "<!ENTITY % e0 SYSTEM 'e0.ent'> %e0;");
    for i in 0..100 {
        let inner = if i == 99 { "<!ELEMENT a EMPTY>".to_owned() } else { format!("<!ENTITY % e{} SYSTEM 'e{}.ent'> %e{};", i + 1, i + 1, i + 1) };
        files = files.with(&format!("e{i}.ent"), &inner);
    }
    let parsed = doctype(Some("main.dtd"), None, &mut files);
    assert!(codes(&parsed).contains(&Code::DtdLimit) && parsed.dtd.element("a").is_none(), "{:?}", parsed.diagnostics);
    // Groups nested very deep do not overflow the stack.
    let deep = format!("<!ELEMENT a {}b{}>", "(".repeat(5000), ")".repeat(5000));
    assert!(codes(&internal(&deep)).contains(&Code::DtdLimit));
    // A great many declarations are read, and the problems listed are capped.
    let many = "<!ELEMENT>".repeat(5000);
    let parsed = internal(&many);
    assert!(parsed.diagnostics.len() <= xmlcraft_dtd::MAX_DIAGNOSTICS + 1);
    assert_eq!(parsed.diagnostics.last().map(|d| d.diagnostic.code), Some(Code::TooManyDiagnostics));
}

proptest::proptest! {
    /// Whatever the text, reading ends without a panic, and every problem is inside the text.
    #[test]
    fn any_text_is_read_without_a_panic_and_the_problems_are_inside_it(
        pieces in proptest::collection::vec(proptest::sample::select(&[
            "<!ELEMENT", "<!ATTLIST", "<!ENTITY", "<!NOTATION", "<![", "INCLUDE", "IGNORE", "[", "]]>", "]", "<!--", "-->", "<?", "?>", "%", "&", ";", "#PCDATA", "(", ")", "|", ",", "*", "+", "?",
            "a", "b", "EMPTY", "ANY", "CDATA", "ID", "NOTATION", "#IMPLIED", "#REQUIRED", "#FIXED", "SYSTEM", "PUBLIC", "NDATA", "'", "\"", " ", "\n", "x y", "%a;", "%b;", "<!ENTITY % a 'A'>",
            "<!ENTITY % b '<!ELEMENT q EMPTY>'>", "é", "日", "\u{1}",
        ][..]), 0..60),
        as_external in proptest::bool::ANY,
    ) {
        let text = pieces.concat();
        let parsed = if as_external { parse_external_subset(&text) } else { internal(&text) };
        for d in &parsed.diagnostics {
            if d.source == 0 && !as_external {
                proptest::prop_assert!(d.diagnostic.span.start <= d.diagnostic.span.end && d.diagnostic.span.end <= text.len(), "{:?} in {} bytes: {text:?}", d.diagnostic.span, text.len());
            }
        }
    }
}
