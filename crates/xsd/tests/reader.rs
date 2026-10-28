//! Reading schema documents: what the model holds, and what is reported.

use xmlcraft_xsd::model::*;
use xmlcraft_xsd::{Read, read};

const XS: &str = "http://www.w3.org/2001/XMLSchema";

fn schema(attributes: &str, body: &str) -> Read {
    read(&format!("<xs:schema xmlns:xs=\"{XS}\" xmlns:tns=\"urn:t\" targetNamespace=\"urn:t\" {attributes}>{body}</xs:schema>"), "test.xsd")
}

fn items(body: &str) -> Vec<Item> {
    let r = schema("", body);
    assert!(r.diagnostics.is_empty(), "{:?}", messages(&r));
    r.document.expect("a schema").items
}

fn messages(r: &Read) -> Vec<String> {
    r.diagnostics.iter().map(|d| format!("{}: {}", d.code.key(), d.message())).collect()
}

fn keys(r: &Read) -> Vec<&'static str> {
    r.diagnostics.iter().map(|d| d.code.key()).collect()
}

fn problems(body: &str) -> Vec<&'static str> {
    keys(&schema("", body))
}

#[test]
fn the_attributes_of_the_schema_element() {
    let r = read(
        &format!(
            "<xs:schema xmlns:xs=\"{XS}\" targetNamespace=\"urn:a\" version=\"1.2\" id=\"s\" elementFormDefault=\"qualified\" attributeFormDefault=\"qualified\" \
             blockDefault=\"extension substitution\" finalDefault=\"#all\" xml:lang=\"en\"/>"
        ),
        "here.xsd",
    );
    assert!(r.diagnostics.is_empty(), "{:?}", messages(&r));
    let d = r.document.expect("a schema");
    assert_eq!(
        (d.location.as_str(), d.target_namespace.as_deref(), d.version.as_deref(), d.id.as_deref()),
        ("here.xsd", Some("urn:a"), Some("1.2"), Some("s"))
    );
    assert_eq!((d.element_form_default, d.attribute_form_default), (Form::Qualified, Form::Qualified));
    assert_eq!(d.block_default, DerivationSet { extension: true, substitution: true, ..DerivationSet::default() });
    assert_eq!(d.final_default, DerivationSet { extension: true, restriction: true, list: true, union: true, ..DerivationSet::default() });
    // The defaults.
    let d = read(&format!("<xs:schema xmlns:xs=\"{XS}\"/>"), "").document.expect("a schema");
    assert_eq!(
        (d.target_namespace, d.element_form_default, d.attribute_form_default, d.block_default, d.final_default),
        (None, Form::Unqualified, Form::Unqualified, DerivationSet::default(), DerivationSet::default())
    );
}

#[test]
fn a_document_that_is_not_a_schema_is_said_not_to_be() {
    for text in ["<a/>", "<xs:schema xmlns:xs=\"urn:other\"/>", "<schema/>", "", "not xml"] {
        let r = read(text, "");
        assert!(r.document.is_none(), "{text:?}");
        assert!(!r.diagnostics.is_empty(), "{text:?}");
    }
    assert_eq!(keys(&read("<a/>", ""))[0], "xsd.not-a-schema");
}

#[test]
fn includes_imports_and_redefinitions_come_first() {
    let items = items(
        "<xs:annotation><xs:documentation>d</xs:documentation></xs:annotation>\
         <xs:include schemaLocation=\"a.xsd\"/><xs:import namespace=\"urn:o\" schemaLocation=\"o.xsd\"/><xs:import/>\
         <xs:redefine schemaLocation=\"r.xsd\"><xs:simpleType name=\"s\"><xs:restriction base=\"xs:string\"/></xs:simpleType><xs:group name=\"g\"><xs:sequence/></xs:group></xs:redefine>\
         <xs:element name=\"e\"/>",
    );
    assert!(matches!(&items[0], Item::Include(i) if i.schema_location == "a.xsd"));
    assert!(matches!(&items[1], Item::Import(i) if i.namespace.as_deref() == Some("urn:o") && i.schema_location.as_deref() == Some("o.xsd")));
    assert!(matches!(&items[2], Item::Import(i) if i.namespace.is_none() && i.schema_location.is_none()));
    assert!(matches!(&items[3], Item::Redefine(r) if r.schema_location == "r.xsd" && r.items.len() == 2));
    assert!(matches!(&items[4], Item::Element(_)));
    assert_eq!(problems("<xs:element name=\"e\"/><xs:include schemaLocation=\"a.xsd\"/>"), ["xsd.unexpected-element"]);
    assert_eq!(problems("<xs:include/>"), ["xsd.missing-attribute"]);
    assert_eq!(problems("<xs:redefine/>"), ["xsd.missing-attribute"]);
    assert_eq!(problems("<xs:redefine schemaLocation=\"r.xsd\"><xs:element name=\"e\"/></xs:redefine>"), ["xsd.unexpected-element"]);
}

