//! The content models, compiled: what a type allows as children, substitution groups, `xs:all`, and the constraints that are about a
//! whole content model.

use xmlcraft_grammar::Name;
use xmlcraft_xsd::model::{ProcessContents, QName};
use xmlcraft_xsd::schema::Schema;
use xmlcraft_xsd::{Assembly, ContentModel, NoLoader, Symbol, XSD_NAMESPACE, assemble, load};

const NS: &str = "urn:t";

fn build(body: &str) -> Assembly {
    let text = format!("<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" xmlns:t=\"{NS}\" targetNamespace=\"{NS}\">{body}</xs:schema>");
    let documents = load(&text, "/s/root.xsd", &mut NoLoader);
    assert!(documents.diagnostics.is_empty(), "{:?}", documents.diagnostics.iter().map(|d| d.diagnostic.message()).collect::<Vec<_>>());
    assemble(&documents)
}

fn keys(a: &Assembly) -> Vec<&'static str> {
    a.diagnostics.iter().map(|d| d.diagnostic.code.key()).collect()
}

fn messages(a: &Assembly) -> Vec<String> {
    a.diagnostics.iter().map(|d| d.diagnostic.message()).collect()
}

fn model<'a>(schema: &'a Schema, local: &str) -> &'a ContentModel {
    let id = schema.type_by_name(&QName::new(Some(NS), local)).unwrap_or_else(|| panic!("no type {local}"));
    schema.content_model(id).unwrap_or_else(|| panic!("{local} has no content model"))
}

/// Read the children (local names without a namespace; `x:name` is in the namespace `urn:x`) and say whether the content is valid.
fn reads(m: &ContentModel, children: &[&str]) -> bool {
    let mut state = m.start();
    for child in children {
        let name = match child.split_once(':') {
            Some((_, local)) => Name::new(Some("urn:x"), local),
            None => Name::local(child),
        };
        match m.step(&state, &name) {
            Some((next, _)) => state = next,
            None => return false,
        }
    }
    m.is_accepting(&state)
}

