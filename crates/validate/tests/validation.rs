//! A document against its DTD: each validity constraint, the entities in content, standalone documents, limits.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use xmlcraft_dtd::{LoadRequest, Loaded, NoLoader, SubsetLoader};
use xmlcraft_syntax::{Code, Severity, parse};
use xmlcraft_validate::{Limits, Problems, check, validate_document_with};

#[derive(Default)]
struct Files(HashMap<String, String>);

impl Files {
    fn of(files: &[(&str, &str)]) -> Self {
        Self(files.iter().map(|(n, t)| ((*n).to_owned(), (*t).to_owned())).collect())
    }
}

impl SubsetLoader for Files {
    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String> {
        self.0
            .get(request.system)
            .map(|text| Loaded { text: text.clone(), location: format!("mem:{}", request.system) })
            .ok_or_else(|| format!("{} is not there", request.system))
    }
}

fn problems(text: &str, loader: &mut dyn SubsetLoader) -> Problems {
    let parsed = parse(text);
    check(&parsed.syntax(), text, parsed.diagnostics(), "mem:doc.xml", loader)
}

fn errors(list: &[xmlcraft_dtd::DtdDiagnostic]) -> Vec<Code> {
    list.iter().filter(|d| d.diagnostic.severity() == Severity::Error).map(|d| d.diagnostic.code).collect()
}

/// The validity errors of a document.
fn invalid(text: &str) -> Vec<Code> {
    errors(&problems(text, &mut NoLoader).validity)
}

fn invalid_with(text: &str, files: &[(&str, &str)]) -> Vec<Code> {
    errors(&problems(text, &mut Files::of(files)).validity)
}

/// The document is valid.
fn valid(text: &str) {
    let p = problems(text, &mut NoLoader);
    assert_eq!((errors(&p.well_formedness), errors(&p.validity)), (vec![], vec![]), "{text}");
}

fn doc(dtd: &str, body: &str) -> String {
    format!("<!DOCTYPE r [{dtd}]>{body}")
}

#[test]
fn the_root_element_is_the_one_the_doctype_names_and_is_declared() {
    valid(&doc("<!ELEMENT r ANY>", "<r/>"));
    assert_eq!(invalid(&doc("<!ELEMENT r ANY><!ELEMENT other ANY>", "<other/>")), [Code::VcRootElement]);
    assert_eq!(invalid(&doc("<!ELEMENT other ANY>", "<r/>")), [Code::VcElementNotDeclared]);
    // Declared in a part that was not read: not for us to say.
    assert_eq!(invalid("<!DOCTYPE r SYSTEM 'r.dtd'><r><x/></r>"), Vec::<Code>::new());
    // The same, read.
    assert_eq!(invalid_with("<!DOCTYPE r SYSTEM 'r.dtd'><r><x/></r>", &[("r.dtd", "<!ELEMENT r ANY><!ELEMENT x EMPTY>")]), Vec::<Code>::new());
    assert_eq!(
        invalid_with("<!DOCTYPE r SYSTEM 'r.dtd'><r><y/></r>", &[("r.dtd", "<!ELEMENT r ANY><!ELEMENT x EMPTY>")]),
        [Code::VcElementNotDeclared]
    );
}

#[test]
fn a_document_without_a_doctype_has_nothing_to_be_valid_against() {
    let p = problems("<r/>", &mut NoLoader);
    assert_eq!((errors(&p.well_formedness), errors(&p.validity)), (vec![], vec![Code::VcNoDtd]));
    // A document that is not well-formed is not validated.
    let p = problems("<!DOCTYPE r [<!ELEMENT r ANY>]><r>", &mut NoLoader);
    assert!(!errors(&p.well_formedness).is_empty() && p.validity.is_empty());
}