#[test]
fn simple_types() {
    let items = items(
        "<xs:simpleType name=\"a\" final=\"restriction\"><xs:restriction base=\"xs:string\"><xs:minLength value=\"1\" fixed=\"true\"/><xs:pattern value=\"[a-z]+\"/>\
         <xs:enumeration value=\"x\"/></xs:restriction></xs:simpleType>\
         <xs:simpleType name=\"b\"><xs:restriction><xs:simpleType><xs:restriction base=\"xs:int\"/></xs:simpleType><xs:maxInclusive value=\"5\"/></xs:restriction></xs:simpleType>\
         <xs:simpleType name=\"c\"><xs:list itemType=\"tns:a\"/></xs:simpleType>\
         <xs:simpleType name=\"d\"><xs:list><xs:simpleType><xs:restriction base=\"xs:int\"/></xs:simpleType></xs:list></xs:simpleType>\
         <xs:simpleType name=\"e\"><xs:union memberTypes=\"xs:int tns:a\"><xs:simpleType><xs:restriction base=\"xs:string\"/></xs:simpleType></xs:union></xs:simpleType>",
    );
    let Item::SimpleType(a) = &items[0] else { panic!("a simple type") };
    assert_eq!(a.name.as_deref(), Some("a"));
    assert_eq!(a.final_, Some(DerivationSet { restriction: true, ..DerivationSet::default() }));
    let SimpleTypeKind::Restriction { base, inline_base, facets } = &a.kind else { panic!("a restriction") };
    assert_eq!(base.as_ref(), Some(&QName::new(Some(XS), "string")));
    assert!(inline_base.is_none());
    assert_eq!(
        facets.iter().map(|f| (f.kind, f.value.as_str(), f.fixed)).collect::<Vec<_>>(),
        [(FacetKind::MinLength, "1", true), (FacetKind::Pattern, "[a-z]+", false), (FacetKind::Enumeration, "x", false)]
    );
    let Item::SimpleType(b) = &items[1] else { panic!("a simple type") };
    assert!(matches!(&b.kind, SimpleTypeKind::Restriction { base: None, inline_base: Some(_), facets } if facets.len() == 1));
    let Item::SimpleType(c) = &items[2] else { panic!("a simple type") };
    assert!(matches!(&c.kind, SimpleTypeKind::List { item_type: Some(q), inline_item: None } if *q == QName::new(Some("urn:t"), "a")));
    let Item::SimpleType(d) = &items[3] else { panic!("a simple type") };
    assert!(matches!(&d.kind, SimpleTypeKind::List { item_type: None, inline_item: Some(_) }));
    let Item::SimpleType(e) = &items[4] else { panic!("a simple type") };
    assert!(matches!(&e.kind, SimpleTypeKind::Union { member_types, inline_members } if member_types.len() == 2 && inline_members.len() == 1));
}

#[test]
fn mistakes_in_simple_types() {
    let restriction = |inner: &str| format!("<xs:simpleType name=\"t\">{inner}</xs:simpleType>");
    assert_eq!(problems(&restriction("")), ["xsd.missing-content"]);
    assert_eq!(problems(&restriction("<xs:restriction/>")), ["xsd.missing-content"]);
    assert_eq!(
        problems(&restriction(
            "<xs:restriction base=\"xs:string\"><xs:simpleType><xs:restriction base=\"xs:int\"/></xs:simpleType></xs:restriction>"
        )),
        ["xsd.conflicting-attributes"]
    );
    assert_eq!(problems(&restriction("<xs:list/>")), ["xsd.missing-content"]);
    assert_eq!(
        problems(&restriction("<xs:list itemType=\"xs:int\"><xs:simpleType><xs:restriction base=\"xs:int\"/></xs:simpleType></xs:list>")),
        ["xsd.conflicting-attributes"]
    );
    assert_eq!(problems(&restriction("<xs:union/>")), ["xsd.missing-content"]);
    assert_eq!(problems(&restriction("<xs:restriction base=\"xs:string\"/><xs:list itemType=\"xs:int\"/>")), ["xsd.duplicate-child"]);
    assert_eq!(problems(&restriction("<xs:restriction base=\"nope:string\"/>")), ["xsd.bad-attribute-value"]);
    assert_eq!(problems(&restriction("<xs:restriction base=\"1x\"/>")), ["xsd.bad-attribute-value"]);
    assert_eq!(problems(&restriction("<xs:restriction base=\"xs:string\"><xs:minLength/></xs:restriction>")), ["xsd.missing-attribute"]);
    assert_eq!(
        problems(&restriction("<xs:restriction base=\"xs:string\"><xs:enumeration value=\"a\" fixed=\"true\"/></xs:restriction>")),
        ["xsd.unknown-attribute"]
    );
    assert_eq!(
        problems(&restriction("<xs:restriction base=\"xs:string\"><xs:minLength value=\"1\" fixed=\"maybe\"/></xs:restriction>")),
        ["xsd.bad-attribute-value"]
    );
    assert_eq!(problems(&restriction("<xs:restriction base=\"xs:string\"><xs:bogus value=\"1\"/></xs:restriction>")), ["xsd.unexpected-element"]);
    assert_eq!(problems("<xs:simpleType><xs:restriction base=\"xs:string\"/></xs:simpleType>"), ["xsd.missing-attribute"]);
    assert_eq!(problems("<xs:simpleType name=\"a b\"><xs:restriction base=\"xs:string\"/></xs:simpleType>"), ["xsd.bad-attribute-value"]);
    assert_eq!(
        problems("<xs:simpleType name=\"t\" final=\"extension\"><xs:restriction base=\"xs:string\"/></xs:simpleType>"),
        ["xsd.bad-attribute-value"]
    );
    assert_eq!(
        problems("<xs:element name=\"e\"><xs:simpleType name=\"local\"><xs:restriction base=\"xs:string\"/></xs:simpleType></xs:element>"),
        ["xsd.unknown-attribute"]
    );
}

