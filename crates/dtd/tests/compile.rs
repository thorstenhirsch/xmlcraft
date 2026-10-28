//! A DTD compiled to a grammar: the first declaration counts, content models become automata, the validity constraints on the
//! declarations are checked, and attribute values are read the way a processor reads them.

use xmlcraft_dtd::{ValueProblem, ValueReader, check_value, compile, parse_internal_subset};
use xmlcraft_grammar::{AttributeDefault, AttributeType, Content, EntityKind, Grammar, NO_DECLARATION, Name, SchemaKind};
use xmlcraft_syntax::{Code, Severity};

struct Compiled {
    grammar: Grammar,
    codes: Vec<(Code, Severity)>,
}

fn grammar_of(subset: &str) -> Compiled {
    let parsed = parse_internal_subset(subset, 0);
    assert!(parsed.diagnostics.is_empty(), "{subset:?}: {:?}", parsed.diagnostics);
    let compiled = compile(&parsed.dtd, Some("r"));
    Compiled { grammar: compiled.grammar, codes: compiled.diagnostics.iter().map(|d| (d.diagnostic.code, d.diagnostic.severity())).collect() }
}

fn errors(subset: &str) -> Vec<Code> {
    grammar_of(subset).codes.into_iter().filter(|(_, s)| *s == Severity::Error).map(|(c, _)| c).collect()
}

fn n(name: &str) -> Name {
    Name::local(name)
}

#[test]
fn the_declarations_become_a_grammar() {
    let c = grammar_of(
        "<!ELEMENT r (a, b*)><!ELEMENT a EMPTY><!ELEMENT b (#PCDATA | a)*><!ELEMENT c ANY>\
         <!ATTLIST r id ID #REQUIRED kind (x|y) 'x' note CDATA #IMPLIED fixed CDATA #FIXED 'f'>\
         <!ENTITY e 'text'><!ENTITY x SYSTEM 'x.xml'><!ENTITY img PUBLIC 'pub' 'i.gif' NDATA gif><!NOTATION gif SYSTEM 'viewer'>",
    );
    assert!(c.codes.is_empty(), "{:?}", c.codes);
    let g = &c.grammar;
    assert_eq!((g.kind(), g.root.clone(), g.incomplete), (SchemaKind::Dtd, Some(n("r")), false));
    assert_eq!(g.elements().iter().map(|e| e.name.local.as_str()).collect::<Vec<_>>(), ["r", "a", "b", "c"]);
    assert!(matches!(g.element(&n("a")).map(|e| &e.content), Some(Content::Empty)));
    assert!(matches!(g.element(&n("c")).map(|e| &e.content), Some(Content::Any)));
    assert!(matches!(g.element(&n("b")).map(|e| &e.content), Some(Content::Mixed(names)) if names == &[n("a")]));
    let r = g.element(&n("r")).expect("r");
    let Content::Elements(content) = &r.content else { panic!("elements") };
    let automaton = &content.automaton;
    // The particles carry the position of the element they name.
    let step = automaton.step(&automaton.start(), &n("a")).expect("a first");
    assert_eq!(step.token, g.element_index(&n("a")).expect("a is declared"));
    assert!(automaton.is_accepting(&automaton.step(&step.state, &n("b")).expect("b").state));
    let attr = |name: &str| r.attribute(&n(name)).map(|a| (a.ty.clone(), a.default.clone()));
    assert_eq!(attr("id"), Some((AttributeType::Id, AttributeDefault::Required)));
    assert_eq!(attr("kind"), Some((AttributeType::Enumeration(vec!["x".into(), "y".into()]), AttributeDefault::Default("x".into()))));
    assert_eq!(attr("fixed"), Some((AttributeType::Cdata, AttributeDefault::Fixed("f".into()))));
    assert_eq!(attr("nope"), None);
    assert_eq!(g.entity("e").map(|e| e.kind.clone()), Some(EntityKind::Internal("text".into())));
    assert_eq!(g.entity("x").map(|e| e.kind.clone()), Some(EntityKind::External { public: None, system: Some("x.xml".into()) }));
    assert_eq!(
        g.entity("img").map(|e| e.kind.clone()),
        Some(EntityKind::Unparsed { public: Some("pub".into()), system: Some("i.gif".into()), notation: "gif".into() })
    );
    assert_eq!(g.notation("gif").and_then(|n| n.system.clone()).as_deref(), Some("viewer"));
}