#[test]
fn an_empty_element_has_no_content_at_all() {
    let dtd = "<!ELEMENT r (e)><!ELEMENT e EMPTY>";
    valid(&doc(dtd, "<r><e/></r>"));
    valid(&doc(dtd, "<r><e></e></r>"));
    for content in ["x", " ", "<!-- c -->", "<?pi?>", "<![CDATA[]]>", "&#32;", "&lt;"] {
        assert_eq!(invalid(&doc(dtd, &format!("<r><e>{content}</e></r>"))), [Code::VcEmptyHasContent], "{content}");
    }
    assert_eq!(invalid(&doc("<!ELEMENT r EMPTY><!ELEMENT x ANY>", "<r><x/></r>")), [Code::VcEmptyHasContent]);
    // Only once.
    assert_eq!(invalid(&doc(dtd, "<r><e>a<!-- c -->b</e></r>")), [Code::VcEmptyHasContent]);
}

#[test]
fn any_content_takes_text_and_declared_elements() {
    let dtd = "<!ELEMENT r ANY><!ELEMENT a EMPTY>";
    valid(&doc(dtd, "<r>text <a/> more<!-- c --><a/></r>"));
    assert_eq!(invalid(&doc(dtd, "<r><b/></r>")), [Code::VcElementNotDeclared]);
}

#[test]
fn mixed_content_takes_text_and_the_elements_listed() {
    let dtd = "<!ELEMENT r (#PCDATA | a | b)*><!ELEMENT a EMPTY><!ELEMENT b EMPTY><!ELEMENT c EMPTY>";
    valid(&doc(dtd, "<r>text<a/>more<b/><a/></r>"));
    valid(&doc(dtd, "<r/>"));
    assert_eq!(invalid(&doc(dtd, "<r>text<c/></r>")), [Code::VcChildNotAllowed]);
    // Text only.
    assert_eq!(invalid(&doc("<!ELEMENT r (#PCDATA)><!ELEMENT a EMPTY>", "<r><a/></r>")), [Code::VcChildNotAllowed]);
    valid(&doc("<!ELEMENT r (#PCDATA)>", "<r>just text</r>"));
}

#[test]
fn element_content_follows_the_model_and_has_no_text() {
    let dtd = "<!ELEMENT r (a, b?, c+)><!ELEMENT a EMPTY><!ELEMENT b EMPTY><!ELEMENT c EMPTY>";
    valid(&doc(dtd, "<r><a/><c/></r>"));
    valid(&doc(dtd, "<r>\n <a/>\n <b/> <c/><c/>\n</r>"));
    valid(&doc(dtd, "<r><a/><!-- c --><c/><?pi?></r>"));
    // Out of order, too much, too little.
    assert_eq!(invalid(&doc(dtd, "<r><b/><a/><c/></r>")), [Code::VcChildNotAllowed]);
    assert_eq!(invalid(&doc(dtd, "<r><a/><c/><a/></r>")), [Code::VcChildNotAllowed]);
    assert_eq!(invalid(&doc(dtd, "<r><a/></r>")), [Code::VcContentIncomplete]);
    assert_eq!(invalid(&doc(dtd, "<r/>")), [Code::VcContentIncomplete]);
    // A mistake is one problem, whatever follows.
    assert_eq!(invalid(&doc(dtd, "<r><c/><c/><c/><a/><a/></r>")), [Code::VcChildNotAllowed]);
    // Text, a CDATA section and references to characters are character data, however blank.
    assert_eq!(invalid(&doc(dtd, "<r>text<a/><c/></r>")), [Code::VcTextNotAllowed]);
    assert_eq!(invalid(&doc(dtd, "<r><a/><![CDATA[ ]]><c/></r>")), [Code::VcTextNotAllowed]);
    assert_eq!(invalid(&doc(dtd, "<r><a/>&#32;<c/></r>")), [Code::VcTextNotAllowed]);
    assert_eq!(invalid(&doc(dtd, "<r><a/>&amp;<c/></r>")), [Code::VcTextNotAllowed]);
}