#[test]
fn qualified_names_use_the_namespaces_in_scope_where_they_are_written() {
    let r = read(
        &format!(
            "<xs:schema xmlns:xs=\"{XS}\" xmlns=\"urn:d\"><xs:simpleType name=\"t\" xmlns:p=\"urn:p\"><xs:restriction base=\"p:b\"/></xs:simpleType><xs:simpleType name=\"u\"><xs:restriction base=\"b\"/></xs:simpleType></xs:schema>"
        ),
        "",
    );
    assert!(r.diagnostics.is_empty(), "{:?}", messages(&r));
    let Some(SchemaDocument { items, .. }) = r.document else { panic!("a schema") };
    let base = |i: usize| match &items[i] {
        Item::SimpleType(SimpleTypeDef { kind: SimpleTypeKind::Restriction { base, .. }, .. }) => base.clone(),
        _ => None,
    };
    assert_eq!(base(0), Some(QName::new(Some("urn:p"), "b")));
    assert_eq!(base(1), Some(QName::new(Some("urn:d"), "b")), "no prefix: the default namespace");
    // Outside the element that declares `p` the prefix is not known.
    let r = read(
        &format!(
            "<xs:schema xmlns:xs=\"{XS}\"><xs:annotation xmlns:p=\"urn:p\"/><xs:simpleType name=\"t\"><xs:restriction base=\"p:b\"/></xs:simpleType></xs:schema>"
        ),
        "",
    );
    assert_eq!(keys(&r), ["xsd.bad-attribute-value"]);
    assert!(messages(&r)[0].contains("prefix 'p' is not declared"), "{:?}", messages(&r));
    // Without a default namespace, an unprefixed name is in no namespace.
    let r = read(&format!("<xs:schema xmlns:xs=\"{XS}\"><xs:simpleType name=\"t\"><xs:restriction base=\"b\"/></xs:simpleType></xs:schema>"), "");
    let Some(SchemaDocument { items, .. }) = r.document else { panic!("a schema") };
    assert!(
        matches!(&items[0], Item::SimpleType(SimpleTypeDef { kind: SimpleTypeKind::Restriction { base: Some(q), .. }, .. }) if *q == QName::new(None, "b"))
    );
}

#[test]
fn complex_types() {
    let items = items(
        "<xs:complexType name=\"a\" abstract=\"true\" mixed=\"true\" block=\"#all\" final=\"extension\">\
           <xs:sequence><xs:element name=\"x\"/><xs:choice minOccurs=\"0\" maxOccurs=\"unbounded\"><xs:element ref=\"tns:y\"/><xs:any namespace=\"##other\" processContents=\"lax\"/></xs:choice></xs:sequence>\
           <xs:attribute name=\"p\" type=\"xs:int\" use=\"required\"/><xs:attributeGroup ref=\"tns:ag\"/><xs:anyAttribute namespace=\"urn:q ##local\"/></xs:complexType>\
         <xs:complexType name=\"b\"><xs:simpleContent><xs:extension base=\"xs:string\"><xs:attribute name=\"u\"/></xs:extension></xs:simpleContent></xs:complexType>\
         <xs:complexType name=\"c\"><xs:simpleContent><xs:restriction base=\"tns:b\"><xs:maxLength value=\"3\"/><xs:attribute name=\"u\" use=\"prohibited\"/></xs:restriction></xs:simpleContent></xs:complexType>\
         <xs:complexType name=\"d\" mixed=\"true\"><xs:complexContent mixed=\"false\"><xs:extension base=\"tns:a\"><xs:all><xs:element name=\"z\"/></xs:all></xs:extension></xs:complexContent></xs:complexType>\
         <xs:complexType name=\"e\"><xs:complexContent><xs:restriction base=\"tns:a\"><xs:group ref=\"tns:g\"/></xs:restriction></xs:complexContent></xs:complexType>\
         <xs:complexType name=\"f\"/>",
    );
    let Item::ComplexType(a) = &items[0] else { panic!("a complex type") };
    assert_eq!((a.abstract_, a.mixed), (Some(true), Some(true)));
    assert_eq!(a.block, Some(DerivationSet { extension: true, restriction: true, ..DerivationSet::default() }));
    let ComplexContentDef::Implicit { particle: Some(p), attributes } = &a.content else { panic!("implicit content") };
    let Term::Sequence(parts) = &p.term else { panic!("a sequence") };
    assert_eq!(parts.len(), 2);
    assert!(matches!(&parts[0].term, Term::Element(e) if e.name.as_deref() == Some("x")));
    assert_eq!((parts[1].min_occurs, parts[1].max_occurs), (0, MaxOccurs::Unbounded));
    let Term::Choice(options) = &parts[1].term else { panic!("a choice") };
    assert!(matches!(&options[0].term, Term::Element(e) if e.reference == Some(QName::new(Some("urn:t"), "y"))));
    assert!(matches!(&options[1].term, Term::Any(w) if w.namespace == NamespaceConstraint::Other && w.process_contents == ProcessContents::Lax));
    assert_eq!((attributes.attributes.len(), attributes.groups.len()), (1, 1));
    assert_eq!(attributes.attributes[0].use_, Some(AttributeUse::Required));
    assert!(
        matches!(&attributes.wildcard, Some(w) if w.namespace == NamespaceConstraint::List(vec![NamespaceToken::Uri("urn:q".into()), NamespaceToken::Local]))
    );
    assert!(matches!(
        &items[1],
        Item::ComplexType(ComplexTypeDef { content: ComplexContentDef::Simple { method: DerivationMethod::Extension, base: Some(_), .. }, .. })
    ));
    let Item::ComplexType(c) = &items[2] else { panic!("a complex type") };
    let ComplexContentDef::Simple { method, facets, attributes, .. } = &c.content else { panic!("simple content") };
    assert_eq!((*method, facets.len(), attributes.attributes[0].use_), (DerivationMethod::Restriction, 1, Some(AttributeUse::Prohibited)));
    let Item::ComplexType(d) = &items[3] else { panic!("a complex type") };
    let ComplexContentDef::Complex { method, mixed, particle, .. } = &d.content else { panic!("complex content") };
    assert_eq!((*method, *mixed, matches!(particle, Some(Particle { term: Term::All(_), .. }))), (DerivationMethod::Extension, Some(false), true));
    let Item::ComplexType(e) = &items[4] else { panic!("a complex type") };
    assert!(matches!(
        &e.content,
        ComplexContentDef::Complex { method: DerivationMethod::Restriction, particle: Some(Particle { term: Term::GroupRef(_), .. }), .. }
    ));
    assert!(matches!(&items[5], Item::ComplexType(ComplexTypeDef { content: ComplexContentDef::Implicit { particle: None, .. }, .. })));
}