#[test]
fn a_particle_that_names_an_undeclared_element_has_no_declaration() {
    let c = grammar_of("<!ELEMENT r (known, unknown)><!ELEMENT known EMPTY>");
    assert!(c.codes.is_empty());
    let Some(Content::Elements(content)) = c.grammar.element(&n("r")).map(|e| &e.content) else { panic!("elements") };
    let automaton = &content.automaton;
    let first = automaton.step(&automaton.start(), &n("known")).expect("known");
    let second = automaton.step(&first.state, &n("unknown")).expect("unknown");
    assert_eq!((first.token, second.token), (1, NO_DECLARATION));
}

#[test]
fn the_first_declaration_counts() {
    let c = grammar_of(
        "<!ELEMENT r EMPTY><!ELEMENT r ANY><!ATTLIST r a CDATA 'one'><!ATTLIST r a ID #IMPLIED b CDATA #IMPLIED><!ENTITY e 'first'><!ENTITY e 'second'>",
    );
    assert_eq!(c.codes, [(Code::VcDuplicateElement, Severity::Error)]);
    let r = c.grammar.element(&n("r")).expect("r");
    assert!(matches!(r.content, Content::Empty));
    assert_eq!(r.attribute(&n("a")).map(|a| a.default.clone()), Some(AttributeDefault::Default("one".into())));
    assert!(r.attribute(&n("b")).is_some());
    assert_eq!(c.grammar.entity("e").map(|e| e.kind.clone()), Some(EntityKind::Internal("first".into())));
    // A notation declared twice is an error.
    assert_eq!(errors("<!NOTATION n SYSTEM 'a'><!NOTATION n SYSTEM 'b'>"), [Code::VcDuplicateNotation]);
    // Attributes of an element that is not declared are no error and have no place in the grammar.
    let c = grammar_of("<!ATTLIST ghost a CDATA #IMPLIED>");
    assert!(c.codes.is_empty() && c.grammar.element(&n("ghost")).is_none());
}

#[test]
fn mixed_content_names_each_element_once() {
    assert_eq!(errors("<!ELEMENT r (#PCDATA | a | b | a)*>"), [Code::VcDuplicateMixedName]);
    assert!(errors("<!ELEMENT r (#PCDATA | a | b)*>").is_empty());
    let c = grammar_of("<!ELEMENT r (#PCDATA | a | a)*>");
    assert!(matches!(c.grammar.element(&n("r")).map(|e| &e.content), Some(Content::Mixed(names)) if names == &[n("a")]));
}

#[test]
fn a_content_model_that_is_not_deterministic_is_a_warning() {
    for model in ["(a | a)", "(a?, a)", "((a, b) | (a, c))", "(a*, a)"] {
        let c = grammar_of(&format!("<!ELEMENT r {model}>"));
        assert_eq!(c.codes, [(Code::DtdNonDeterministic, Severity::Warning)], "{model}");
    }
    for model in ["(a, b?)", "(a | b)*", "(a, (b | c)+, d?)", "((a, b) | c)"] {
        assert!(grammar_of(&format!("<!ELEMENT r {model}>")).codes.is_empty(), "{model}");
    }
}

#[test]
fn id_attributes_have_no_default_and_there_is_one_per_element() {
    assert!(errors("<!ELEMENT r ANY><!ATTLIST r a ID #IMPLIED>").is_empty());
    assert!(errors("<!ELEMENT r ANY><!ATTLIST r a ID #REQUIRED>").is_empty());
    assert_eq!(errors("<!ELEMENT r ANY><!ATTLIST r a ID #FIXED 'x'>"), [Code::VcIdAttributeDefault]);
    assert_eq!(errors("<!ELEMENT r ANY><!ATTLIST r a ID 'x'>"), [Code::VcIdAttributeDefault]);
    assert_eq!(errors("<!ELEMENT r ANY><!ATTLIST r a ID #IMPLIED b ID #IMPLIED>"), [Code::VcOneIdPerElement]);
    // In two declarations too, and the same name twice is one attribute.
    assert_eq!(errors("<!ELEMENT r ANY><!ATTLIST r a ID #IMPLIED><!ATTLIST r b ID #IMPLIED>"), [Code::VcOneIdPerElement]);
    assert!(errors("<!ELEMENT r ANY><!ATTLIST r a ID #IMPLIED><!ATTLIST r a ID #IMPLIED>").is_empty());
}