#[test]
fn the_message_says_what_was_expected() {
    let text = doc("<!ELEMENT r (a, (b | c))><!ELEMENT a EMPTY><!ELEMENT b EMPTY><!ELEMENT c EMPTY>", "<r><a/><a/></r>");
    let p = problems(&text, &mut NoLoader);
    let message = p.validity.iter().find(|d| d.diagnostic.code == Code::VcChildNotAllowed).map(|d| d.diagnostic.message()).expect("a problem");
    assert!(message.contains("'a'") && message.contains("b, c"), "{message}");
    let text = doc("<!ELEMENT r (a, b)><!ELEMENT a EMPTY><!ELEMENT b EMPTY>", "<r><a/></r>");
    let message = problems(&text, &mut NoLoader).validity[0].diagnostic.message();
    assert!(message.contains("still needs b"), "{message}");
}

#[test]
fn attributes_are_declared_required_fixed_and_of_their_type() {
    let dtd = "<!ELEMENT r EMPTY><!ATTLIST r req CDATA #REQUIRED opt CDATA #IMPLIED fix CDATA #FIXED 'f' kind (x | y) 'x' tok NMTOKEN #IMPLIED toks NMTOKENS #IMPLIED>";
    valid(&doc(dtd, "<r req='1'/>"));
    valid(&doc(dtd, "<r req='1' fix='f' kind='y' tok=' a ' toks='a  b\tc'/>"));
    assert_eq!(invalid(&doc(dtd, "<r/>")), [Code::VcRequiredAttribute]);
    assert_eq!(invalid(&doc(dtd, "<r req='1' nope='2'/>")), [Code::VcAttributeNotDeclared]);
    assert_eq!(invalid(&doc(dtd, "<r req='1' fix='g'/>")), [Code::VcFixedAttribute]);
    assert_eq!(invalid(&doc(dtd, "<r req='1' kind='z'/>")), [Code::VcAttributeValue]);
    assert_eq!(invalid(&doc(dtd, "<r req='1' tok='a b'/>")), [Code::VcAttributeValue]);
    assert_eq!(invalid(&doc(dtd, "<r req='1' toks=''/>")), [Code::VcAttributeValue]);
    // The attribute of an element is declared for that element.
    assert_eq!(
        invalid(&doc("<!ELEMENT r (e)><!ELEMENT e EMPTY><!ATTLIST r a CDATA #IMPLIED>", "<r a='1'><e a='1'/></r>")),
        [Code::VcAttributeNotDeclared]
    );
    // References in values.
    valid(&doc(&format!("<!ENTITY v 'x'>{dtd}"), "<r req='&v;&#65;&lt;' kind='&v;'/>"));
    assert_eq!(invalid(&doc(&format!("<!ENTITY v 'z'>{dtd}"), "<r req='1' kind='&v;'/>")), [Code::VcAttributeValue]);
}

#[test]
fn ids_are_unique_and_idrefs_name_them() {
    let dtd = "<!ELEMENT r (e*)><!ELEMENT e EMPTY><!ATTLIST e id ID #IMPLIED ref IDREF #IMPLIED refs IDREFS #IMPLIED>";
    valid(&doc(dtd, "<r><e id='a'/><e id='b' ref='a'/><e refs='a b'/></r>"));
    // A reference may come before the ID.
    valid(&doc(dtd, "<r><e ref='late'/><e id='late'/></r>"));
    assert_eq!(invalid(&doc(dtd, "<r><e id='a'/><e id='a'/></r>")), [Code::VcDuplicateId]);
    assert_eq!(invalid(&doc(dtd, "<r><e ref='nowhere'/></r>")), [Code::VcIdrefNotFound]);
    assert_eq!(invalid(&doc(dtd, "<r><e id='a'/><e refs='a nowhere also'/></r>")), [Code::VcIdrefNotFound, Code::VcIdrefNotFound]);
    assert_eq!(invalid(&doc(dtd, "<r><e id='1a'/></r>")), [Code::VcAttributeValue]);
    // Names in a namespace-aware document have no colon.
    assert_eq!(invalid(&doc(dtd, "<r><e id='a:b'/></r>")), [Code::VcAttributeValue]);
}