#[test]
fn mistakes_in_complex_types() {
    let ct = |inner: &str| format!("<xs:complexType name=\"t\">{inner}</xs:complexType>");
    assert_eq!(problems(&ct("<xs:simpleContent><xs:extension base=\"xs:string\"/></xs:simpleContent><xs:sequence/>")), ["xsd.unexpected-element"]);
    assert_eq!(problems(&ct("<xs:sequence/><xs:simpleContent><xs:extension base=\"xs:string\"/></xs:simpleContent>")), ["xsd.unexpected-element"]);
    assert_eq!(problems(&ct("<xs:sequence/><xs:choice/>")), ["xsd.duplicate-child"]);
    assert_eq!(problems(&ct("<xs:anyAttribute/><xs:attribute name=\"a\"/>")), ["xsd.unexpected-element"]);
    assert_eq!(problems(&ct("<xs:anyAttribute/><xs:anyAttribute/>")), ["xsd.duplicate-child"]);
    assert_eq!(problems(&ct("<xs:simpleContent/>")), ["xsd.missing-content"]);
    assert_eq!(problems(&ct("<xs:complexContent><xs:extension/></xs:complexContent>")), ["xsd.missing-attribute"]);
    assert_eq!(
        problems(&ct("<xs:complexContent><xs:extension base=\"xs:string\"/><xs:restriction base=\"xs:string\"/></xs:complexContent>")),
        ["xsd.duplicate-child"]
    );
    assert_eq!(
        problems(&ct("<xs:simpleContent><xs:extension base=\"xs:string\"><xs:minLength value=\"1\"/></xs:extension></xs:simpleContent>")),
        ["xsd.unexpected-element"]
    );
    assert_eq!(
        problems(&ct("<xs:simpleContent><xs:restriction base=\"xs:string\"><xs:minLength value=\"1\"/></xs:restriction></xs:simpleContent>")),
        Vec::<&str>::new()
    );
    assert_eq!(problems("<xs:complexType name=\"t\" mixed=\"yes\"/>"), ["xsd.bad-attribute-value"]);
    assert_eq!(problems("<xs:complexType name=\"t\" block=\"list\"/>"), ["xsd.bad-attribute-value"]);
    assert_eq!(problems("<xs:complexType/>"), ["xsd.missing-attribute"]);
    assert_eq!(problems("<xs:element name=\"e\"><xs:complexType name=\"t\"/></xs:element>"), ["xsd.unknown-attribute"]);
    assert_eq!(problems("<xs:element name=\"e\"><xs:complexType abstract=\"true\"/></xs:element>"), ["xsd.unknown-attribute"]);
}