#[test]
fn notation_attributes() {
    let base = "<!NOTATION n SYSTEM 'n'><!NOTATION m SYSTEM 'm'>";
    assert!(errors(&format!("{base}<!ELEMENT r ANY><!ATTLIST r a NOTATION (n | m) #IMPLIED>")).is_empty());
    assert_eq!(
        errors(&format!("{base}<!ELEMENT r ANY><!ATTLIST r a NOTATION (n | m) #IMPLIED b NOTATION (n) #IMPLIED>")),
        [Code::VcOneNotationPerElement]
    );
    assert_eq!(errors(&format!("{base}<!ELEMENT r EMPTY><!ATTLIST r a NOTATION (n) #IMPLIED>")), [Code::VcNotationOnEmpty]);
    assert_eq!(errors(&format!("{base}<!ELEMENT r ANY><!ATTLIST r a NOTATION (n | zzz) #IMPLIED>")), [Code::VcNotationNotDeclared]);
    assert_eq!(errors(&format!("{base}<!ELEMENT r ANY><!ATTLIST r a NOTATION (n | n) #IMPLIED>")), [Code::VcDuplicateToken]);
    assert_eq!(errors("<!ELEMENT r ANY><!ATTLIST r a (x | y | x) #IMPLIED>"), [Code::VcDuplicateToken]);
    // An unparsed entity needs its notation.
    assert_eq!(errors("<!ENTITY img SYSTEM 'i.gif' NDATA gif>"), [Code::VcNotationNotDeclared]);
    assert!(errors("<!NOTATION gif SYSTEM 'g'><!ENTITY img SYSTEM 'i.gif' NDATA gif>").is_empty());
}

#[test]
fn what_may_be_declared_elsewhere_is_not_missing() {
    // With an external part that was not read, a notation may be declared there.
    let parsed = xmlcraft_dtd::parse_doctype(
        &xmlcraft_dtd::DoctypeInput {
            external: Some(xmlcraft_dtd::ExternalRef { public: None, system: Some("r.dtd") }),
            internal: Some(("<!ENTITY img SYSTEM 'i.gif' NDATA gif><!ELEMENT r ANY><!ATTLIST r a NOTATION (zzz) #IMPLIED>", 0)),
            location: "",
        },
        &mut xmlcraft_dtd::NoLoader,
    );
    let compiled = compile(&parsed.dtd, Some("r"));
    assert!(compiled.grammar.incomplete);
    assert!(compiled.diagnostics.is_empty(), "{:?}", compiled.diagnostics);
}

#[test]
fn default_values_must_suit_their_types() {
    let wrong = |attlist: &str| errors(&format!("<!ELEMENT r ANY><!ENTITY e 'a'>{attlist}"));
    // Legal.
    for ok in [
        "<!ATTLIST r x CDATA 'anything at all'>",
        "<!ATTLIST r x NMTOKEN ' a1 '>",
        "<!ATTLIST r x NMTOKENS 'a  b\tc'>",
        "<!ATTLIST r x (a | b) 'a'>",
        "<!ATTLIST r x (a | b) '&e;'>",
        "<!ATTLIST r x IDREF #FIXED 'n1'>",
        "<!ATTLIST r x IDREFS 'n1 n2'>",
        "<!ATTLIST r x ENTITY 'anything'>",
        "<!ATTLIST r x ENTITIES 'a b'>",
        "<!ATTLIST r x CDATA '&#60; &lt; &amp;'>",
    ] {
        assert_eq!(wrong(ok), Vec::<Code>::new(), "{ok}");
    }
    // Illegal.
    for bad in [
        "<!ATTLIST r x NMTOKEN 'a b'>",
        "<!ATTLIST r x NMTOKEN ''>",
        "<!ATTLIST r x NMTOKENS ''>",
        "<!ATTLIST r x (a | b) 'c'>",
        "<!ATTLIST r x (a | b) 'a b'>",
        "<!ATTLIST r x IDREF '1x'>",
        "<!ATTLIST r x IDREFS 'n1 2'>",
        "<!ATTLIST r x ENTITY 'a b'>",
        "<!ATTLIST r x CDATA '&nope;'>",
        "<!ATTLIST r x (a | b) '&nope;'>",
    ] {
        assert_eq!(wrong(bad), [Code::VcAttributeDefault], "{bad}");
    }
}