#[test]
fn entity_attributes_name_unparsed_entities() {
    let dtd = "<!NOTATION n SYSTEM 'n'><!ENTITY pic SYSTEM 'p.gif' NDATA n><!ENTITY text 'x'><!ELEMENT r EMPTY><!ATTLIST r e ENTITY #IMPLIED es ENTITIES #IMPLIED>";
    valid(&doc(dtd, "<r e='pic' es='pic pic'/>"));
    assert_eq!(invalid(&doc(dtd, "<r e='text'/>")), [Code::VcEntityAttribute]);
    assert_eq!(invalid(&doc(dtd, "<r e='nope'/>")), [Code::VcEntityAttribute]);
    assert_eq!(invalid(&doc(dtd, "<r es='pic nope'/>")), [Code::VcEntityAttribute]);
}

#[test]
fn entities_in_content_are_read_in_the_place_they_are_referred_to() {
    let dtd = "<!ENTITY kids '<a/><b/>'><!ENTITY txt 'some text'><!ENTITY sp '  '><!ENTITY cs '&#38;#32;'><!ELEMENT r (a, b)><!ELEMENT a EMPTY><!ELEMENT b EMPTY>";
    // The elements an entity brings count as the children of the element that refers to it.
    valid(&doc(dtd, "<r>&kids;</r>"));
    valid(&doc(dtd, "<r><a/>&sp;<b/></r>"));
    assert_eq!(invalid(&doc(dtd, "<r>&kids;&kids;</r>")), [Code::VcChildNotAllowed]);
    assert_eq!(
        invalid(&doc("<!ENTITY kids '<a/><b/>'><!ELEMENT r (a)><!ELEMENT a EMPTY><!ELEMENT b EMPTY>", "<r>&kids;</r>")),
        [Code::VcChildNotAllowed]
    );
    // Text from an entity is text; white space from an entity is white space, a character reference in it is not.
    assert_eq!(invalid(&doc(dtd, "<r><a/>&txt;<b/></r>")), [Code::VcTextNotAllowed]);
    assert_eq!(invalid(&doc(dtd, "<r><a/>&cs;<b/></r>")), [Code::VcTextNotAllowed]);
    // An element with a declaration of its own, inside an entity.
    valid(&doc("<!ENTITY e '<q><a/></q>'><!ELEMENT r (q)><!ELEMENT q (a)><!ELEMENT a EMPTY>", "<r>&e;</r>"));
    assert_eq!(
        invalid(&doc("<!ENTITY e '<q><q/></q>'><!ELEMENT r (q)><!ELEMENT q (a)><!ELEMENT a EMPTY>", "<r>&e;</r>")),
        [Code::VcChildNotAllowed, Code::VcContentIncomplete]
    );
    // A reference is content in an EMPTY element.
    assert_eq!(invalid(&doc("<!ENTITY e ''><!ELEMENT r EMPTY>", "<r>&e;</r>")), [Code::VcEmptyHasContent]);
}

#[test]
fn a_problem_inside_an_entity_is_reported_at_the_reference() {
    let text = doc("<!ENTITY e '<b/>'><!ELEMENT r (a)><!ELEMENT a EMPTY><!ELEMENT b EMPTY>", "<r>before &e; after</r>");
    let p = problems(&text, &mut NoLoader);
    let at = p.validity.iter().find(|d| d.diagnostic.code == Code::VcChildNotAllowed).map(|d| d.diagnostic.span.clone()).expect("a problem");
    assert_eq!(&text[at], "&e;");
}