#[test]
fn particles() {
    let items = items(
        "<xs:group name=\"g\"><xs:choice><xs:element name=\"a\" minOccurs=\"0\" maxOccurs=\"3\"/><xs:element name=\"b\" maxOccurs=\"0\" minOccurs=\"0\"/></xs:choice></xs:group>\
         <xs:complexType name=\"t\"><xs:sequence minOccurs=\"2\" maxOccurs=\"5\"><xs:group ref=\"tns:g\" maxOccurs=\"unbounded\"/><xs:any minOccurs=\"0\"/></xs:sequence></xs:complexType>",
    );
    let Item::Group(g) = &items[0] else { panic!("a group") };
    let Some(Particle { term: Term::Choice(options), min_occurs: 1, max_occurs: MaxOccurs::Bounded(1), .. }) = &g.content else { panic!("a choice") };
    assert_eq!((options[0].min_occurs, options[0].max_occurs), (0, MaxOccurs::Bounded(3)));
    assert_eq!((options[1].min_occurs, options[1].max_occurs), (0, MaxOccurs::Bounded(0)));
    let Item::ComplexType(t) = &items[1] else { panic!("a complex type") };
    let ComplexContentDef::Implicit { particle: Some(p), .. } = &t.content else { panic!("content") };
    assert_eq!((p.min_occurs, p.max_occurs), (2, MaxOccurs::Bounded(5)));
    let Term::Sequence(parts) = &p.term else { panic!("a sequence") };
    assert!(matches!(&parts[0].term, Term::GroupRef(q) if q.local == "g") && parts[0].max_occurs == MaxOccurs::Unbounded);
    assert!(matches!(&parts[1].term, Term::Any(w) if w.namespace == NamespaceConstraint::Any && w.process_contents == ProcessContents::Strict));
    // Mistakes.
    let seq =
        |attributes: &str| format!("<xs:complexType name=\"t\"><xs:sequence {attributes}><xs:element name=\"a\"/></xs:sequence></xs:complexType>");
    assert_eq!(problems(&seq("minOccurs=\"3\" maxOccurs=\"2\"")), ["xsd.min-occurs-over-max-occurs"]);
    assert_eq!(problems(&seq("minOccurs=\"-1\"")), ["xsd.bad-attribute-value"]);
    assert_eq!(problems(&seq("maxOccurs=\"many\"")), ["xsd.bad-attribute-value"]);
    assert_eq!(problems(&seq("minOccurs=\"1.5\"")), ["xsd.bad-attribute-value"]);
    assert_eq!(problems(&seq("minOccurs=\"0\" maxOccurs=\"0\"")), Vec::<&str>::new());
    assert_eq!(problems(&seq("minOccurs=\" 2 \" maxOccurs=\" unbounded \"")), Vec::<&str>::new());
    assert_eq!(problems("<xs:group name=\"g\"><xs:sequence minOccurs=\"1\"/></xs:group>"), ["xsd.unknown-attribute"]);
    assert_eq!(problems("<xs:group name=\"g\"/>"), ["xsd.missing-content"]);
    assert_eq!(problems("<xs:group name=\"g\"><xs:element name=\"a\"/></xs:group>"), ["xsd.missing-content", "xsd.unexpected-element"]);
    assert_eq!(problems("<xs:complexType name=\"t\"><xs:group/></xs:complexType>"), ["xsd.missing-attribute"]);
    assert_eq!(problems("<xs:complexType name=\"t\"><xs:all><xs:group ref=\"tns:g\"/></xs:all></xs:complexType>"), ["xsd.unexpected-element"]);
    assert_eq!(
        problems("<xs:complexType name=\"t\"><xs:sequence><xs:any namespace=\"##any ##other\"/></xs:sequence></xs:complexType>"),
        ["xsd.bad-attribute-value"]
    );
    assert_eq!(
        problems("<xs:complexType name=\"t\"><xs:sequence><xs:any processContents=\"never\"/></xs:sequence></xs:complexType>"),
        ["xsd.bad-attribute-value"]
    );
    assert_eq!(
        problems("<xs:complexType name=\"t\"><xs:sequence><xs:any namespace=\"##foo\"/></xs:sequence></xs:complexType>"),
        ["xsd.bad-attribute-value"]
    );
}

#[test]
fn elements() {
    let items = items(
        "<xs:element name=\"a\" type=\"xs:string\" default=\"d\" nillable=\"true\" abstract=\"true\" block=\"substitution\" final=\"extension\" substitutionGroup=\"tns:h\"/>\
         <xs:element name=\"b\" fixed=\"f\"><xs:complexType/><xs:unique name=\"u\"><xs:selector xpath=\".//p\"/><xs:field xpath=\"@id\"/><xs:field xpath=\"q\"/></xs:unique>\
           <xs:key name=\"k\"><xs:selector xpath=\"x\"/><xs:field xpath=\"y\"/></xs:key><xs:keyref name=\"r\" refer=\"tns:k\"><xs:selector xpath=\"x\"/><xs:field xpath=\"y\"/></xs:keyref></xs:element>\
         <xs:complexType name=\"t\"><xs:sequence><xs:element name=\"l\" form=\"qualified\" nillable=\"1\" block=\"#all\" type=\"xs:int\"/><xs:element ref=\"tns:a\" minOccurs=\"0\"/></xs:sequence></xs:complexType>",
    );
    let Item::Element(a) = &items[0] else { panic!("an element") };
    assert_eq!((a.name.as_deref(), a.nillable, a.abstract_), (Some("a"), Some(true), Some(true)));
    assert_eq!(a.default.as_ref().map(|d| d.value.as_str()), Some("d"));
    assert_eq!(a.type_name, Some(QName::new(Some(XS), "string")));
    assert_eq!(a.substitution_group, Some(QName::new(Some("urn:t"), "h")));
    assert_eq!(a.final_, Some(DerivationSet { extension: true, ..DerivationSet::default() }));
    let Item::Element(b) = &items[1] else { panic!("an element") };
    assert!(matches!(b.inline_type.as_deref(), Some(InlineType::Complex(_))));
    assert_eq!(b.fixed.as_ref().map(|d| d.value.as_str()), Some("f"));
    let kinds: Vec<_> = b.identity_constraints.iter().map(|c| (c.kind, c.name.as_deref(), c.fields.len())).collect();
    assert_eq!(kinds, [(IdentityKind::Unique, Some("u"), 2), (IdentityKind::Key, Some("k"), 1), (IdentityKind::Keyref, Some("r"), 1)]);
    assert_eq!(b.identity_constraints[0].selector.as_ref().map(|s| s.0.as_str()), Some(".//p"));
    assert_eq!(b.identity_constraints[2].refer, Some(QName::new(Some("urn:t"), "k")));
    let Item::ComplexType(t) = &items[2] else { panic!("a complex type") };
    let ComplexContentDef::Implicit { particle: Some(Particle { term: Term::Sequence(parts), .. }), .. } = &t.content else { panic!("content") };
    assert!(matches!(&parts[0].term, Term::Element(e) if e.form == Some(Form::Qualified) && e.nillable == Some(true)));
    assert!(matches!(&parts[1].term, Term::Element(e) if e.reference.is_some() && e.name.is_none()) && parts[1].min_occurs == 0);
}