#[test]
fn sequences_choices_and_counts() {
    let a = build(
        "<xs:complexType name=\"t\"><xs:sequence>\
           <xs:element name=\"a\"/><xs:element name=\"b\" minOccurs=\"1\" maxOccurs=\"2\"/>\
           <xs:choice minOccurs=\"0\" maxOccurs=\"unbounded\"><xs:element name=\"c\"/><xs:element name=\"d\"/></xs:choice>\
         </xs:sequence></xs:complexType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let m = model(&a.schema, "t");
    assert!(reads(m, &["a", "b"]) && reads(m, &["a", "b", "b"]) && reads(m, &["a", "b", "c", "d", "c"]));
    assert!(!reads(m, &["a"]) && !reads(m, &["a", "b", "b", "b"]) && !reads(m, &["b"]) && !reads(m, &["a", "b", "e"]) && !reads(m, &[]));
}

#[test]
fn the_step_says_which_element_took_a_child() {
    let a = build(
        "<xs:element name=\"item\" type=\"xs:int\"/><xs:complexType name=\"t\"><xs:sequence><xs:element ref=\"t:item\"/><xs:element ref=\"t:item\"/></xs:sequence></xs:complexType>",
    );
    let m = model(&a.schema, "t");
    assert_eq!(m.symbols().len(), 1, "one symbol for one element, however often it comes");
    let (_, symbol) = m.step(&m.start(), &Name::new(Some(NS), "item")).expect("a step");
    assert_eq!(symbol, &Symbol::Element(a.schema.element_by_name(&QName::new(Some(NS), "item")).expect("item")));
    assert!(m.step(&m.start(), &Name::local("item")).is_none(), "the global element is in the namespace of the schema");
}

#[test]
fn a_wildcard_lets_in_the_elements_of_its_namespaces() {
    let a = build(
        "<xs:complexType name=\"t\"><xs:sequence><xs:element name=\"a\"/><xs:any namespace=\"##other\" processContents=\"skip\" minOccurs=\"0\" maxOccurs=\"unbounded\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"local\"><xs:sequence><xs:any namespace=\"##local urn:x\" processContents=\"lax\"/></xs:sequence></xs:complexType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let m = model(&a.schema, "t");
    assert!(reads(m, &["a"]) && reads(m, &["a", "x:z", "x:y"]));
    assert!(!reads(m, &["a", "z"]), "no namespace is not 'other'");
    let (_, symbol) = m.step(&m.step(&m.start(), &Name::local("a")).expect("a").0, &Name::new(Some("urn:x"), "z")).expect("a wildcard step");
    assert!(matches!(symbol, Symbol::Wildcard(w) if w.process_contents == ProcessContents::Skip));
    let local = model(&a.schema, "local");
    assert!(reads(local, &["q"]) && reads(local, &["x:q"]));
    assert!(!reads(local, &[]) && !reads(local, &["q", "q"]));
}

#[test]
fn an_extension_reads_the_content_of_its_base_first() {
    let a = build(
        "<xs:complexType name=\"base\"><xs:sequence><xs:element name=\"a\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"ext\"><xs:complexContent><xs:extension base=\"t:base\"><xs:sequence><xs:element name=\"b\" minOccurs=\"0\"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let m = model(&a.schema, "ext");
    assert!(reads(m, &["a"]) && reads(m, &["a", "b"]));
    assert!(!reads(m, &["b"]) && !reads(m, &["a", "b", "a"]));
}

#[test]
fn only_types_with_elements_in_them_have_a_model() {
    let a = build(
        "<xs:complexType name=\"empty\"/><xs:complexType name=\"text\"><xs:simpleContent><xs:extension base=\"xs:string\"/></xs:simpleContent></xs:complexType>\
         <xs:complexType name=\"onlytext\" mixed=\"true\"/><xs:complexType name=\"someelements\" mixed=\"true\"><xs:sequence><xs:element name=\"i\" minOccurs=\"0\"/></xs:sequence></xs:complexType>\
         <xs:simpleType name=\"simple\"><xs:restriction base=\"xs:string\"/></xs:simpleType>",
    );
    let id = |l: &str| a.schema.type_by_name(&QName::new(Some(NS), l)).expect("a type");
    assert!(
        a.schema.content_model(id("empty")).is_none()
            && a.schema.content_model(id("text")).is_none()
            && a.schema.content_model(id("simple")).is_none()
    );
    assert!(reads(model(&a.schema, "onlytext"), &[]) && !reads(model(&a.schema, "onlytext"), &["x"]), "mixed content with no elements allows none");
    assert!(!reads(model(&a.schema, "someelements"), &["i", "i"]) && reads(model(&a.schema, "someelements"), &["i"]));
}

#[test]
fn what_may_come_next_and_what_must() {
    let a = build(
        "<xs:complexType name=\"t\"><xs:sequence><xs:element name=\"a\"/><xs:element name=\"b\"/><xs:element name=\"c\" minOccurs=\"0\"/></xs:sequence></xs:complexType>",
    );
    let m = model(&a.schema, "t");
    let after_a = m.step(&m.start(), &Name::local("a")).expect("a").0;
    let next = m.expected(&after_a);
    assert_eq!(next.len(), 1);
    assert!(next[0].mandatory);
    assert!(matches!(next[0].symbol, Symbol::Element(id) if a.schema.element(*id).expect("b").name.local == "b"));
    let after_b = m.step(&after_a, &Name::local("b")).expect("b").0;
    let next = m.expected(&after_b);
    assert_eq!(next.len(), 1);
    assert!(!next[0].mandatory, "c may be left out");
}

#[test]
fn a_substitution_group_may_stand_in_for_its_head() {
    let a = build(
        "<xs:element name=\"head\" type=\"t:base\"/>\
         <xs:element name=\"m1\" type=\"t:base\" substitutionGroup=\"t:head\"/>\
         <xs:element name=\"m2\" type=\"t:base\" substitutionGroup=\"t:m1\"/>\
         <xs:element name=\"abs\" type=\"t:base\" abstract=\"true\"/>\
         <xs:element name=\"am\" type=\"t:base\" substitutionGroup=\"t:abs\"/>\
         <xs:complexType name=\"base\"/>\
         <xs:complexType name=\"t\"><xs:sequence><xs:element ref=\"t:head\"/><xs:element ref=\"t:abs\"/></xs:sequence></xs:complexType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let m = model(&a.schema, "t");
    let ns = |l: &str| Name::new(Some(NS), l);
    let reads_ns = |names: &[&str]| {
        let mut state = m.start();
        for n in names {
            match m.step(&state, &ns(n)) {
                Some((next, _)) => state = next,
                None => return false,
            }
        }
        m.is_accepting(&state)
    };
    assert!(reads_ns(&["head", "am"]) && reads_ns(&["m1", "am"]) && reads_ns(&["m2", "am"]), "a member, and a member of a member");
    assert!(!reads_ns(&["head", "abs"]), "an abstract head cannot be used itself");
    assert!(!reads_ns(&["head", "head"]));
}

#[test]
fn what_a_head_blocks_is_not_a_substitute() {
    let a = build(
        "<xs:complexType name=\"base\"><xs:sequence><xs:element name=\"x\" minOccurs=\"0\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"ext\"><xs:complexContent><xs:extension base=\"t:base\"><xs:sequence><xs:element name=\"y\" minOccurs=\"0\"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>\
         <xs:complexType name=\"restr\"><xs:complexContent><xs:restriction base=\"t:base\"/></xs:complexContent></xs:complexType>\
         <xs:complexType name=\"guarded\" block=\"extension\"><xs:sequence><xs:element name=\"x\" minOccurs=\"0\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"gext\"><xs:complexContent><xs:extension base=\"t:guarded\"/></xs:complexContent></xs:complexType>\
         <xs:element name=\"free\" type=\"t:base\"/><xs:element name=\"fe\" type=\"t:ext\" substitutionGroup=\"t:free\"/><xs:element name=\"fr\" type=\"t:restr\" substitutionGroup=\"t:free\"/>\
         <xs:element name=\"noext\" type=\"t:base\" block=\"extension\"/><xs:element name=\"ne\" type=\"t:ext\" substitutionGroup=\"t:noext\"/><xs:element name=\"nr\" type=\"t:restr\" substitutionGroup=\"t:noext\"/>\
         <xs:element name=\"norestr\" type=\"t:base\" block=\"restriction\"/><xs:element name=\"rr\" type=\"t:restr\" substitutionGroup=\"t:norestr\"/>\
         <xs:element name=\"nosub\" type=\"t:base\" block=\"substitution\"/><xs:element name=\"ns\" type=\"t:base\" substitutionGroup=\"t:nosub\"/>\
         <xs:element name=\"byType\" type=\"t:guarded\"/><xs:element name=\"bg\" type=\"t:gext\" substitutionGroup=\"t:byType\"/>\
         <xs:complexType name=\"guardedR\" block=\"restriction\"><xs:sequence><xs:element name=\"x\" minOccurs=\"0\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"grestr\"><xs:complexContent><xs:restriction base=\"t:guardedR\"/></xs:complexContent></xs:complexType>\
         <xs:element name=\"byTypeR\" type=\"t:guardedR\"/><xs:element name=\"br\" type=\"t:grestr\" substitutionGroup=\"t:byTypeR\"/>\
         <xs:element name=\"abm\" type=\"t:base\" abstract=\"true\" substitutionGroup=\"t:free\"/>\
         <xs:complexType name=\"uses\"><xs:choice><xs:element ref=\"t:free\"/><xs:element ref=\"t:noext\"/><xs:element ref=\"t:norestr\"/><xs:element ref=\"t:nosub\"/><xs:element ref=\"t:byType\"/><xs:element ref=\"t:byTypeR\"/></xs:choice></xs:complexType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let m = model(&a.schema, "uses");
    let takes = |name: &str| m.step(&m.start(), &Name::new(Some(NS), name)).is_some();
    assert!(takes("free") && takes("fe") && takes("fr"));
    assert!(takes("noext") && !takes("ne") && takes("nr"), "the head blocks extension");
    assert!(takes("norestr") && !takes("rr"), "the head blocks restriction");
    assert!(takes("nosub") && !takes("ns"), "the head blocks substitution");
    assert!(takes("byType") && !takes("bg"), "the type of the head blocks extension");
    assert!(takes("byTypeR") && !takes("br"), "the type of the head blocks restriction");
    assert!(!takes("abm"), "an abstract member cannot be used itself");
}

#[test]
fn an_all_group_takes_its_elements_in_any_order() {
    let a = build(
        "<xs:element name=\"head\" type=\"xs:string\"/><xs:element name=\"alt\" type=\"xs:string\" substitutionGroup=\"t:head\"/>\
         <xs:complexType name=\"t\"><xs:all><xs:element name=\"p\"/><xs:element name=\"q\" minOccurs=\"0\"/><xs:element name=\"r\"/></xs:all></xs:complexType>\
         <xs:complexType name=\"maybe\"><xs:all minOccurs=\"0\"><xs:element name=\"p\"/><xs:element name=\"never\" minOccurs=\"0\" maxOccurs=\"0\"/></xs:all></xs:complexType>\
         <xs:complexType name=\"subst\"><xs:all><xs:element ref=\"t:head\"/><xs:element name=\"z\"/></xs:all></xs:complexType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let m = model(&a.schema, "t");
    assert!(reads(m, &["p", "r"]) && reads(m, &["r", "p"]) && reads(m, &["q", "r", "p"]) && reads(m, &["r", "q", "p"]));
    assert!(!reads(m, &["p"]) && !reads(m, &["p", "r", "r"]) && !reads(m, &["p", "r", "x"]) && !reads(m, &[]) && !reads(m, &["q", "p"]));
    let maybe = model(&a.schema, "maybe");
    assert!(reads(maybe, &[]) && reads(maybe, &["p"]) && !reads(maybe, &["p", "p"]) && !reads(maybe, &["never"]) && !reads(maybe, &["p", "never"]));
    // The group may be left out, so what is in it is not mandatory until it is begun.
    let at_start = maybe.expected(&maybe.start());
    assert_eq!(at_start.len(), 1);
    assert!(!at_start[0].mandatory);
    let subst = model(&a.schema, "subst");
    let ns = |l: &str| Name::new(Some(NS), l);
    let step = |s, n: &str| subst.step(&s, &ns(n)).map(|(s, _)| s);
    let s = step(subst.start(), "alt").expect("a substitute takes the slot of its head");
    assert!(step(s.clone(), "head").is_none(), "the slot is taken");
    assert!(!subst.is_accepting(&s), "z is still to come");
    let s = subst.step(&s, &Name::local("z")).expect("z").0;
    assert!(subst.is_accepting(&s));
    // What may come next: the slots not taken, and the required ones are mandatory.
    let m = model(&a.schema, "t");
    let after_p = m.step(&m.start(), &Name::local("p")).expect("p").0;
    let next: Vec<(String, bool)> = m
        .expected(&after_p)
        .into_iter()
        .map(|n| match n.symbol {
            Symbol::Element(id) => (a.schema.element(*id).expect("an element").name.local.clone(), n.mandatory),
            Symbol::Wildcard(_) => panic!("no wildcards here"),
        })
        .collect();
    assert_eq!(next, [("q".to_owned(), false), ("r".to_owned(), true)]);
}

#[test]
fn a_content_model_in_which_an_element_could_be_taken_twice_is_ambiguous() {
    for (what, body) in [
        ("optional first", "<xs:sequence><xs:element name=\"a\" minOccurs=\"0\"/><xs:element name=\"a\"/></xs:sequence>"),
        (
            "two branches",
            "<xs:choice><xs:element name=\"a\"/><xs:sequence><xs:element name=\"a\"/><xs:element name=\"b\"/></xs:sequence></xs:choice>",
        ),
        ("wildcard first", "<xs:sequence><xs:any minOccurs=\"0\"/><xs:element name=\"a\"/></xs:sequence>"),
    ] {
        let a = build(&format!("<xs:complexType name=\"t\">{body}</xs:complexType>"));
        assert_eq!(keys(&a), ["xsd.unique-particle-attribution"], "{what}: {:?}", messages(&a));
    }
    // Models in which every element has its place.
    let a = build(
        "<xs:complexType name=\"t\"><xs:sequence><xs:element name=\"a\"/><xs:any namespace=\"##other\" minOccurs=\"0\"/><xs:element name=\"a\" minOccurs=\"0\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"u\"><xs:choice><xs:element name=\"a\"/><xs:element name=\"b\"/></xs:choice></xs:complexType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
}

#[test]
fn elements_of_one_name_in_a_model_must_have_one_type() {
    let a = build(
        "<xs:complexType name=\"t\"><xs:sequence><xs:element name=\"a\" type=\"xs:int\"/><xs:element name=\"b\"/><xs:element name=\"a\" type=\"xs:string\"/></xs:sequence></xs:complexType>",
    );
    assert_eq!(keys(&a), ["xsd.component"], "{:?}", messages(&a));
    // Three types for one name is one mistake.
    let a = build(
        "<xs:complexType name=\"t\"><xs:sequence><xs:element name=\"a\" type=\"xs:int\"/><xs:element name=\"a\" type=\"xs:string\"/><xs:element name=\"a\" type=\"xs:boolean\"/></xs:sequence></xs:complexType>",
    );
    assert_eq!(keys(&a), ["xsd.component"], "{:?}", messages(&a));
    let a = build(
        "<xs:complexType name=\"t\"><xs:sequence><xs:element name=\"a\" type=\"xs:int\"/><xs:element name=\"b\"/><xs:element name=\"a\" type=\"xs:int\"/></xs:sequence></xs:complexType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    // A substitute counts: `a` here is a member of the group of `head`, and the local `a` has another type.
    let a = build(
        "<xs:element name=\"head\" type=\"xs:string\"/>\
         <xs:complexType name=\"t\"><xs:sequence><xs:element ref=\"t:head\"/><xs:element name=\"head\" type=\"xs:int\" form=\"qualified\"/></xs:sequence></xs:complexType>",
    );
    assert_eq!(keys(&a), ["xsd.component"], "{:?}", messages(&a));
}

#[test]
fn a_model_that_is_too_large_is_refused_not_unrolled() {
    let a = build(
        "<xs:complexType name=\"t\"><xs:sequence maxOccurs=\"100000\"><xs:sequence maxOccurs=\"100000\"><xs:element name=\"a\"/><xs:element name=\"b\"/></xs:sequence></xs:sequence></xs:complexType>\
         <xs:complexType name=\"fine\"><xs:sequence><xs:element name=\"a\" maxOccurs=\"1000\"/></xs:sequence></xs:complexType>",
    );
    assert_eq!(keys(&a), ["xsd.limit"], "{:?}", messages(&a));
    let id = a.schema.type_by_name(&QName::new(Some(NS), "t")).expect("t");
    assert!(a.schema.content_model(id).is_none());
    assert!(reads(model(&a.schema, "fine"), &["a"; 1000]) && !reads(model(&a.schema, "fine"), &["a"; 1001]));
}

#[test]
fn an_all_group_is_only_the_whole_content_in_xml_schema_1_0() {
    let a = build(
        "<xs:complexType name=\"base\"><xs:all><xs:element name=\"p\"/></xs:all></xs:complexType>\
         <xs:complexType name=\"ext\"><xs:complexContent><xs:extension base=\"t:base\"><xs:sequence><xs:element name=\"q\"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>",
    );
    // The mistake is reported once, for the extension; the model is still made, with the group read in order.
    assert_eq!(keys(&a), ["xsd.component"], "{:?}", messages(&a));
    assert!(messages(&a)[0].contains("ext"), "{:?}", messages(&a));
    assert!(reads(model(&a.schema, "ext"), &["p", "q"]));
}
