//! The components of a schema: complex types and their derivation, groups, elements, attributes and their values, substitution groups
//! and identity constraints; what is wrong with them is reported.

use xmlcraft_xsd::model::{DerivationMethod, QName};
use xmlcraft_xsd::schema::{ComplexType, ContentType, NamespaceSet, Particle, Schema, Term, TypeKind};
use xmlcraft_xsd::{Assembly, LoadRequest, Loaded, NoLoader, SchemaLoader, XSD_NAMESPACE, assemble, load};

const NS: &str = "urn:t";

fn schema_with(attributes: &str, body: &str) -> String {
    format!("<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" xmlns:t=\"{NS}\" targetNamespace=\"{NS}\" {attributes}>{body}</xs:schema>")
}

fn build_with(attributes: &str, body: &str) -> Assembly {
    let documents = load(&schema_with(attributes, body), "/s/root.xsd", &mut NoLoader);
    // The mistakes of reading are not what these tests are about.
    assert!(documents.diagnostics.is_empty(), "{:?}", documents.diagnostics.iter().map(|d| d.diagnostic.message()).collect::<Vec<_>>());
    assemble(&documents)
}

fn build(body: &str) -> Assembly {
    build_with("", body)
}

fn keys(a: &Assembly) -> Vec<&'static str> {
    a.diagnostics.iter().map(|d| d.diagnostic.code.key()).collect()
}

fn messages(a: &Assembly) -> Vec<String> {
    a.diagnostics.iter().map(|d| d.diagnostic.message()).collect()
}

fn complex<'a>(schema: &'a Schema, local: &str) -> &'a ComplexType {
    let id = schema.type_by_name(&QName::new(Some(NS), local)).unwrap_or_else(|| panic!("no type {local}"));
    match &schema.type_entry(id).expect("an entry").kind {
        TypeKind::Complex(c) => c,
        TypeKind::Simple(_) => panic!("{local} is simple"),
    }
}

/// The names of the attributes a type uses, in order, with a star on the required ones.
fn attributes(schema: &Schema, c: &ComplexType) -> Vec<String> {
    c.attributes
        .iter()
        .map(|u| {
            let name = &schema.attributes[u.attribute.0].name.local;
            if u.required { format!("{name}*") } else { name.clone() }
        })
        .collect()
}

fn occurs(p: &Particle) -> String {
    match (p.min, p.max) {
        (1, Some(1)) => String::new(),
        (min, Some(max)) => format!("{{{min},{max}}}"),
        (min, None) => format!("{{{min},}}"),
    }
}

/// A content model as text: `(a,b)` a sequence, `(a|b)` a choice, `(a&b)` an `all` group, `*` a wildcard.
fn shape(schema: &Schema, p: &Particle) -> String {
    let list = |children: &[Particle], sep: &str| format!("({})", children.iter().map(|c| shape(schema, c)).collect::<Vec<_>>().join(sep));
    let term = match &p.term {
        Term::Element(id) => schema.element(*id).expect("an element").name.local.clone(),
        Term::Wildcard(_) => "*".to_owned(),
        Term::Sequence(c) => list(c, ","),
        Term::Choice(c) => list(c, "|"),
        Term::All(c) => list(c, "&"),
    };
    format!("{term}{}", occurs(p))
}

fn content_shape(schema: &Schema, c: &ComplexType) -> String {
    match &c.content {
        ContentType::Empty => "empty".to_owned(),
        ContentType::Simple(_) => "simple".to_owned(),
        ContentType::ElementOnly(content) => shape(schema, &content.particle),
        ContentType::Mixed(content) => format!("mixed {}", shape(schema, &content.particle)),
    }
}