#[test]
fn mistakes_in_elements() {
    assert_eq!(problems("<xs:element/>"), ["xsd.missing-attribute"]);
    assert_eq!(problems("<xs:element name=\"e\" ref=\"tns:e\"/>"), ["xsd.unknown-attribute"], "ref is for local elements");
    assert_eq!(problems("<xs:element name=\"e\" default=\"a\" fixed=\"b\"/>"), ["xsd.conflicting-attributes"]);
    assert_eq!(
        problems("<xs:element name=\"e\" type=\"xs:int\"><xs:simpleType><xs:restriction base=\"xs:int\"/></xs:simpleType></xs:element>"),
        ["xsd.conflicting-attributes"]
    );
    assert_eq!(
        problems("<xs:element name=\"e\"><xs:simpleType><xs:restriction base=\"xs:int\"/></xs:simpleType><xs:complexType/></xs:element>"),
        ["xsd.duplicate-child"]
    );
    assert_eq!(problems("<xs:element name=\"e\" minOccurs=\"0\"/>"), ["xsd.unknown-attribute"]);
    assert_eq!(problems("<xs:element name=\"e\" form=\"qualified\"/>"), ["xsd.unknown-attribute"]);
    assert_eq!(problems("<xs:element name=\"e\" nillable=\"maybe\"/>"), ["xsd.bad-attribute-value"]);
    assert_eq!(problems("<xs:element name=\"e\" block=\"list\"/>"), ["xsd.bad-attribute-value"]);
    assert_eq!(problems("<xs:element name=\"e\" final=\"substitution\"/>"), ["xsd.bad-attribute-value"]);
    assert_eq!(problems("<xs:element name=\"e\"><xs:annotation/><xs:annotation/></xs:element>"), ["xsd.duplicate-child"]);
    let local = |a: &str| format!("<xs:complexType name=\"t\"><xs:sequence><xs:element {a}/></xs:sequence></xs:complexType>");
    assert_eq!(problems(&local("")), ["xsd.missing-attribute"]);
    assert_eq!(problems(&local("name=\"a\" ref=\"tns:a\"")), ["xsd.conflicting-attributes"]);
    assert_eq!(problems(&local("ref=\"tns:a\" type=\"xs:int\"")), ["xsd.conflicting-attributes"]);
    assert_eq!(problems(&local("ref=\"tns:a\" nillable=\"true\"")), ["xsd.conflicting-attributes"]);
    assert_eq!(problems(&local("ref=\"tns:a\" default=\"x\"")), ["xsd.conflicting-attributes"]);
    assert_eq!(problems(&local("ref=\"tns:a\" form=\"qualified\"")), ["xsd.conflicting-attributes"]);
    assert_eq!(problems(&local("ref=\"tns:a\" block=\"#all\"")), ["xsd.conflicting-attributes"]);
    assert_eq!(problems(&local("ref=\"tns:a\"")), Vec::<&str>::new());
    assert_eq!(problems(&local("name=\"a\" substitutionGroup=\"tns:b\"")), ["xsd.unknown-attribute"]);
    assert_eq!(problems(&local("name=\"a\" abstract=\"true\"")), ["xsd.unknown-attribute"]);
    // Identity constraints.
    let ic = |inner: &str| format!("<xs:element name=\"e\">{inner}</xs:element>");
    assert_eq!(problems(&ic("<xs:key><xs:selector xpath=\"x\"/><xs:field xpath=\"y\"/></xs:key>")), ["xsd.missing-attribute"]);
    assert_eq!(problems(&ic("<xs:key name=\"k\"><xs:field xpath=\"y\"/></xs:key>")), ["xsd.missing-content"]);
    assert_eq!(problems(&ic("<xs:key name=\"k\"><xs:selector xpath=\"x\"/></xs:key>")), ["xsd.missing-content"]);
    assert_eq!(problems(&ic("<xs:key name=\"k\"><xs:selector/><xs:field xpath=\"y\"/></xs:key>")), ["xsd.missing-attribute"]);
    assert_eq!(problems(&ic("<xs:keyref name=\"k\"><xs:selector xpath=\"x\"/><xs:field xpath=\"y\"/></xs:keyref>")), ["xsd.missing-attribute"]);
    assert_eq!(
        problems(&ic("<xs:key name=\"k\" refer=\"tns:k\"><xs:selector xpath=\"x\"/><xs:field xpath=\"y\"/></xs:key>")),
        ["xsd.unknown-attribute"]
    );
    assert_eq!(
        problems(&ic("<xs:unique name=\"k\"><xs:selector xpath=\"x\"/><xs:selector xpath=\"x\"/><xs:field xpath=\"y\"/></xs:unique>")),
        ["xsd.duplicate-child"]
    );
}

