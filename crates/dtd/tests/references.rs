//! References to general entities: the well-formedness rules that need the declarations (declared, parsed, not external in an attribute
//! value, no `<` in an attribute value, no recursion, a replacement text that is well-formed where it is used), for the document and for
//! the default values of attributes.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use xmlcraft_dtd::{LoadRequest, Loaded, NoLoader, SubsetLoader, problems_of_document};
use xmlcraft_syntax::{Code, Severity, parse};

/// External texts by system identifier.
#[derive(Default)]
struct Files(HashMap<String, String>);

impl Files {
    fn of(files: &[(&str, &str)]) -> Self {
        Self(files.iter().map(|(name, text)| ((*name).to_owned(), (*text).to_owned())).collect())
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

/// The codes of the errors in a document, with its external texts.
fn errors_with(text: &str, loader: &mut dyn SubsetLoader) -> Vec<Code> {
    let parsed = parse(text);
    problems_of_document(&parsed.syntax(), text, parsed.diagnostics(), "mem:doc.xml", loader)
        .into_iter()
        .filter(|d| d.diagnostic.severity() == Severity::Error)
        .map(|d| d.diagnostic.code)
        .collect()
}

fn errors(text: &str) -> Vec<Code> {
    errors_with(text, &mut NoLoader)
}

/// The document is well-formed as far as the DTD and its entities go.
fn clean(text: &str) {
    assert_eq!(errors(text), Vec::<Code>::new(), "{text}");
}

fn clean_with(text: &str, files: &[(&str, &str)]) {
    assert_eq!(errors_with(text, &mut Files::of(files)), Vec::<Code>::new(), "{text}");
}

/// Exactly this one problem.
fn broken(text: &str, code: Code) {
    assert_eq!(errors(text), vec![code], "{text}");
}

fn broken_with(text: &str, files: &[(&str, &str)], code: Code) {
    assert_eq!(errors_with(text, &mut Files::of(files)), vec![code], "{text}");
}

#[test]
fn an_entity_must_be_declared_where_the_document_has_all_of_its_declarations() {
    broken("<!DOCTYPE r [<!ELEMENT r ANY>]><r>&x;</r>", Code::UndeclaredEntity);
    broken("<!DOCTYPE r [<!ELEMENT r ANY><!ATTLIST r a CDATA #IMPLIED>]><r a='&x;'/>", Code::UndeclaredEntity);
    clean("<!DOCTYPE r [<!ENTITY x 'v'>]><r a='&x;'>&x;</r>");
    // The five that need no declaration, also when the document declares them.
    clean("<!DOCTYPE r [<!ELEMENT r ANY>]><r a='&lt;&gt;&amp;&quot;&apos;'>&lt;&gt;&amp;&quot;&apos;</r>");
    clean("<!DOCTYPE r [<!ENTITY lt '&#38;#60;'>]><r>&lt;</r>");
    // A name that may be declared in a part that is not here is not an error: the external subset, or a parameter entity.
    clean("<!DOCTYPE r SYSTEM 'r.dtd'><r>&x;</r>");
    clean("<!DOCTYPE r [<!ENTITY % p '<!-- c -->'> %p;]><r>&x;</r>");
    clean("<!DOCTYPE r PUBLIC '-//X//EN' 'x.dtd'><r a='&x;'/>");
}

#[test]
fn a_standalone_document_declares_its_entities_in_itself() {
    // Not declared at all, and declared in the external subset: both break the constraint.
    broken("<?xml version='1.0' standalone='yes'?><!DOCTYPE r SYSTEM 'r.dtd'><r>&x;</r>", Code::UndeclaredEntity);
    broken_with(
        "<?xml version='1.0' standalone='yes'?><!DOCTYPE r SYSTEM 'r.dtd'><r>&x;</r>",
        &[("r.dtd", "<!ENTITY x 'v'>")],
        Code::EntityExternalStandalone,
    );
    // The same document that is not standalone is fine.
    clean_with("<?xml version='1.0'?><!DOCTYPE r SYSTEM 'r.dtd'><r>&x;</r>", &[("r.dtd", "<!ENTITY x 'v'>")]);
    clean_with("<?xml version='1.0' standalone='no'?><!DOCTYPE r SYSTEM 'r.dtd'><r>&x;</r>", &[("r.dtd", "<!ENTITY x 'v'>")]);
    // An entity declared in the document is declared.
    clean_with("<?xml version='1.0' standalone='yes'?><!DOCTYPE r SYSTEM 'r.dtd' [<!ENTITY x 'v'>]><r>&x;</r>", &[("r.dtd", "<!ENTITY y 'w'>")]);
}

#[test]
fn an_unparsed_entity_is_not_referenced_in_text_or_in_an_attribute() {
    let dtd = "<!NOTATION n SYSTEM 'n'><!ENTITY img SYSTEM 'i.png' NDATA n>";
    broken(&format!("<!DOCTYPE r [{dtd}]><r>&img;</r>"), Code::EntityUnparsed);
    broken(&format!("<!DOCTYPE r [{dtd}]><r a='&img;'/>"), Code::EntityUnparsed);
    // Reached through another entity.
    broken(&format!("<!DOCTYPE r [{dtd}<!ENTITY via '&img;'>]><r>&via;</r>"), Code::EntityUnparsed);
}

#[test]
fn an_external_entity_is_not_referenced_in_an_attribute_value() {
    let dtd = "<!ENTITY ext SYSTEM 'e.ent'>";
    broken(&format!("<!DOCTYPE r [{dtd}]><r a='&ext;'/>"), Code::EntityExternalInAttribute);
    broken(&format!("<!DOCTYPE r [{dtd}<!ENTITY via '&ext;'>]><r a='x&via;y'/>"), Code::EntityExternalInAttribute);
    // In text it is allowed (it is read when it can be).
    clean_with(&format!("<!DOCTYPE r [{dtd}]><r>&ext;</r>"), &[("e.ent", "text")]);
    clean(&format!("<!DOCTYPE r [{dtd}]><r>&ext;</r>"));
}

#[test]
fn the_replacement_text_of_an_entity_in_an_attribute_value_has_no_less_than_sign() {
    broken("<!DOCTYPE r [<!ENTITY e 'a<b'>]><r a='&e;'/>", Code::EntityLtInAttribute);
    broken("<!DOCTYPE r [<!ENTITY e 'a<b'><!ENTITY f '&e;'><!ENTITY g 'x&f;'>]><r a='&g;'/>", Code::EntityLtInAttribute);
    // A character reference to `<` is written as `&#38;#60;` and is not a `<` in the replacement text.
    clean("<!DOCTYPE r [<!ENTITY e '&#38;#60;'>]><r a='&e;'/>");
    // The same text is fine as content when it is well-formed there.
    clean("<!DOCTYPE r [<!ENTITY e '<b/>'>]><r>&e;</r>");
}

#[test]
fn an_entity_that_refers_to_itself_is_an_error_where_it_is_used() {
    broken("<!DOCTYPE r [<!ENTITY a '&a;'>]><r>&a;</r>", Code::DtdRecursiveEntity);
    broken("<!DOCTYPE r [<!ENTITY a '&b;'><!ENTITY b '&c;'><!ENTITY c '&a;'>]><r>&a;</r>", Code::DtdRecursiveEntity);
    broken("<!DOCTYPE r [<!ENTITY a '&b;'><!ENTITY b '&a;'>]><r x='&a;'/>", Code::DtdRecursiveEntity);
    // Declared and never used: not an error.
    clean("<!DOCTYPE r [<!ENTITY a '&b;'><!ENTITY b '&a;'>]><r/>");
    // The same entity twice in a text is not a cycle.
    clean("<!DOCTYPE r [<!ENTITY d 'x'><!ENTITY b '&d;'><!ENTITY c '&d;'><!ENTITY a '&b;&c;&d;'>]><r>&a;&a;</r>");
}

#[test]
fn the_replacement_text_of_an_entity_in_text_is_well_formed_content() {
    for (declaration, why) in [
        ("<!ENTITY e '</foo><foo>'>", "end tag that closes nothing"),
        ("<!ENTITY e '&#38;#9'>", "a partial character reference"),
        ("<!ENTITY e '&#38;'>", "a bare ampersand"),
        ("<!ENTITY e '&#60;!--'>", "a comment that is not closed"),
        ("<!ENTITY e '&#60;foo>'>", "a start tag that is not closed"),
        ("<!ENTITY e \"<foo a='&#60;'></foo>\">", "a `<` in an attribute value"),
        ("<!ENTITY e \"<foo a='&#38;'></foo>\">", "a bare ampersand in an attribute value"),
        ("<!ENTITY e \"<?xml encoding='UTF-8'?>\">", "a text declaration in an internal entity"),
        ("<!ENTITY e 'x]]>y'>", "a CDATA end in text"),
    ] {
        let found = errors(&format!("<!DOCTYPE r [{declaration}]><r>&e;</r>"));
        assert_eq!(found, vec![Code::EntityNotWellFormed], "{why}");
    }
    // Used in an attribute value, an ampersand that starts nothing is the problem (and a start tag is a `<`).
    broken("<!DOCTYPE r [<!ENTITY e '&#38;'>]><r a='&e;'/>", Code::EntityNotWellFormed);
    // Markup, nested references and text are content.
    clean("<!DOCTYPE r [<!ENTITY e '<b a=\"1\">x &amp; y</b>t<![CDATA[<]]><!-- c --><?pi x?>'><!ENTITY f '&e;&e;'>]><r>&f;</r>");
    // The text may use names and prefixes that only the place it is used knows.
    clean("<!DOCTYPE r [<!ENTITY e '<p:b/>'>]><r xmlns:p='urn:p'>&e;</r>");
    // An end tag that would close the element the text is parsed in leaves something behind.
    broken("<!DOCTYPE r [<!ENTITY e '</_>'>]><r>&e;</r>", Code::EntityNotWellFormed);
    broken("<!DOCTYPE r [<!ENTITY e '</_><_>'>]><r>&e;</r>", Code::EntityNotWellFormed);
}

#[test]
fn a_reference_in_the_replacement_text_is_checked_where_the_text_is_used() {
    // Undeclared in the text of an entity, used in text and in an attribute.
    broken("<!DOCTYPE r [<!ENTITY foo '&bar;'>]><r a='&foo;'/>", Code::UndeclaredEntity);
    broken("<!DOCTYPE r [<!ENTITY foo '&bar;'>]><r>&foo;</r>", Code::UndeclaredEntity);
    // Not used: not looked at.
    clean("<!DOCTYPE r [<!ENTITY foo '&bar;'>]><r/>");
    // A reference in an attribute value of the text is an attribute reference.
    broken("<!DOCTYPE r [<!ENTITY ext SYSTEM 'e'><!ENTITY foo '<b a=\"&ext;\"/>'>]><r>&foo;</r>", Code::EntityExternalInAttribute);
}

#[test]
fn the_default_values_of_attributes_are_checked() {
    let decl = "<!ELEMENT r ANY>";
    broken(&format!("<!DOCTYPE r [{decl}<!ATTLIST r a CDATA '&foo;'>]><r/>"), Code::UndeclaredEntity);
    // The entity must be declared before.
    broken(&format!("<!DOCTYPE r [{decl}<!ATTLIST r a CDATA '&e;'><!ENTITY e 'v'>]><r/>"), Code::UndeclaredEntity);
    broken(&format!("<!DOCTYPE r [{decl}<!ATTLIST r a CDATA #FIXED '&e;'><!ENTITY e 'v'>]><r/>"), Code::UndeclaredEntity);
    clean(&format!("<!DOCTYPE r [{decl}<!ENTITY e 'v'><!ATTLIST r a CDATA '&e;'>]><r/>"));
    broken(&format!("<!DOCTYPE r [{decl}<!ENTITY e SYSTEM 'x'><!ATTLIST r a CDATA '&e;'>]><r/>"), Code::EntityExternalInAttribute);
    broken(
        &format!("<!DOCTYPE r [{decl}<!NOTATION n SYSTEM 'n'><!ENTITY e SYSTEM 'x' NDATA n><!ATTLIST r a CDATA '&e;'>]><r/>"),
        Code::EntityUnparsed,
    );
    broken(&format!("<!DOCTYPE r [{decl}<!ENTITY e1 '&e2;'><!ENTITY e2 '&e1;'><!ATTLIST r a CDATA '&e1;'>]><r/>"), Code::DtdRecursiveEntity);
    broken(&format!("<!DOCTYPE r [{decl}<!ENTITY e 'a<b'><!ATTLIST r a CDATA '&e;'>]><r/>"), Code::EntityLtInAttribute);
    // Where the entity may be declared out of sight, an undeclared name is not an error, but the other rules still hold.
    clean(&format!("<!DOCTYPE r SYSTEM 'r.dtd' [{decl}<!ATTLIST r a CDATA '&foo;'>]><r/>"));
    broken(&format!("<!DOCTYPE r SYSTEM 'r.dtd' [{decl}<!ENTITY e SYSTEM 'x'><!ATTLIST r a CDATA '&e;'>]><r/>"), Code::EntityExternalInAttribute);
    // A default value in the external subset is not the document's: an undeclared name there is for validity to say.
    clean_with("<!DOCTYPE r SYSTEM 'r.dtd'><r/>", &[("r.dtd", "<!ELEMENT r ANY><!ATTLIST r a CDATA '&later;'><!ENTITY later 'v'>")]);
    broken_with(
        "<!DOCTYPE r SYSTEM 'r.dtd'><r/>",
        &[("r.dtd", "<!ELEMENT r ANY><!ENTITY e SYSTEM 'x'><!ATTLIST r a CDATA '&e;'>")],
        Code::EntityExternalInAttribute,
    );
}

#[test]
fn an_external_entity_is_read_as_content_when_there_is_a_way_to_read_it() {
    let doc = "<!DOCTYPE r [<!ENTITY e SYSTEM 'e.ent'>]><r>&e;</r>";
    clean_with(doc, &[("e.ent", "<a>text</a> more")]);
    clean_with(doc, &[("e.ent", "<?xml version='1.0' encoding='UTF-8'?><a/>")]);
    clean_with(doc, &[("e.ent", "<?xml encoding='UTF-8'?>x")]);
    // The text declaration.
    for text in [
        "<?xml encoding='UTF-8' version='1.0'?>x",
        "<?XML version='1.0' encoding='UTF-8'?>x",
        "<?xml version='1.0'?>x",
        "<?xml encoding='UTF-8' standalone='no'?>x",
        "<?xml encoding?>x",
        "<?xml encoding 'UTF-8'?>x",
        "<?xml encoding='UTF-8'",
        "<?xml version='1.1' encoding='UTF-8'?>x",
    ] {
        let found = errors_with(doc, &mut Files::of(&[("e.ent", text)]));
        assert_eq!(found, vec![Code::EntityNotWellFormed], "{text}");
    }
    // Not at the start, or twice.
    for text in
        ["\n<?xml version='1.0' encoding='UTF-8'?>x", "x<?xml version='1.0' encoding='UTF-8'?>", "<?xml encoding='UTF-8'?><?xml encoding='UTF-8'?>"]
    {
        let found = errors_with(doc, &mut Files::of(&[("e.ent", text)]));
        assert_eq!(found, vec![Code::EntityNotWellFormed], "{text}");
    }
    // The content must be balanced.
    broken_with(doc, &[("e.ent", "<a>")], Code::EntityNotWellFormed);
    broken_with(doc, &[("e.ent", "</a>")], Code::EntityNotWellFormed);
    // An entity that is itself, or that contains one that is in the text.
    broken_with(doc, &[("e.ent", "&e;")], Code::DtdRecursiveEntity);
    broken_with(
        "<!DOCTYPE r [<!ENTITY e SYSTEM 'e.ent'><!ENTITY f SYSTEM 'f.ent'>]><r>&e;</r>",
        &[("e.ent", "&f;"), ("f.ent", "&e;")],
        Code::DtdRecursiveEntity,
    );
    clean_with("<!DOCTYPE r [<!ENTITY e SYSTEM 'e.ent'><!ENTITY f SYSTEM 'f.ent'>]><r>&e;</r>", &[("e.ent", "&f;"), ("f.ent", "<f/>")]);
    // A version the document cannot take only matters to a document of that version.
    clean_with("<?xml version='1.1'?><!DOCTYPE r [<!ENTITY e SYSTEM 'e.ent'>]><r>&e;</r>", &[("e.ent", "<?xml version='1.1' encoding='UTF-8'?>x")]);
}

#[test]
fn an_external_entity_that_cannot_be_read_is_a_warning_and_is_not_checked() {
    let doc = "<!DOCTYPE r [<!ENTITY e SYSTEM 'missing.ent'>]><r>&e;&e;</r>";
    let parsed = parse(doc);
    let all = problems_of_document(&parsed.syntax(), doc, parsed.diagnostics(), "mem:doc.xml", &mut Files::default());
    let not_read: Vec<_> = all.iter().filter(|d| d.diagnostic.code == Code::DtdExternalNotRead).collect();
    assert_eq!(not_read.len(), 1, "once, where it was first used: {all:?}");
    assert_eq!(not_read[0].diagnostic.severity(), Severity::Warning);
    assert!(all.iter().all(|d| d.diagnostic.severity() != Severity::Error), "{all:?}");
}

#[test]
fn entities_that_multiply_cost_no_more_than_their_declarations() {
    // Ten levels of ten references each: ten billion characters if it were expanded.
    let mut dtd = String::from("<!ENTITY l0 'ha'>");
    for level in 1..=10 {
        dtd.push_str(&format!("<!ENTITY l{level} '{}'>", format!("&l{};", level - 1).repeat(10)));
    }
    let started = Instant::now();
    clean(&format!("<!DOCTYPE r [{dtd}]><r>&l10;&l10;<e a='&l10;'/></r>"));
    // A million references to one entity.
    let body = "&l1;".repeat(1_000_000);
    clean(&format!("<!DOCTYPE r [{dtd}]><r>{body}</r>"));
    assert!(started.elapsed() < Duration::from_secs(20), "took {:?}", started.elapsed());
}

#[test]
fn entities_nested_very_deep_are_cut_off_without_overflowing_the_stack() {
    let mut dtd = String::from("<!ENTITY e0 'x'>");
    for level in 1..=5000 {
        dtd.push_str(&format!("<!ENTITY e{level} '&e{};'>", level - 1));
    }
    let found = errors(&format!("<!DOCTYPE r [{dtd}]><r>&e5000;</r>"));
    assert_eq!(found, vec![Code::DtdLimit]);
    // A chain that is not too deep is fine.
    let mut dtd = String::from("<!ENTITY e0 'x'>");
    for level in 1..=100 {
        dtd.push_str(&format!("<!ENTITY e{level} '&e{};'>", level - 1));
    }
    clean(&format!("<!DOCTYPE r [{dtd}]><r>&e100;</r>"));
}

#[test]
fn a_document_with_no_doctype_is_left_to_the_syntax_check() {
    // The undeclared entity is reported once, by the parser.
    assert_eq!(errors("<r>&x;</r>"), vec![Code::UndeclaredEntity]);
}

#[test]
fn the_problems_are_where_the_reference_is() {
    let text = "<!DOCTYPE r [<!ENTITY a '&b;'><!ENTITY b '&a;'>]><r>ok &a; end</r>";
    let parsed = parse(text);
    let all = problems_of_document(&parsed.syntax(), text, parsed.diagnostics(), "", &mut NoLoader);
    let problem = all.iter().find(|d| d.diagnostic.code == Code::DtdRecursiveEntity).expect("a problem");
    assert_eq!(problem.source, 0);
    assert_eq!(&text[problem.diagnostic.span.clone()], "&a;");
    let text = "<!DOCTYPE r [<!ELEMENT r ANY><!ATTLIST r a CDATA '&nope;'>]><r/>";
    let parsed = parse(text);
    let all = problems_of_document(&parsed.syntax(), text, parsed.diagnostics(), "", &mut NoLoader);
    let problem = all.iter().find(|d| d.diagnostic.code == Code::UndeclaredEntity).expect("a problem");
    assert!(text[problem.diagnostic.span.clone()].contains("a CDATA"), "{:?}", &text[problem.diagnostic.span.clone()]);
}