#[test]
fn an_undeclared_entity_is_a_validity_error_where_it_is_not_a_well_formedness_error() {
    // An internal parameter-entity reference is enough to make it so (errata E13).
    let text = doc("<!ENTITY % pe \"<!ENTITY ent1 'text'>\">%pe;<!ELEMENT r ANY>", "<r>&ent2;</r>");
    let p = problems(&text, &mut NoLoader);
    assert_eq!((errors(&p.well_formedness), errors(&p.validity)), (vec![], vec![Code::VcEntityNotDeclared]));
    // Where the entity may be declared out of sight, nothing is said.
    assert_eq!(invalid("<!DOCTYPE r SYSTEM 'r.dtd' [<!ELEMENT r ANY>]><r>&nowhere;</r>"), Vec::<Code>::new());
}

#[test]
fn external_entities_are_not_read_and_their_content_is_not_judged() {
    let dtd = "<!ENTITY ext SYSTEM 'e.ent'><!ELEMENT r (a, b)><!ELEMENT a EMPTY><!ELEMENT b EMPTY>";
    // The element is not called incomplete for what an entity that cannot be read might hold.
    valid(&doc(dtd, "<r><a/>&ext;</r>"));
    assert_eq!(invalid(&doc(dtd, "<r><a/></r>")), [Code::VcContentIncomplete]);
    assert_eq!(invalid(&doc("<!ENTITY ext SYSTEM 'e.ent'><!ELEMENT r EMPTY>", "<r>&ext;</r>")), [Code::VcEmptyHasContent]);
}

#[test]
fn a_standalone_document_does_not_lean_on_declarations_outside_it() {
    let external = "<!ELEMENT r (a)><!ELEMENT a EMPTY><!ATTLIST r d CDATA 'dflt' t NMTOKEN #IMPLIED>";
    let head = "<?xml version='1.0' standalone='yes'?><!DOCTYPE r SYSTEM 'r.dtd'>";
    // The same document that is not standalone is valid.
    assert_eq!(invalid_with("<?xml version='1.0'?><!DOCTYPE r SYSTEM 'r.dtd'><r d='x'><a/></r>", &[("r.dtd", external)]), Vec::<Code>::new());
    // A default from outside, white space in element content declared outside, a value the outside type would normalise.
    assert_eq!(invalid_with(&format!("{head}<r><a/></r>"), &[("r.dtd", external)]), [Code::VcStandalone]);
    assert_eq!(invalid_with(&format!("{head}<r d='x'>\n<a/></r>"), &[("r.dtd", external)]), [Code::VcStandalone]);
    assert_eq!(invalid_with(&format!("{head}<r d='x' t=' tok '><a/></r>"), &[("r.dtd", external)]), [Code::VcStandalone]);
    assert_eq!(invalid_with(&format!("{head}<r d='x' t='tok'><a/></r>"), &[("r.dtd", external)]), Vec::<Code>::new());
    // Declared in the document itself, white space is fine.
    assert_eq!(invalid("<?xml version='1.0' standalone='yes'?><!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY>]><r>\n<a/>\n</r>"), Vec::<Code>::new());
}

#[test]
fn the_declarations_themselves_are_part_of_the_check() {
    assert_eq!(invalid(&doc("<!ELEMENT r ANY><!ELEMENT r EMPTY>", "<r/>")), [Code::VcDuplicateElement]);
    assert_eq!(invalid(&doc("<!ELEMENT r ANY><!ATTLIST r a ID #FIXED 'x'>", "<r/>")), [Code::VcIdAttributeDefault]);
    // The nesting of parameter entities in declarations is a validity constraint, found when the DTD is read.
    let p = problems("<!DOCTYPE r SYSTEM 'r.dtd'><r/>", &mut Files::of(&[("r.dtd", "<!ENTITY % e '(a'><!ELEMENT r %e;)>")]));
    assert!(errors(&p.well_formedness).is_empty(), "{:?}", p.well_formedness);
    assert!(errors(&p.validity).contains(&Code::VcPeNesting), "{:?}", p.validity);
}