#[test]
fn attributes_and_attribute_groups() {
    let items = items(
        "<xs:attribute name=\"a\" type=\"xs:int\" default=\"1\"/><xs:attribute name=\"b\" fixed=\"x\"><xs:simpleType><xs:restriction base=\"xs:string\"/></xs:simpleType></xs:attribute>\
         <xs:attributeGroup name=\"g\"><xs:attribute name=\"p\" use=\"required\" form=\"qualified\"/><xs:attribute ref=\"tns:a\" use=\"prohibited\"/><xs:attributeGroup ref=\"tns:h\"/><xs:anyAttribute/></xs:attributeGroup>",
    );
    let Item::Attribute(a) = &items[0] else { panic!("an attribute") };
    assert_eq!((a.name.as_deref(), a.default.as_ref().map(|d| d.value.as_str())), (Some("a"), Some("1")));
    let Item::Attribute(b) = &items[1] else { panic!("an attribute") };
    assert!(b.inline_type.is_some() && b.fixed.is_some());
    let Item::AttributeGroup(g) = &items[2] else { panic!("an attribute group") };
    assert_eq!((g.uses.attributes.len(), g.uses.groups.len(), g.uses.wildcard.is_some()), (2, 1, true));
    assert_eq!((g.uses.attributes[0].use_, g.uses.attributes[0].form), (Some(AttributeUse::Required), Some(Form::Qualified)));
    assert_eq!(g.uses.attributes[1].reference, Some(QName::new(Some("urn:t"), "a")));
    assert_eq!(problems("<xs:attribute/>"), ["xsd.missing-attribute"]);
    assert_eq!(problems("<xs:attribute name=\"a\" use=\"optional\"/>"), ["xsd.unknown-attribute"]);
    assert_eq!(problems("<xs:attribute name=\"a\" ref=\"tns:a\"/>"), ["xsd.unknown-attribute"]);
    assert_eq!(problems("<xs:attribute name=\"xmlns\"/>"), ["xsd.bad-attribute-value"]);
    assert_eq!(problems("<xs:attribute name=\"a\" default=\"1\" fixed=\"2\"/>"), ["xsd.conflicting-attributes"]);
    assert_eq!(
        problems("<xs:attribute name=\"a\" type=\"xs:int\"><xs:simpleType><xs:restriction base=\"xs:int\"/></xs:simpleType></xs:attribute>"),
        ["xsd.conflicting-attributes"]
    );
    let local = |a: &str| format!("<xs:attributeGroup name=\"g\"><xs:attribute {a}/></xs:attributeGroup>");
    assert_eq!(problems(&local("name=\"a\" ref=\"tns:a\"")), ["xsd.conflicting-attributes"]);
    assert_eq!(problems(&local("ref=\"tns:a\" type=\"xs:int\"")), ["xsd.conflicting-attributes"]);
    assert_eq!(problems(&local("ref=\"tns:a\" form=\"qualified\"")), ["xsd.conflicting-attributes"]);
    assert_eq!(problems(&local("name=\"a\" use=\"optional\" default=\"x\"")), Vec::<&str>::new());
    assert_eq!(problems(&local("name=\"a\" use=\"required\" default=\"x\"")), ["xsd.conflicting-attributes"]);
    assert_eq!(problems(&local("name=\"a\" use=\"sometimes\"")), ["xsd.bad-attribute-value"]);
    assert_eq!(problems("<xs:attributeGroup name=\"g\"><xs:attributeGroup/></xs:attributeGroup>"), ["xsd.missing-attribute"]);
    assert_eq!(problems("<xs:attributeGroup/>"), ["xsd.missing-attribute"]);
}

#[test]
fn notations() {
    let items = items("<xs:notation name=\"n\" public=\"-//X//N//EN\" system=\"n.dtd\"/><xs:notation name=\"m\" system=\"m\"/>");
    assert!(matches!(&items[0], Item::Notation(n) if n.public.as_deref() == Some("-//X//N//EN") && n.system.as_deref() == Some("n.dtd")));
    assert_eq!(problems("<xs:notation name=\"n\"/>"), ["xsd.missing-attribute"]);
    assert_eq!(problems("<xs:notation public=\"p\"/>"), ["xsd.missing-attribute"]);
}

#[test]
fn annotations() {
    let r = schema(
        "",
        "<xs:annotation id=\"a1\"><xs:documentation source=\"http://x\" xml:lang=\"en\">Hello <b>big</b> world &amp; more</xs:documentation><xs:documentation>second</xs:documentation><xs:appinfo>anything <x:y xmlns:x=\"urn:x\"/></xs:appinfo></xs:annotation>\
         <xs:element name=\"e\"><xs:annotation><xs:documentation>on e</xs:documentation></xs:annotation></xs:element>",
    );
    assert!(r.diagnostics.is_empty(), "{:?}", messages(&r));
    let doc = r.document.expect("a schema");
    let a = &doc.annotations[0];
    assert_eq!(a.documentation.len(), 2);
    assert_eq!((a.documentation[0].source.as_deref(), a.documentation[0].lang.as_deref()), (Some("http://x"), Some("en")));
    assert_eq!(a.documentation[0].text, "Hello big world & more");
    assert_eq!(a.appinfo, 1);
    let Item::Element(e) = &doc.items[0] else { panic!("an element") };
    assert_eq!(e.annotation.as_ref().map(|a| a.documentation[0].text.as_str()), Some("on e"));
    assert_eq!(problems("<xs:annotation><xs:element name=\"e\"/></xs:annotation>"), ["xsd.unexpected-element"]);
    assert_eq!(problems("<xs:annotation><xs:documentation bogus=\"1\"/></xs:annotation>"), ["xsd.unknown-attribute"]);
    assert_eq!(problems("<xs:annotation foo=\"1\"/>"), ["xsd.unknown-attribute"]);
}