#[test]
fn a_type_has_the_content_its_particles_describe() {
    let a = build(
        "<xs:complexType name=\"plain\"><xs:sequence><xs:element name=\"a\" type=\"xs:string\"/>\
           <xs:choice minOccurs=\"0\" maxOccurs=\"unbounded\"><xs:element name=\"b\" type=\"xs:int\"/><xs:any namespace=\"##other\" processContents=\"lax\"/></xs:choice>\
         </xs:sequence><xs:attribute name=\"id\" type=\"xs:ID\" use=\"required\"/><xs:attribute name=\"n\" type=\"xs:int\"/></xs:complexType>\
         <xs:complexType name=\"text\" mixed=\"true\"><xs:sequence><xs:element name=\"i\" type=\"xs:string\" minOccurs=\"0\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"nothing\"/>\
         <xs:complexType name=\"onlytext\" mixed=\"true\"/>\
         <xs:complexType name=\"emptyseq\"><xs:sequence/></xs:complexType>\
         <xs:complexType name=\"never\"><xs:sequence><xs:element name=\"z\" maxOccurs=\"0\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"unordered\"><xs:all><xs:element name=\"p\"/><xs:element name=\"q\" minOccurs=\"0\"/></xs:all></xs:complexType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let plain = complex(&a.schema, "plain");
    assert_eq!(content_shape(&a.schema, plain), "(a,(b|*){0,})");
    assert_eq!(attributes(&a.schema, plain), ["id*", "n"]);
    assert_eq!(content_shape(&a.schema, complex(&a.schema, "text")), "mixed (i{0,1})");
    assert_eq!(content_shape(&a.schema, complex(&a.schema, "nothing")), "empty");
    assert_eq!(content_shape(&a.schema, complex(&a.schema, "onlytext")), "mixed ()");
    assert_eq!(content_shape(&a.schema, complex(&a.schema, "emptyseq")), "empty");
    assert_eq!(content_shape(&a.schema, complex(&a.schema, "never")), "empty", "an element that may occur no times is no content");
    assert_eq!(content_shape(&a.schema, complex(&a.schema, "unordered")), "(p&q{0,1})");
    // The wildcard `##other` is in the namespaces but the schema's own.
    let Term::Choice(choice) = (match &plain.content {
        ContentType::ElementOnly(c) => match &c.particle.term {
            Term::Sequence(items) => items.get(1).map(|p| p.term.clone()).expect("the choice"),
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }) else {
        panic!("not a choice")
    };
    let Term::Wildcard(w) = &choice[1].term else { panic!("not a wildcard") };
    assert_eq!(w.constraint, NamespaceSet::Not(Some(NS.to_owned())));
    // The type is derived from anyType, by restriction.
    let id = a.schema.type_by_name(&QName::new(Some(NS), "plain")).expect("plain");
    let entry = a.schema.type_entry(id).expect("an entry");
    assert_eq!(entry.base, a.schema.type_by_name(&QName::new(Some(XSD_NAMESPACE), "anyType")));
    assert_eq!(entry.derivation, Some(DerivationMethod::Restriction));
}

#[test]
fn an_extension_adds_to_its_base_in_any_order_of_definition() {
    let a = build(
        "<xs:complexType name=\"ext\"><xs:complexContent><xs:extension base=\"t:base\">\
           <xs:sequence><xs:element name=\"b\"/></xs:sequence><xs:attribute name=\"y\"/></xs:extension></xs:complexContent></xs:complexType>\
         <xs:complexType name=\"base\"><xs:sequence><xs:element name=\"a\"/></xs:sequence><xs:attribute name=\"x\" use=\"required\"/><xs:anyAttribute namespace=\"urn:o\"/></xs:complexType>\
         <xs:complexType name=\"ext2\"><xs:complexContent><xs:extension base=\"t:ext\"><xs:anyAttribute namespace=\"##local\"/></xs:extension></xs:complexContent></xs:complexType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let ext = complex(&a.schema, "ext");
    assert_eq!(content_shape(&a.schema, ext), "((a),(b))");
    assert_eq!(attributes(&a.schema, ext), ["x*", "y"], "the attributes of the base come first");
    let base = a.schema.type_by_name(&QName::new(Some(NS), "base"));
    let id = a.schema.type_by_name(&QName::new(Some(NS), "ext")).expect("ext");
    assert_eq!(a.schema.type_entry(id).and_then(|t| t.base), base);
    assert_eq!(a.schema.type_entry(id).and_then(|t| t.derivation), Some(DerivationMethod::Extension));
    assert_eq!(ext.attribute_wildcard.as_ref().map(|w| &w.constraint), Some(&NamespaceSet::List(vec![Some("urn:o".to_owned())])), "inherited");
    // A wildcard added by an extension widens the one of the base.
    let ext2 = complex(&a.schema, "ext2");
    assert_eq!(content_shape(&a.schema, ext2), "((a),(b))", "no content of its own: the content of the base");
    assert_eq!(ext2.attribute_wildcard.as_ref().map(|w| &w.constraint), Some(&NamespaceSet::List(vec![None, Some("urn:o".to_owned())])));
}

#[test]
fn an_extension_of_an_empty_or_text_type() {
    let a = build(
        "<xs:complexType name=\"e\"/>\
         <xs:complexType name=\"e1\"><xs:complexContent><xs:extension base=\"t:e\"><xs:sequence><xs:element name=\"a\"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>\
         <xs:complexType name=\"e2\"><xs:complexContent><xs:extension base=\"t:e\"><xs:attribute name=\"k\"/></xs:extension></xs:complexContent></xs:complexType>\
         <xs:complexType name=\"m\" mixed=\"true\"><xs:sequence><xs:element name=\"i\" minOccurs=\"0\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"m1\"><xs:complexContent mixed=\"true\"><xs:extension base=\"t:m\"><xs:sequence><xs:element name=\"j\"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    assert_eq!(content_shape(&a.schema, complex(&a.schema, "e1")), "(a)");
    assert_eq!(content_shape(&a.schema, complex(&a.schema, "e2")), "empty");
    assert_eq!(content_shape(&a.schema, complex(&a.schema, "m1")), "mixed ((i{0,1}),(j))");
}

#[test]
fn a_restriction_keeps_the_attributes_it_does_not_change_and_not_the_wildcard() {
    let a = build(
        "<xs:complexType name=\"base\"><xs:sequence><xs:element name=\"a\"/><xs:element name=\"b\" minOccurs=\"0\"/></xs:sequence>\
           <xs:attribute name=\"x\"/><xs:attribute name=\"y\"/><xs:attribute name=\"z\" use=\"required\"/><xs:attribute name=\"w\"/><xs:anyAttribute/></xs:complexType>\
         <xs:complexType name=\"restr\"><xs:complexContent><xs:restriction base=\"t:base\"><xs:sequence><xs:element name=\"a\"/></xs:sequence>\
           <xs:attribute name=\"x\" use=\"prohibited\"/><xs:attribute name=\"y\" type=\"xs:int\" use=\"required\"/></xs:restriction></xs:complexContent></xs:complexType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let restr = complex(&a.schema, "restr");
    assert_eq!(content_shape(&a.schema, restr), "(a)");
    // The attribute that is declared again is the new one, x is gone, z and w are as they were.
    assert_eq!(attributes(&a.schema, restr), ["y*", "z*", "w"]);
    assert!(restr.attribute_wildcard.is_none(), "a restriction has the wildcard it says and no other");
    let id = a.schema.type_by_name(&QName::new(Some(NS), "restr")).expect("restr");
    assert_eq!(a.schema.type_entry(id).and_then(|t| t.derivation), Some(DerivationMethod::Restriction));
}

#[test]
fn simple_content_extends_and_restricts() {
    let a = build(
        "<xs:complexType name=\"price\"><xs:simpleContent><xs:extension base=\"xs:decimal\"><xs:attribute name=\"cur\" type=\"xs:string\"/></xs:extension></xs:simpleContent></xs:complexType>\
         <xs:complexType name=\"cheap\"><xs:simpleContent><xs:restriction base=\"t:price\"><xs:maxInclusive value=\"10\"/></xs:restriction></xs:simpleContent></xs:complexType>\
         <xs:complexType name=\"tiny\"><xs:simpleContent><xs:restriction base=\"t:cheap\"><xs:simpleType><xs:restriction base=\"xs:integer\"><xs:maxInclusive value=\"3\"/></xs:restriction></xs:simpleType></xs:restriction></xs:simpleContent></xs:complexType>\
         <xs:complexType name=\"money\"><xs:simpleContent><xs:extension base=\"t:cheap\"><xs:attribute name=\"note\"/></xs:extension></xs:simpleContent></xs:complexType>\
         <xs:complexType name=\"mx\" mixed=\"true\"><xs:sequence><xs:element name=\"i\" minOccurs=\"0\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"txt\"><xs:simpleContent><xs:restriction base=\"t:mx\"><xs:simpleType><xs:restriction base=\"xs:string\"><xs:maxLength value=\"2\"/></xs:restriction></xs:simpleType></xs:restriction></xs:simpleContent></xs:complexType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let text = |local: &str, lexical: &str| match &complex(&a.schema, local).content {
        ContentType::Simple(t) => t.validate(lexical, &xmlcraft_datatypes::NoNamespaces).is_ok(),
        other => panic!("{other:?}"),
    };
    assert!(text("price", "12.5") && !text("price", "x"));
    assert!(text("cheap", "9.5") && !text("cheap", "11"));
    assert!(text("tiny", "3") && !text("tiny", "4") && !text("tiny", "2.5"));
    assert!(text("money", "10") && !text("money", "10.5"));
    assert!(text("txt", "ab") && !text("txt", "abc"), "text of a mixed type that may be empty");
    assert_eq!(attributes(&a.schema, complex(&a.schema, "cheap")), ["cur"]);
    assert_eq!(attributes(&a.schema, complex(&a.schema, "money")), ["cur", "note"]);
}

#[test]
fn derivations_that_cannot_be_are_reported() {
    // A cycle is reported once, at one of the references.
    let a = build(
        "<xs:complexType name=\"a\"><xs:complexContent><xs:extension base=\"t:b\"/></xs:complexContent></xs:complexType>\
         <xs:complexType name=\"b\"><xs:complexContent><xs:extension base=\"t:a\"/></xs:complexContent></xs:complexType>",
    );
    assert_eq!(keys(&a), ["xsd.circular-definition"], "{:?}", messages(&a));
    // A union that lists itself twice is one mistake, not two.
    let a = build("<xs:simpleType name=\"u\"><xs:union memberTypes=\"t:u t:u\"/></xs:simpleType>");
    assert_eq!(keys(&a), ["xsd.circular-definition"], "{:?}", messages(&a));
    // `final` stops extension, and the default for the document stops restriction.
    let a = build_with(
        "finalDefault=\"restriction\"",
        "<xs:complexType name=\"base\" final=\"extension\"><xs:sequence><xs:element name=\"a\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"ext\"><xs:complexContent><xs:extension base=\"t:base\"/></xs:complexContent></xs:complexType>\
         <xs:complexType name=\"restr\"><xs:complexContent><xs:restriction base=\"t:other\"/></xs:complexContent></xs:complexType>\
         <xs:complexType name=\"other\"><xs:sequence><xs:element name=\"a\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"fine\"><xs:complexContent><xs:restriction base=\"t:base\"><xs:sequence><xs:element name=\"a\"/></xs:sequence></xs:restriction></xs:complexContent></xs:complexType>",
    );
    assert_eq!(keys(&a), ["xsd.derivation-not-allowed", "xsd.derivation-not-allowed"], "{:?}", messages(&a));
    // The wrong kind of base.
    let a = build(
        "<xs:complexType name=\"a\"><xs:complexContent><xs:extension base=\"xs:string\"/></xs:complexContent></xs:complexType>\
         <xs:complexType name=\"elements\"><xs:sequence><xs:element name=\"e\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"b\"><xs:simpleContent><xs:restriction base=\"t:elements\"/></xs:simpleContent></xs:complexType>\
         <xs:complexType name=\"c\"><xs:simpleContent><xs:restriction base=\"xs:string\"/></xs:simpleContent></xs:complexType>\
         <xs:complexType name=\"mixed\" mixed=\"true\"><xs:sequence><xs:element name=\"i\" minOccurs=\"0\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"d\"><xs:simpleContent><xs:restriction base=\"t:mixed\"/></xs:simpleContent></xs:complexType>",
    );
    let mut found = keys(&a);
    found.sort_unstable();
    assert_eq!(found, ["xsd.component", "xsd.wrong-kind-of-type", "xsd.wrong-kind-of-type", "xsd.wrong-kind-of-type"], "{:?}", messages(&a));
    // A type with simple content gets no elements, and mixed content has to stay as the base has it.
    let a = build(
        "<xs:complexType name=\"s\"><xs:simpleContent><xs:extension base=\"xs:string\"/></xs:simpleContent></xs:complexType>\
         <xs:complexType name=\"s1\"><xs:complexContent><xs:extension base=\"t:s\"><xs:sequence><xs:element name=\"a\"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>\
         <xs:complexType name=\"m\" mixed=\"true\"><xs:sequence><xs:element name=\"i\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"m1\"><xs:complexContent><xs:extension base=\"t:m\"><xs:sequence><xs:element name=\"j\"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>\
         <xs:complexType name=\"elements\"><xs:sequence><xs:element name=\"e\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"m3\"><xs:complexContent mixed=\"true\"><xs:extension base=\"t:elements\"><xs:sequence><xs:element name=\"f\"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>\
         <xs:complexType name=\"m2\"><xs:complexContent mixed=\"false\"><xs:extension base=\"t:m\"/></xs:complexContent></xs:complexType>\
         <xs:complexType name=\"unordered\"><xs:all><xs:element name=\"p\"/></xs:all></xs:complexType>\
         <xs:complexType name=\"u1\"><xs:complexContent><xs:extension base=\"t:unordered\"><xs:sequence><xs:element name=\"q\"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>",
    );
    assert_eq!(keys(&a), ["xsd.component"; 4], "{:?}", messages(&a));
    // Nothing to add, nothing to disagree about: `m2` has the content of `m`.
    assert!(!messages(&a).iter().any(|m| m.contains("m2")), "{:?}", messages(&a));
}

#[test]
fn the_block_of_a_type_comes_from_the_document_unless_it_says_otherwise() {
    let a = build_with(
        "blockDefault=\"extension substitution\"",
        "<xs:complexType name=\"plain\"/><xs:complexType name=\"own\" block=\"restriction\"/><xs:complexType name=\"none\" block=\"\"/>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let block = |local: &str| complex(&a.schema, local).block;
    assert!(block("plain").extension && !block("plain").restriction, "from blockDefault");
    assert!(!block("plain").substitution, "a type can only block derivation");
    assert!(block("own").restriction && !block("own").extension);
    assert!(block("none").is_empty());
}

#[test]
fn an_all_group_holds_elements_that_occur_once_at_most() {
    let a = build(
        "<xs:complexType name=\"t\"><xs:all><xs:element name=\"p\"/><xs:element name=\"q\" minOccurs=\"0\" maxOccurs=\"1\"/><xs:element name=\"r\" maxOccurs=\"2\"/></xs:all></xs:complexType>",
    );
    assert_eq!(keys(&a), ["xsd.component"], "{:?}", messages(&a));
}

#[test]
fn attributes_of_a_type_are_checked_against_each_other() {
    let a = build(
        "<xs:attributeGroup name=\"g\"><xs:attribute name=\"p\"/></xs:attributeGroup>\
         <xs:complexType name=\"twice\"><xs:attribute name=\"p\"/><xs:attributeGroup ref=\"t:g\"/></xs:complexType>\
         <xs:complexType name=\"ids\"><xs:attribute name=\"i\" type=\"xs:ID\"/><xs:attribute name=\"j\" type=\"xs:ID\"/></xs:complexType>\
         <xs:complexType name=\"base\"><xs:attribute name=\"k\"/></xs:complexType>\
         <xs:complexType name=\"again\"><xs:complexContent><xs:extension base=\"t:base\"><xs:attribute name=\"k\"/></xs:extension></xs:complexContent></xs:complexType>",
    );
    assert_eq!(keys(&a), ["xsd.component"; 3], "{:?}", messages(&a));
}

#[test]
fn a_group_is_expanded_where_it_is_used() {
    let a = build(
        "<xs:group name=\"g\"><xs:sequence><xs:element name=\"a\"/><xs:element name=\"b\"/></xs:sequence></xs:group>\
         <xs:group name=\"c\"><xs:choice><xs:element name=\"x\"/><xs:group ref=\"t:g\"/></xs:choice></xs:group>\
         <xs:complexType name=\"t1\"><xs:sequence><xs:group ref=\"t:g\" minOccurs=\"0\" maxOccurs=\"3\"/><xs:element name=\"c\"/><xs:group ref=\"t:c\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"t2\"><xs:group ref=\"t:g\"/></xs:complexType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    assert_eq!(content_shape(&a.schema, complex(&a.schema, "t1")), "((a,b){0,3},c,(x|(a,b)))");
    assert_eq!(content_shape(&a.schema, complex(&a.schema, "t2")), "(a,b)");
}

#[test]
fn groups_that_contain_themselves_are_reported_but_elements_may_come_back_to_their_group() {
    let a = build(
        "<xs:group name=\"g1\"><xs:sequence><xs:group ref=\"t:g2\"/></xs:sequence></xs:group>\
         <xs:group name=\"g2\"><xs:sequence><xs:group ref=\"t:g1\"/></xs:sequence></xs:group>\
         <xs:complexType name=\"t\"><xs:group ref=\"t:g1\"/></xs:complexType>\
         <xs:complexType name=\"u\"><xs:group ref=\"t:missing\"/></xs:complexType>",
    );
    assert_eq!(keys(&a), ["xsd.circular-definition", "xsd.unresolved-reference"], "{:?}", messages(&a));
    // A group whose element has a type that uses the group is not a cycle of groups.
    let a = build(
        "<xs:group name=\"g\"><xs:sequence><xs:element name=\"x\" minOccurs=\"0\"><xs:complexType><xs:group ref=\"t:g\"/></xs:complexType></xs:element></xs:sequence></xs:group>\
         <xs:element name=\"root\"><xs:complexType><xs:group ref=\"t:g\"/></xs:complexType></xs:element>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let root = a.schema.element(a.schema.element_by_name(&QName::new(Some(NS), "root")).expect("root")).expect("root");
    let TypeKind::Complex(c) = &a.schema.type_entry(root.type_id).expect("a type").kind else { panic!("not complex") };
    assert_eq!(content_shape(&a.schema, c), "(x{0,1})");
}

#[test]
fn groups_that_multiply_are_refused_before_they_use_up_the_memory() {
    let mut body = String::from("<xs:group name=\"g0\"><xs:sequence><xs:element name=\"a\"/></xs:sequence></xs:group>");
    for i in 1..=24 {
        body.push_str(&format!(
            "<xs:group name=\"g{i}\"><xs:sequence><xs:group ref=\"t:g{}\"/><xs:group ref=\"t:g{}\"/></xs:sequence></xs:group>",
            i - 1,
            i - 1
        ));
    }
    body.push_str("<xs:complexType name=\"t\"><xs:group ref=\"t:g24\"/></xs:complexType>");
    let a = build(&body);
    assert_eq!(keys(&a), ["xsd.limit"], "reported once");
}

#[test]
fn elements_have_their_types_and_may_refer_to_each_other() {
    let a = build_with(
        "elementFormDefault=\"qualified\" blockDefault=\"extension\" finalDefault=\"restriction\"",
        "<xs:element name=\"root\" type=\"t:rt\" nillable=\"true\"/>\
         <xs:complexType name=\"rt\"><xs:sequence><xs:element ref=\"t:root\" minOccurs=\"0\"/><xs:element name=\"q\" type=\"xs:int\"/><xs:element name=\"u\" form=\"unqualified\"/>\
           <xs:element name=\"anon\"><xs:simpleType><xs:restriction base=\"xs:string\"><xs:maxLength value=\"2\"/></xs:restriction></xs:simpleType></xs:element></xs:sequence></xs:complexType>\
         <xs:element name=\"abs\" abstract=\"true\" block=\"#all\" final=\"extension\"/>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let root = a.schema.element(a.schema.element_by_name(&QName::new(Some(NS), "root")).expect("root")).expect("root");
    assert!(root.nillable && !root.abstract_);
    assert!(root.block.extension && !root.block.restriction && !root.block.substitution, "from blockDefault");
    assert!(root.final_.restriction && !root.final_.extension, "from finalDefault");
    let q = a.schema.elements.iter().find(|e| e.name.local == "q").expect("q");
    assert!(q.final_.is_empty(), "a local element is not final for anything");
    assert_eq!(Some(root.type_id), a.schema.type_by_name(&QName::new(Some(NS), "rt")));
    assert_eq!(content_shape(&a.schema, complex(&a.schema, "rt")), "(root{0,1},q,u,anon)");
    let names: Vec<_> = a.schema.elements.iter().map(|e| (e.name.local.clone(), e.name.namespace.clone(), e.scope.is_some())).collect();
    assert!(names.contains(&("q".to_owned(), Some(NS.to_owned()), true)), "a local element is qualified by elementFormDefault: {names:?}");
    assert!(names.contains(&("u".to_owned(), None, true)), "and not when it says it is not");
    assert!(names.contains(&("root".to_owned(), Some(NS.to_owned()), false)), "a global element has no scope");
    let abs = a.schema.element(a.schema.element_by_name(&QName::new(Some(NS), "abs")).expect("abs")).expect("abs");
    assert!(
        abs.abstract_ && abs.block.extension && abs.block.restriction && abs.block.substitution && abs.final_.extension && !abs.final_.restriction
    );
    assert_eq!(Some(abs.type_id), a.schema.type_by_name(&QName::new(Some(XSD_NAMESPACE), "anyType")), "no type: anyType");
}

#[test]
fn what_an_element_refers_to_has_to_be_there() {
    let a = build(
        "<xs:element name=\"a\" type=\"t:nothing\"/>\
         <xs:element name=\"b\" type=\"t:rt\"/>\
         <xs:element name=\"b\" type=\"xs:string\"/>\
         <xs:complexType name=\"rt\"><xs:sequence><xs:element ref=\"t:gone\"/></xs:sequence></xs:complexType>",
    );
    assert_eq!(keys(&a), ["xsd.unresolved-reference", "xsd.duplicate-definition", "xsd.unresolved-reference"], "{:?}", messages(&a));
}

#[test]
fn the_default_and_the_fixed_value_of_an_element_must_be_valid_for_its_type() {
    let a = build(
        "<xs:element name=\"ok\" type=\"xs:int\" default=\"5\"/>\
         <xs:element name=\"fix\" type=\"t:later\" fixed=\"abc\"/>\
         <xs:simpleType name=\"later\"><xs:restriction base=\"xs:string\"><xs:maxLength value=\"3\"/></xs:restriction></xs:simpleType>\
         <xs:element name=\"mixed\" default=\"x\"><xs:complexType mixed=\"true\"><xs:sequence><xs:element name=\"i\" minOccurs=\"0\"/></xs:sequence></xs:complexType></xs:element>\
         <xs:element name=\"choice\" default=\"x\"><xs:complexType mixed=\"true\"><xs:choice><xs:element name=\"i\"/><xs:element name=\"j\" minOccurs=\"0\"/></xs:choice></xs:complexType></xs:element>\
         <xs:element name=\"text\" type=\"t:amount\" default=\"1.5\"/>\
         <xs:complexType name=\"amount\"><xs:simpleContent><xs:extension base=\"xs:decimal\"/></xs:simpleContent></xs:complexType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let constraint = |local: &str| {
        let id = a.schema.element_by_name(&QName::new(Some(NS), local)).expect("an element");
        a.schema.element(id).expect("an element").value_constraint.clone()
    };
    let ok = constraint("ok").expect("a default");
    assert_eq!((ok.lexical.as_str(), ok.fixed), ("5", false));
    let fix = constraint("fix").expect("a fixed value");
    assert_eq!((fix.lexical.as_str(), fix.fixed), ("abc", true));
    assert!(constraint("mixed").is_some() && constraint("text").is_some() && constraint("choice").is_some());

    let a = build(
        "<xs:element name=\"bad\" type=\"xs:int\" default=\"five\"/>\
         <xs:element name=\"long\" type=\"t:short\" fixed=\"abcd\"/>\
         <xs:simpleType name=\"short\"><xs:restriction base=\"xs:string\"><xs:maxLength value=\"3\"/></xs:restriction></xs:simpleType>\
         <xs:element name=\"id\" type=\"xs:ID\" default=\"x\"/>\
         <xs:element name=\"elements\" default=\"x\"><xs:complexType><xs:sequence><xs:element name=\"i\"/></xs:sequence></xs:complexType></xs:element>\
         <xs:element name=\"required\" default=\"x\"><xs:complexType mixed=\"true\"><xs:sequence><xs:element name=\"i\"/></xs:sequence></xs:complexType></xs:element>",
    );
    assert_eq!(keys(&a), ["xsd.value-constraint"; 5], "{:?}", messages(&a));
}

#[test]
fn a_substitution_group_gives_its_members_the_type_of_the_head() {
    let a = build(
        "<xs:element name=\"member\" substitutionGroup=\"t:head\"/>\
         <xs:element name=\"head\" type=\"t:text\"/>\
         <xs:simpleType name=\"text\"><xs:restriction base=\"xs:string\"/></xs:simpleType>\
         <xs:simpleType name=\"short\"><xs:restriction base=\"t:text\"><xs:maxLength value=\"2\"/></xs:restriction></xs:simpleType>\
         <xs:element name=\"narrow\" type=\"t:short\" substitutionGroup=\"t:head\"/>\
         <xs:element name=\"deep\" substitutionGroup=\"t:member\"/>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let element = |local: &str| a.schema.element(a.schema.element_by_name(&QName::new(Some(NS), local)).expect("an element")).expect("an element");
    let text = a.schema.type_by_name(&QName::new(Some(NS), "text"));
    assert_eq!(Some(element("member").type_id), text);
    assert_eq!(Some(element("deep").type_id), text, "through a member of a member");
    assert_eq!(a.schema.element_by_name(&QName::new(Some(NS), "head")), element("member").substitution_group);
    assert_eq!(a.schema.element_by_name(&QName::new(Some(NS), "member")), element("deep").substitution_group);
}

#[test]
fn a_substitution_group_must_not_come_back_to_itself_or_change_the_type() {
    let a = build(
        "<xs:element name=\"a\" substitutionGroup=\"t:b\"/><xs:element name=\"b\" substitutionGroup=\"t:a\"/>\
         <xs:element name=\"head\" type=\"xs:string\"/><xs:element name=\"wrong\" type=\"xs:int\" substitutionGroup=\"t:head\"/>\
         <xs:element name=\"nohead\" substitutionGroup=\"t:nothing\"/>",
    );
    let mut keys = keys(&a);
    keys.sort_unstable();
    assert_eq!(keys, ["xsd.circular-definition", "xsd.component", "xsd.unresolved-reference"], "{:?}", messages(&a));
}

#[test]
fn identity_constraints_are_found_and_their_keyrefs_resolved() {
    let a = build(
        "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"a\" maxOccurs=\"unbounded\"/><xs:element name=\"b\" maxOccurs=\"unbounded\"/></xs:sequence></xs:complexType>\
           <xs:keyref name=\"kr\" refer=\"t:k\"><xs:selector xpath=\"b\"/><xs:field xpath=\"@ref\"/></xs:keyref>\
           <xs:key name=\"k\"><xs:selector xpath=\"a\"/><xs:field xpath=\"@id\"/></xs:key>\
           <xs:unique name=\"u\"><xs:selector xpath=\".//a\"/><xs:field xpath=\"@x\"/><xs:field xpath=\"@y\"/></xs:unique>\
         </xs:element>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let r = a.schema.element(a.schema.element_by_name(&QName::new(Some(NS), "r")).expect("r")).expect("r");
    assert_eq!(r.identity_constraints.len(), 3);
    let by_name =
        |local: &str| r.identity_constraints.iter().copied().find(|id| a.schema.constraints[id.0].name.local == local).expect("a constraint");
    let kr = &a.schema.constraints[by_name("kr").0];
    assert_eq!(kr.refer, Some(by_name("k")));
    assert_eq!((kr.selector.as_str(), kr.fields.as_slice()), ("b", ["@ref".to_owned()].as_slice()));
    assert_eq!(a.schema.constraints[by_name("u").0].fields.len(), 2);
    assert!(
        a.schema.constraints[by_name("u").0].namespaces.iter().any(|(p, u)| p.as_deref() == Some("xs") && u == XSD_NAMESPACE),
        "the prefixes in scope are kept"
    );
}

#[test]
fn mistakes_in_identity_constraints_are_reported() {
    let a = build(
        "<xs:element name=\"r\">\
           <xs:key name=\"k\"><xs:selector xpath=\"a\"/><xs:field xpath=\"@id\"/></xs:key>\
           <xs:keyref name=\"gone\" refer=\"t:nothing\"><xs:selector xpath=\"b\"/><xs:field xpath=\"@r\"/></xs:keyref>\
           <xs:keyref name=\"two\" refer=\"t:k\"><xs:selector xpath=\"b\"/><xs:field xpath=\"@r\"/><xs:field xpath=\"@s\"/></xs:keyref>\
           <xs:keyref name=\"loop\" refer=\"t:gone\"><xs:selector xpath=\"b\"/><xs:field xpath=\"@r\"/></xs:keyref>\
           <xs:unique name=\"k\"><xs:selector xpath=\"c\"/><xs:field xpath=\"@z\"/></xs:unique>\
         </xs:element>",
    );
    assert_eq!(
        keys(&a),
        ["xsd.unresolved-reference", "xsd.component", "xsd.wrong-kind-of-type", "xsd.duplicate-definition"].map(|k| k),
        "{:?}",
        messages(&a)
    );
}

#[test]
fn attributes_are_global_or_local_and_may_be_referred_to() {
    let a = build_with(
        "attributeFormDefault=\"qualified\"",
        "<xs:attribute name=\"lang\" type=\"xs:language\" default=\"en\"/>\
         <xs:attribute name=\"unused\" type=\"xs:int\"/>\
         <xs:complexType name=\"t\"><xs:attribute ref=\"t:lang\" use=\"required\"/><xs:attribute name=\"local\"/><xs:attribute name=\"bare\" form=\"unqualified\" type=\"t:code\"/>\
           <xs:attribute ref=\"t:unused\" use=\"prohibited\"/><xs:attribute ref=\"t:lang\" fixed=\"fr\" use=\"optional\"/></xs:complexType>\
         <xs:simpleType name=\"code\"><xs:restriction base=\"xs:token\"><xs:length value=\"2\"/></xs:restriction></xs:simpleType>",
    );
    // `lang` twice: the second use is reported.
    assert_eq!(keys(&a), ["xsd.component"], "{:?}", messages(&a));
    let t = complex(&a.schema, "t");
    assert_eq!(attributes(&a.schema, t), ["lang*", "local", "bare"], "a prohibited use is no use");
    let decls: Vec<_> = a.schema.attributes.iter().map(|d| (d.name.local.clone(), d.name.namespace.clone(), d.global)).collect();
    assert!(decls.contains(&("lang".to_owned(), Some(NS.to_owned()), true)));
    assert!(decls.contains(&("unused".to_owned(), Some(NS.to_owned()), true)), "a global attribute nothing refers to is still there");
    assert!(decls.contains(&("local".to_owned(), Some(NS.to_owned()), false)), "qualified by attributeFormDefault");
    assert!(decls.contains(&("bare".to_owned(), None, false)));
    let lang = a.schema.attributes.iter().find(|d| d.name.local == "lang").expect("lang");
    assert_eq!(lang.value_constraint.as_ref().map(|c| (c.lexical.as_str(), c.fixed)), Some(("en", false)));
}

#[test]
fn the_default_of_an_attribute_must_be_valid_for_its_type() {
    let a = build(
        "<xs:attribute name=\"n\" type=\"xs:int\" default=\"x\"/>\
         <xs:attribute name=\"i\" type=\"xs:ID\" fixed=\"a\"/>\
         <xs:complexType name=\"t\"><xs:attribute name=\"m\" type=\"xs:boolean\" fixed=\"maybe\"/><xs:attribute ref=\"t:n\" default=\"7\"/><xs:attribute ref=\"t:n\"/></xs:complexType>",
    );
    // n, i and m are wrong; the reference `t:n` with its own default is fine, and using n twice is a mistake of its own.
    let mut keys = keys(&a);
    keys.sort_unstable();
    assert_eq!(keys, ["xsd.component", "xsd.value-constraint", "xsd.value-constraint", "xsd.value-constraint"], "{:?}", messages(&a));
}

#[test]
fn attribute_groups_are_expanded_and_their_wildcards_intersected() {
    let a = build(
        "<xs:attributeGroup name=\"a\"><xs:attribute name=\"p\"/><xs:anyAttribute namespace=\"urn:a urn:b ##targetNamespace\"/></xs:attributeGroup>\
         <xs:attributeGroup name=\"b\"><xs:attribute name=\"q\" use=\"required\"/><xs:attributeGroup ref=\"t:c\"/></xs:attributeGroup>\
         <xs:attributeGroup name=\"c\"><xs:attribute name=\"r\"/><xs:anyAttribute namespace=\"urn:b urn:c ##targetNamespace\" processContents=\"skip\"/></xs:attributeGroup>\
         <xs:complexType name=\"t\"><xs:attributeGroup ref=\"t:a\"/><xs:attributeGroup ref=\"t:b\"/></xs:complexType>\
         <xs:complexType name=\"u\"><xs:attributeGroup ref=\"t:c\"/><xs:anyAttribute namespace=\"##other\" processContents=\"lax\"/></xs:complexType>",
    );
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let t = complex(&a.schema, "t");
    assert_eq!(attributes(&a.schema, t), ["p", "q*", "r"]);
    assert_eq!(t.attribute_wildcard.as_ref().map(|w| &w.constraint), Some(&NamespaceSet::List(vec![Some("urn:b".to_owned()), Some(NS.to_owned())])));
    // `##other` of this schema leaves out the target namespace, and the rest of the group's list stays.
    let u = complex(&a.schema, "u");
    let w = u.attribute_wildcard.as_ref().expect("a wildcard");
    assert_eq!(w.constraint, NamespaceSet::List(vec![Some("urn:b".to_owned()), Some("urn:c".to_owned())]));
    assert_eq!(w.process_contents, xmlcraft_xsd::model::ProcessContents::Lax, "from the wildcard written in the type");
}

#[test]
fn attribute_groups_that_contain_themselves_or_are_missing_are_reported() {
    let a = build(
        "<xs:attributeGroup name=\"a\"><xs:attributeGroup ref=\"t:b\"/></xs:attributeGroup>\
         <xs:attributeGroup name=\"b\"><xs:attributeGroup ref=\"t:a\"/></xs:attributeGroup>\
         <xs:complexType name=\"t\"><xs:attributeGroup ref=\"t:a\"/><xs:attributeGroup ref=\"t:none\"/></xs:complexType>",
    );
    assert_eq!(keys(&a), ["xsd.circular-definition", "xsd.unresolved-reference"], "{:?}", messages(&a));
}

#[test]
fn a_wildcard_that_cannot_be_written_is_reported() {
    // Two groups that exclude different namespaces have in common what neither can name.
    let other = format!(
        "<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" targetNamespace=\"urn:o\"><xs:attributeGroup name=\"g\"><xs:anyAttribute namespace=\"##other\"/></xs:attributeGroup></xs:schema>"
    );
    let root = format!(
        "<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" xmlns:o=\"urn:o\" xmlns:t=\"{NS}\" targetNamespace=\"{NS}\"><xs:import namespace=\"urn:o\" schemaLocation=\"o.xsd\"/>\
         <xs:attributeGroup name=\"mine\"><xs:anyAttribute namespace=\"##other\"/></xs:attributeGroup>\
         <xs:complexType name=\"t\"><xs:attributeGroup ref=\"t:mine\"/><xs:attributeGroup ref=\"o:g\"/></xs:complexType></xs:schema>"
    );
    let mut files = Files(vec![("/s/o.xsd", other)]);
    let docs = load(&root, "/s/root.xsd", &mut files);
    assert!(docs.diagnostics.is_empty(), "{:?}", docs.diagnostics);
    let a = assemble(&docs);
    assert_eq!(keys(&a), ["xsd.component"], "{:?}", messages(&a));
}

#[test]
fn notations_are_declared_once() {
    let a = build("<xs:notation name=\"gif\" public=\"image/gif\" system=\"viewer.exe\"/><xs:notation name=\"png\" system=\"png.exe\"/>");
    assert!(a.diagnostics.is_empty(), "{:?}", messages(&a));
    let gif = a.schema.notations.iter().find(|n| n.name.local == "gif").expect("gif");
    assert_eq!((gif.public.as_deref(), gif.system.as_deref()), (Some("image/gif"), Some("viewer.exe")));
    assert_eq!(a.schema.notations.len(), 2);
    let a = build("<xs:notation name=\"gif\" public=\"a\"/><xs:notation name=\"gif\" public=\"b\"/>");
    assert_eq!(keys(&a), ["xsd.duplicate-definition"]);
}

#[test]
fn xs_redefine_is_reported_as_not_applied() {
    let included = format!(
        "<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" targetNamespace=\"{NS}\"><xs:simpleType name=\"a\"><xs:restriction base=\"xs:string\"/></xs:simpleType></xs:schema>"
    );
    let root = format!(
        "<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" targetNamespace=\"{NS}\"><xs:redefine schemaLocation=\"inc.xsd\"><xs:simpleType name=\"a\"><xs:restriction base=\"a\"><xs:maxLength value=\"2\"/></xs:restriction></xs:simpleType></xs:redefine></xs:schema>"
    );
    let mut files = Files(vec![("/s/inc.xsd", included)]);
    let docs = load(&root, "/s/root.xsd", &mut files);
    let a = assemble(&docs);
    assert_eq!(keys(&a), ["xsd.unsupported"], "{:?}", messages(&a));
}

struct Files(Vec<(&'static str, String)>);

impl SchemaLoader for Files {
    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String> {
        let wanted = request.location.unwrap_or_default();
        self.0
            .iter()
            .find(|(l, _)| l.ends_with(wanted))
            .map(|(l, t)| Loaded { text: t.clone(), location: (*l).to_owned() })
            .ok_or_else(|| "no".to_owned())
    }
}