#[test]
fn values_are_read_with_their_references_replaced_and_their_white_space_normalised() {
    let c = grammar_of(
        "<!ENTITY a 'one'><!ENTITY two 'a &a; b'><!ENTITY ws '  x\ty  '><!ENTITY lt2 '&#38;#60;'><!ENTITY ext SYSTEM 'e.xml'><!ENTITY loop '&loop;'><!ENTITY p 'a&#10;b'>",
    );
    let reader = ValueReader::new(&c.grammar);
    let read = |literal: &str, ty: AttributeType| reader.read(literal, &ty);
    assert_eq!(read("a  b\n\tc\r\nd\re", AttributeType::Cdata), Ok("a  b  c d e".to_owned()), "each white space character is a space");
    assert_eq!(read("  a  b \n c ", AttributeType::Nmtokens), Ok("a b c".to_owned()), "trimmed and collapsed unless CDATA");
    assert_eq!(read("&a;&two;", AttributeType::Cdata), Ok("onea one b".to_owned()));
    assert_eq!(read("&ws;", AttributeType::Cdata), Ok("  x y  ".to_owned()));
    assert_eq!(read("&ws;", AttributeType::Nmtoken), Ok("x y".to_owned()));
    // Predefined entities and character references: the character, taken as it is (not turned into a space by this rule, for CDATA).
    assert_eq!(read("&lt;&gt;&amp;&quot;&apos;", AttributeType::Cdata), Ok("<>&\"'".to_owned()));
    assert_eq!(read("a&#32;&#x20;b&#10;c", AttributeType::Cdata), Ok("a  b\nc".to_owned()));
    // A reference in the replacement text of an entity is a reference too (`&#38;#60;` is `&#60;`, which is `<`).
    assert_eq!(read("&lt2;", AttributeType::Cdata), Ok("<".to_owned()));
    // A character reference in the declaration of an entity gives a character in its replacement text, which is then read like the rest.
    assert_eq!(read("&p;", AttributeType::Cdata), Ok("a b".to_owned()));
    // Problems.
    assert_eq!(read("&nope;", AttributeType::Cdata), Err(ValueProblem::Undeclared("nope".into())));
    assert_eq!(read("x &ext;", AttributeType::Cdata), Err(ValueProblem::NotInternal("ext".into())));
    assert_eq!(read("&loop;", AttributeType::Cdata), Err(ValueProblem::Recursive("loop".into())));
    assert!(matches!(read("&#0;", AttributeType::Cdata), Err(ValueProblem::BadReference(_))));
    assert!(matches!(read("&#xD800;", AttributeType::Cdata), Err(ValueProblem::BadReference(_))));
    assert!(matches!(read("a & b", AttributeType::Cdata), Err(ValueProblem::BadReference(_))));
    assert!(matches!(read("&unterminated", AttributeType::Cdata), Err(ValueProblem::BadReference(_))));
}

#[test]
fn entities_that_multiply_are_cut_off() {
    let mut subset = String::from("<!ENTITY l0 'haha'>");
    for level in 1..=12 {
        subset.push_str(&format!("<!ENTITY l{level} '{}'>", format!("&l{};", level - 1).repeat(10)));
    }
    let c = grammar_of(&subset);
    let reader = ValueReader::new(&c.grammar);
    let started = std::time::Instant::now();
    assert_eq!(reader.read("&l12;", &AttributeType::Cdata), Err(ValueProblem::TooLong));
    assert_eq!(reader.read("&l3;", &AttributeType::Cdata).map(|v| v.len()), Ok(4000));
    // A chain that is too deep.
    let mut subset = String::from("<!ENTITY e0 'x'>");
    for level in 1..=100 {
        subset.push_str(&format!("<!ENTITY e{level} '&e{};'>", level - 1));
    }
    let c = grammar_of(&subset);
    let reader = ValueReader::new(&c.grammar);
    assert_eq!(reader.read("&e100;", &AttributeType::Cdata), Err(ValueProblem::TooLong));
    assert_eq!(reader.read("&e20;", &AttributeType::Cdata), Ok("x".to_owned()));
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "took {:?}", started.elapsed());
}

#[test]
fn legal_values_of_each_type() {
    use AttributeType::*;
    let list = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    assert!(check_value(&Cdata, "").is_ok() && check_value(&Cdata, "anything <>").is_ok());
    for (ty, ok, bad) in [
        (Id, "a1", "1a"),
        (Idref, "_x:y", "a b"),
        (Entity, "é", ""),
        (Nmtoken, "1a-b.c", "a b"),
        (Idrefs, "a b c", "a 1"),
        (Entities, "a", ""),
        (Nmtokens, "1 22 a", ""),
        (Enumeration(list(&["x", "y"])), "y", "z"),
        (Notation(list(&["n"])), "n", "m"),
    ] {
        assert!(check_value(&ty, ok).is_ok(), "{ty:?} {ok}");
        assert!(check_value(&ty, bad).is_err(), "{ty:?} {bad}");
    }
}