#[test]
fn the_rest_of_the_mistakes_a_document_can_hold() {
    assert_eq!(problems("<xs:bogus/>"), ["xsd.unexpected-element"]);
    assert_eq!(problems("<other:thing xmlns:other=\"urn:o\"/>"), ["xsd.unexpected-element"]);
    assert_eq!(problems("text"), ["xsd.unexpected-text"]);
    assert_eq!(problems("<xs:element name=\"e\">text</xs:element>"), ["xsd.unexpected-text"]);
    assert_eq!(problems("<xs:element name=\"e\" foo=\"1\"/>"), ["xsd.unknown-attribute"]);
    assert_eq!(problems("<xs:element name=\"e\" xs:foo=\"1\"/>"), ["xsd.unknown-attribute"]);
    // Attributes of other namespaces are fine anywhere.
    assert_eq!(problems("<xs:element name=\"e\" xmlns:x=\"urn:x\" x:foo=\"1\"/>"), Vec::<&str>::new());
    assert_eq!(problems("<xs:element id=\"x\" name=\"a\"/><xs:element id=\"x\" name=\"b\"/>"), ["xsd.duplicate-id"]);
    assert_eq!(problems("<xs:element id=\"1x\" name=\"a\"/>"), ["xsd.bad-attribute-value"]);
    assert_eq!(problems("<xs:element name=\"1x\"/>"), ["xsd.bad-attribute-value"]);
    assert_eq!(problems("<xs:element name=\"a:b\"/>"), ["xsd.bad-attribute-value"]);
    // Both the schema element and its children are checked.
    let r = read(&format!("<xs:schema xmlns:xs=\"{XS}\" bogus=\"1\" elementFormDefault=\"sometimes\"><xs:element/></xs:schema>"), "");
    assert_eq!(keys(&r), ["xsd.unknown-attribute", "xsd.bad-attribute-value", "xsd.missing-attribute"]);
}

#[test]
fn a_document_that_is_not_well_formed_is_read_as_far_as_it_goes() {
    let r = read(&format!("<xs:schema xmlns:xs=\"{XS}\"><xs:element name=\"a\"><xs:element name=\"b\"></xs:schema>"), "");
    assert!(r.document.is_some());
    assert!(keys(&r).iter().any(|k| k.starts_with("xml.")), "{:?}", keys(&r));
    let r = read("<xs:schema", "");
    assert!(r.document.is_none() || !r.diagnostics.is_empty());
    // Diagnostics are in the order of the text.
    let r = read(&format!("<xs:schema xmlns:xs=\"{XS}\" b=\"1\"><xs:element a=\"2\" name=\"x\"/></xs:schema>"), "");
    let starts: Vec<usize> = r.diagnostics.iter().map(|d| d.span.start).collect();
    assert!(starts.windows(2).all(|w| w[0] <= w[1]), "{starts:?}");
}

#[test]
fn spans_point_at_what_is_meant() {
    let text = format!("<xs:schema xmlns:xs=\"{XS}\"><xs:element name=\"ok\" bogus=\"1\"/></xs:schema>");
    let r = read(&text, "");
    let d = &r.diagnostics[0];
    assert_eq!(&text[d.span.clone()], "bogus");
    let doc = r.document.expect("a schema");
    let Item::Element(e) = &doc.items[0] else { panic!("an element") };
    assert_eq!(&text[e.span.clone()], "<xs:element name=\"ok\" bogus=\"1\"/>");
    assert_eq!(&text[e.name_span.clone()], "ok");
}

#[test]
fn the_schema_language_may_be_the_default_namespace() {
    let text = format!(
        "<schema xmlns=\"{XS}\" targetNamespace=\"urn:t\" xmlns:t=\"urn:t\"><element name=\"a\" type=\"string\"/><complexType name=\"c\"><sequence><element ref=\"t:a\"/></sequence></complexType></schema>"
    );
    let r = read(&text, "");
    assert!(r.diagnostics.is_empty(), "{:?}", messages(&r));
    let doc = r.document.expect("a schema");
    let Item::Element(a) = &doc.items[0] else { panic!("an element") };
    assert_eq!(a.type_name, Some(QName::new(Some(XS), "string")), "an unprefixed type is in the default namespace, which is the schema's");
    assert!(matches!(&doc.items[1], Item::ComplexType(_)));
}

#[test]
fn an_inner_declaration_of_a_prefix_wins() {
    let r = read(
        &format!(
            "<xs:schema xmlns:xs=\"{XS}\" xmlns:p=\"urn:a\"><xs:simpleType name=\"t\" xmlns:p=\"urn:b\"><xs:restriction base=\"p:x\"/></xs:simpleType><xs:simpleType name=\"u\"><xs:restriction base=\"p:x\"/></xs:simpleType></xs:schema>"
        ),
        "",
    );
    let items = r.document.expect("a schema").items;
    let base = |i: usize| match &items[i] {
        Item::SimpleType(SimpleTypeDef { kind: SimpleTypeKind::Restriction { base, .. }, .. }) => base.clone(),
        _ => None,
    };
    assert_eq!((base(0), base(1)), (Some(QName::new(Some("urn:b"), "x")), Some(QName::new(Some("urn:a"), "x"))));
}

#[test]
fn documentation_may_be_cdata_and_elements_of_other_namespaces_are_not_ours() {
    let r = schema("", "<xs:annotation><xs:documentation><![CDATA[a < b & c]]> and <i>more</i></xs:documentation></xs:annotation>");
    assert!(r.diagnostics.is_empty(), "{:?}", messages(&r));
    assert_eq!(r.document.expect("a schema").annotations[0].documentation[0].text, "a < b & c and more");
    // An element with the name of one of ours, in another namespace, is not ours.
    assert_eq!(problems("<o:element xmlns:o=\"urn:o\" name=\"x\"/>"), ["xsd.unexpected-element"]);
    assert_eq!(problems("<xs:element name=\"e\"><o:complexType xmlns:o=\"urn:o\"/></xs:element>"), ["xsd.unexpected-element"]);
}