#[test]
fn a_non_deterministic_model_is_a_warning_not_an_error() {
    let p = problems(&doc("<!ELEMENT r (a | a)><!ELEMENT a EMPTY>", "<r><a/></r>"), &mut NoLoader);
    assert!(errors(&p.validity).is_empty());
    assert!(p.validity.iter().any(|d| d.diagnostic.code == Code::DtdNonDeterministic));
}

#[test]
fn entities_that_multiply_and_documents_that_nest_are_bounded() {
    let started = Instant::now();
    // Ten levels of ten references, in content: ten billion elements if it were all read.
    let mut dtd = String::from("<!ENTITY l0 '<a/>'>");
    for level in 1..=10 {
        dtd.push_str(&format!("<!ENTITY l{level} '{}'>", format!("&l{};", level - 1).repeat(10)));
    }
    let text = doc(&format!("<!ELEMENT r (a)*><!ELEMENT a EMPTY>{dtd}"), "<r>&l10;<zzz/></r>");
    let p = problems(&text, &mut NoLoader);
    assert!(p.validity.iter().any(|d| d.diagnostic.code == Code::DtdLimit), "{:?}", p.validity);
    // What comes after is still checked.
    assert!(p.validity.iter().any(|d| d.diagnostic.code == Code::VcElementNotDeclared), "{:?}", p.validity);
    // Nested nine hundred deep.
    let deep = format!("{}{}", "<e>".repeat(900), "</e>".repeat(900));
    let p = problems(&doc("<!ELEMENT r (e)><!ELEMENT e ANY>", &format!("<r>{deep}</r>")), &mut NoLoader);
    assert!(errors(&p.well_formedness).is_empty() && errors(&p.validity).is_empty(), "{:?}", p.validity);
    assert!(started.elapsed() < Duration::from_secs(30), "took {:?}", started.elapsed());
}

/// The validity problems with other limits.
fn with_limits(text: &str, limits: &Limits) -> Vec<Code> {
    let parsed = parse(text);
    let checked = xmlcraft_dtd::check_document(&parsed.syntax(), text, parsed.diagnostics(), "", &mut NoLoader);
    let read = checked.dtd.expect("a DTD");
    let compiled = xmlcraft_dtd::compile(&read.dtd, checked.root_name.as_deref());
    validate_document_with(&parsed.syntax(), &compiled.grammar, false, limits).iter().map(|d| d.code).collect()
}

#[test]
fn the_limits_on_entities_are_those_given_and_spare_the_document_itself() {
    // Entities nested deeper than the limit are given up (the default is forty).
    let mut dtd = String::from("<!ENTITY e0 '<a/>'>");
    for level in 1..=60 {
        dtd.push_str(&format!("<!ENTITY e{level} '&e{};'>", level - 1));
    }
    let text = |level: usize| doc(&format!("<!ELEMENT r (a)*><!ELEMENT a EMPTY>{dtd}"), &format!("<r>&e{level};</r>"));
    assert_eq!(with_limits(&text(30), &Limits::default()), Vec::<Code>::new());
    assert_eq!(with_limits(&text(60), &Limits::default()), [Code::DtdLimit]);
    assert_eq!(with_limits(&text(30), &Limits { entity_depth: 10, ..Limits::default() }), [Code::DtdLimit]);
    // The budget for what entities bring is not spent by the document: a thousand elements first, then an entity of one.
    let body = format!("<r>{}&e0;</r>", "<a/>".repeat(1000));
    let small = Limits { entity_events: 10, ..Limits::default() };
    assert_eq!(with_limits(&doc(&format!("<!ELEMENT r (a)*><!ELEMENT a EMPTY>{dtd}"), &body), &small), Vec::<Code>::new());
    // Too many elements open at once.
    let nested = format!("{}{}", "<e>".repeat(50), "</e>".repeat(50));
    let open = Limits { open_elements: 20, ..Limits::default() };
    assert!(with_limits(&doc("<!ELEMENT r (e)><!ELEMENT e ANY>", &format!("<r>{nested}</r>")), &open).contains(&Code::DtdLimit));
}
