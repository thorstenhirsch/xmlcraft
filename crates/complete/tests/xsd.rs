//! What an XML Schema allows at a place in a document, and the text to put for it.

use xmlcraft_complete::{Choices, ElementChoice, xsd_attribute_snippet, xsd_choices_at, xsd_element_snippet};
use xmlcraft_syntax::parse;
use xmlcraft_xsd::schema::Schema;
use xmlcraft_xsd::{NoLoader, XSD_NAMESPACE, assemble, load};

const NS: &str = "urn:o";

/// A schema for the namespace `urn:o`, with the elements written as local ones (`form`: qualified or not) and the types as given.
fn schema(form: &str, body: &str) -> Schema {
    schema_with(form, "unqualified", body)
}

fn schema_with(form: &str, attribute_form: &str, body: &str) -> Schema {
    let text = format!(
        "<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" xmlns:o=\"{NS}\" targetNamespace=\"{NS}\" elementFormDefault=\"{form}\" attributeFormDefault=\"{attribute_form}\">{body}</xs:schema>"
    );
    let documents = load(&text, "/s/o.xsd", &mut NoLoader);
    assert!(documents.diagnostics.is_empty(), "{:?}", documents.diagnostics.iter().map(|d| d.diagnostic.message()).collect::<Vec<_>>());
    let assembly = assemble(&documents);
    assert!(assembly.diagnostics.is_empty(), "{:?}", assembly.diagnostics.iter().map(|d| d.diagnostic.message()).collect::<Vec<_>>());
    assembly.schema
}

/// The choices at the place marked `|` in `text`.
fn at(schema: &Schema, marked: &str) -> Choices {
    let offset = marked.find('|').expect("a marker");
    let text = marked.replacen('|', "", 1);
    xsd_choices_at(&parse(&text).syntax(), offset, schema)
}

fn names(c: &Choices) -> Vec<(String, bool)> {
    c.elements.iter().map(|e| (e.name.clone(), e.mandatory)).collect()
}

fn pair(n: &str, m: bool) -> (String, bool) {
    (n.to_owned(), m)
}

const ORDER: &str = "<xs:element name=\"order\"><xs:complexType><xs:sequence>\
        <xs:element name=\"customer\" type=\"xs:string\"/>\
        <xs:element name=\"item\" maxOccurs=\"unbounded\"><xs:complexType>\
          <xs:sequence><xs:element name=\"note\" type=\"xs:string\" minOccurs=\"0\"/></xs:sequence>\
          <xs:attribute name=\"sku\" type=\"xs:string\" use=\"required\"/>\
          <xs:attribute name=\"kind\" use=\"optional\"><xs:simpleType><xs:restriction base=\"xs:string\"><xs:enumeration value=\"pen\"/><xs:enumeration value=\"ink\"/></xs:restriction></xs:simpleType></xs:attribute>\
          <xs:attribute name=\"gift\" type=\"xs:boolean\"/>\
        </xs:complexType></xs:element>\
        <xs:element name=\"total\" type=\"xs:decimal\" minOccurs=\"0\"/>\
      </xs:sequence><xs:attribute name=\"id\" type=\"xs:ID\" use=\"required\"/></xs:complexType></xs:element>";

#[test]
fn the_root_element_is_one_of_the_global_elements() {
    let s = schema(
        "qualified",
        &format!("{ORDER}<xs:element name=\"memo\" type=\"xs:string\"/><xs:element name=\"hidden\" abstract=\"true\" type=\"xs:string\"/>"),
    );
    let before = at(&s, "|");
    assert_eq!(names(&before), [pair("order", false), pair("memo", false)], "an abstract element cannot be a root");
    // A root of a namespace needs the declaration of it (the default one is free).
    assert_eq!(before.elements.iter().map(|e| e.declare.clone()).collect::<Vec<_>>(), vec![Some((None, NS.to_owned())); 2]);
    assert_eq!(names(&at(&s, "|<order xmlns=\"urn:o\"/>")).len(), 2, "before the root of a document that has one");
}

#[test]
fn the_content_model_of_the_type_is_followed_through_the_children_before_the_position() {
    let s = schema("qualified", ORDER);
    let doc = |inside: &str| format!("<order xmlns=\"urn:o\" id=\"o1\">{inside}</order>");
    assert_eq!(names(&at(&s, &doc("|"))), [pair("customer", true)]);
    assert_eq!(names(&at(&s, &doc("<customer/>|"))), [pair("item", true)]);
    assert_eq!(names(&at(&s, &doc("<customer/><item sku=\"1\"/>|"))), [pair("item", false), pair("total", false)]);
    assert_eq!(names(&at(&s, &doc("<customer/><item sku=\"1\"/><total/>|"))), Vec::<(String, bool)>::new(), "the content is complete");
    // Between children, only those before the position count.
    assert_eq!(names(&at(&s, &doc("<customer/>|<item sku=\"1\"/>"))), [pair("item", true)]);
    // A child that does not fit ends the following.
    assert_eq!(names(&at(&s, &doc("<total/><customer/>|"))), [pair("customer", true)]);
    assert_eq!(at(&s, &doc("<customer/>|")).parent.as_deref(), Some("order"));
    assert!(at(&s, &doc("<customer/>|")).elements.iter().all(|e| e.id.is_some() && !e.empty));
}

#[test]
fn a_child_is_typed_by_the_model_of_its_parent() {
    let s = schema("qualified", ORDER);
    // Inside an item: its own content model (a `note`), not that of the order.
    let doc = "<order xmlns=\"urn:o\" id=\"o1\"><customer/><item sku=\"1\">|</item></order>";
    assert_eq!(names(&at(&s, doc)), [pair("note", false)]);
    assert_eq!(at(&s, doc).parent.as_deref(), Some("item"));
    // The second item is the same type, after the first.
    let second = "<order xmlns=\"urn:o\" id=\"o1\"><customer/><item sku=\"1\"/><item sku=\"2\"><note/>|</item></order>";
    assert_eq!(names(&at(&s, second)), Vec::<(String, bool)>::new());
    // An element that is not where the model allows it is not typed: nothing is offered in it.
    let stray = "<order xmlns=\"urn:o\" id=\"o1\"><total/><item sku=\"1\">|</item></order>";
    assert!(at(&s, stray).elements.is_empty());
    // A sibling before it that the model does not take leaves it unknown what the model says about this one.
    let misfit = "<order xmlns=\"urn:o\" id=\"o1\"><customer/><bogus/><item sku=\"1\">|</item></order>";
    assert!(at(&s, misfit).elements.is_empty());
    // And one that is not declared at all.
    assert!(at(&s, "<order xmlns=\"urn:o\"><nope>|</nope></order>").elements.is_empty());
    assert!(at(&s, "<elsewhere>|</elsewhere>").elements.is_empty());
}

#[test]
fn the_attributes_of_the_type_say_which_are_required_used_and_what_values_they_take() {
    let s = schema("qualified", ORDER);
    let c = at(&s, "<order xmlns=\"urn:o\" id=\"o1\"><customer/><item |/></order>");
    assert!(c.in_start_tag && c.elements.is_empty(), "in a start tag only attributes are offered");
    let by_name: Vec<_> = c.attributes.iter().map(|a| (a.name.as_str(), a.mandatory, a.used, a.values.clone())).collect();
    assert_eq!(
        by_name,
        [
            ("sku", true, false, Vec::<String>::new()),
            ("kind", false, false, vec!["pen".to_owned(), "ink".to_owned()]),
            ("gift", false, false, vec!["true".to_owned(), "false".to_owned()])
        ]
    );
    // One that the start tag has is used; where a new one goes is after the last.
    let given = at(&s, "<order xmlns=\"urn:o\" id=\"o1\"><customer/><item sku=\"1\" |/></order>");
    assert_eq!(given.attributes.iter().filter(|a| a.used).map(|a| a.name.as_str()).collect::<Vec<_>>(), ["sku"]);
    let text = "<order xmlns=\"urn:o\" id=\"o1\"><customer/><item sku=\"1\" /></order>";
    assert_eq!(given.attribute_at.and_then(|at| text.get(at..at + 2)), Some(" /"), "after `sku=\"1\"`, where the space before the slash is");
    // The attributes of the element the position is in the content of are offered there too.
    let inside = at(&s, "<order xmlns=\"urn:o\" id=\"o1\">|</order>");
    assert_eq!(inside.element.as_deref(), Some("order"));
    assert_eq!(inside.attributes.iter().map(|a| (a.name.as_str(), a.used)).collect::<Vec<_>>(), [("id", true)]);
}

#[test]
fn the_text_of_an_element_has_the_values_its_type_lists() {
    let s = schema(
        "qualified",
        "<xs:element name=\"r\"><xs:complexType><xs:sequence>\
           <xs:element name=\"state\"><xs:simpleType><xs:restriction base=\"xs:string\"><xs:enumeration value=\"open\"/><xs:enumeration value=\"closed\"/></xs:restriction></xs:simpleType></xs:element>\
           <xs:element name=\"flag\" type=\"xs:boolean\"/>\
           <xs:element name=\"either\"><xs:simpleType><xs:union><xs:simpleType><xs:restriction base=\"xs:string\"><xs:enumeration value=\"a\"/></xs:restriction></xs:simpleType><xs:simpleType><xs:restriction base=\"xs:string\"><xs:enumeration value=\"b\"/></xs:restriction></xs:simpleType></xs:union></xs:simpleType></xs:element>\
           <xs:element name=\"free\" type=\"xs:string\"/>\
           <xs:element name=\"plus\"><xs:complexType><xs:simpleContent><xs:extension base=\"xs:boolean\"/></xs:simpleContent></xs:complexType></xs:element>\
         </xs:sequence></xs:complexType></xs:element>",
    );
    let doc = |inside: &str| format!("<r xmlns=\"urn:o\">{inside}</r>");
    let values = |inside: &str| at(&s, &doc(inside)).values;
    let strings = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    assert_eq!(values("<state>|</state>"), strings(&["open", "closed"]));
    assert_eq!(values("<state/><flag>|</flag>"), strings(&["true", "false"]));
    assert_eq!(values("<state/><flag/><either>|</either>"), strings(&["a", "b"]), "the lists of the members of a union");
    assert_eq!(values("<state/><flag/><either/><free>|</free>"), Vec::<String>::new());
    assert_eq!(values("<state/><flag/><either/><free/><plus>|</plus>"), strings(&["true", "false"]), "simple content");
    // Where the text is of no such type, nothing; and it is not in an element that has children.
    assert!(at(&s, &doc("|")).values.is_empty());
}

#[test]
fn names_are_written_as_the_document_writes_them() {
    let body = "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"a\" type=\"xs:string\"/><xs:element name=\"b\" type=\"xs:string\"/></xs:sequence></xs:complexType></xs:element>";
    // Qualified locals with a prefix for the namespace.
    let q = schema("qualified", body);
    assert_eq!(names(&at(&q, "<o:r xmlns:o=\"urn:o\">|</o:r>")), [pair("o:a", true)]);
    // Qualified locals with the default namespace.
    assert_eq!(names(&at(&q, "<r xmlns=\"urn:o\">|</r>")), [pair("a", true)]);
    // Unqualified locals are in no namespace: under a default namespace they need `xmlns=""`.
    let u = schema("unqualified", body);
    let under_default = at(&u, "<r xmlns=\"urn:o\">|</r>");
    assert_eq!(names(&under_default), [pair("a", true)]);
    assert_eq!(
        under_default.elements.first().and_then(|e| e.declare.clone()),
        Some((None, String::new())),
        "`a` would be in urn:o without `xmlns=\"\"`"
    );
    let with_prefix = at(&u, "<o:r xmlns:o=\"urn:o\">|</o:r>");
    assert_eq!(names(&with_prefix), [pair("a", true)]);
    assert_eq!(with_prefix.elements.first().and_then(|e| e.declare.clone()), None, "no default namespace in scope: nothing to undeclare");
    let default_in_scope = at(&u, "<o:r xmlns:o=\"urn:o\" xmlns=\"urn:other\">|</o:r>");
    assert_eq!(
        default_in_scope.elements.first().and_then(|e| e.declare.clone()),
        Some((None, String::new())),
        "`xmlns=\"\"` puts it in no namespace"
    );
    // The root element before the document has any declaration: it declares the namespace.
    assert_eq!(at(&q, "|").elements.first().and_then(|e| e.declare.clone()), Some((None, NS.to_owned())));
}

#[test]
fn xsi_type_changes_the_type_of_the_element() {
    let s = schema(
        "qualified",
        "<xs:complexType name=\"base\"><xs:sequence><xs:element name=\"a\" type=\"xs:string\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"derived\"><xs:complexContent><xs:extension base=\"o:base\"><xs:sequence><xs:element name=\"b\" type=\"xs:string\"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>\
         <xs:element name=\"r\" type=\"o:base\"/>",
    );
    assert_eq!(names(&at(&s, "<r xmlns=\"urn:o\"><a/>|</r>")), Vec::<(String, bool)>::new());
    let derived = "<r xmlns=\"urn:o\" xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\" i:type=\"derived\"><a/>|</r>";
    assert_eq!(names(&at(&s, derived)), [pair("b", true)]);
    // A type that is not declared leaves the type of the element.
    let unknown = "<r xmlns=\"urn:o\" xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\" i:type=\"nope\">|</r>";
    assert_eq!(names(&at(&s, unknown)), [pair("a", true)]);
}

fn choice(c: &Choices, name: &str) -> ElementChoice {
    c.elements.iter().find(|e| e.name == name).cloned().unwrap_or_else(|| panic!("no choice {name}: {:?}", names(c)))
}

#[test]
fn an_element_is_written_with_what_it_must_have() {
    let s = schema("qualified", ORDER);
    let c = at(&s, "<order xmlns=\"urn:o\" id=\"o1\">|</order>");
    let item_place = at(&s, "<order xmlns=\"urn:o\" id=\"o1\"><customer/>|</order>");
    let snippet = |c: &Choices, name: &str, mandatory: bool| xsd_element_snippet(&s, &c.scope, &choice(c, name), mandatory);
    // A simple element: a tag pair, the caret in it.
    let customer = snippet(&c, "customer", true);
    assert_eq!((customer.text.as_str(), customer.caret), ("<customer></customer>", "<customer>".len()));
    // An element with a required attribute: the caret in its value; with the children it must have (none here) when asked.
    let item = snippet(&item_place, "item", true);
    assert_eq!((item.text.as_str(), item.caret), ("<item sku=\"\"></item>", "<item sku=\"".len()));
    // Without the required content the attributes are still written for the element itself.
    assert_eq!(snippet(&item_place, "item", false).text, "<item sku=\"\"></item>");
}

#[test]
fn the_children_an_element_must_have_are_written_in_it_and_in_them() {
    let s = schema(
        "qualified",
        "<xs:element name=\"r\"><xs:complexType><xs:sequence>\
           <xs:element name=\"head\"><xs:complexType><xs:sequence><xs:element name=\"title\" type=\"xs:string\"/><xs:element name=\"sub\" type=\"xs:string\" minOccurs=\"0\"/></xs:sequence><xs:attribute name=\"lang\" use=\"required\"><xs:simpleType><xs:restriction base=\"xs:string\"><xs:enumeration value=\"en\"/><xs:enumeration value=\"de\"/></xs:restriction></xs:simpleType></xs:attribute></xs:complexType></xs:element>\
           <xs:element name=\"fixed\" type=\"xs:string\" fixed=\"yes\"/>\
           <xs:element name=\"dflt\" type=\"xs:string\" default=\"d\"/>\
           <xs:element name=\"nothing\"><xs:complexType/></xs:element>\
           <xs:element name=\"pick\"><xs:complexType><xs:choice><xs:element name=\"x\" type=\"xs:string\"/><xs:sequence><xs:element name=\"y\" type=\"xs:string\"/><xs:element name=\"z\" type=\"xs:string\"/></xs:sequence></xs:choice></xs:complexType></xs:element>\
         </xs:sequence></xs:complexType></xs:element>",
    );
    let c = at(&s, "<r xmlns=\"urn:o\">|</r>");
    let snippet = |name: &str, mandatory: bool| {
        // Each is the choice at its place; an element that is not yet allowed is found from the root's own model.
        let place = match name {
            "head" => c.clone(),
            "fixed" => at(&s, "<r xmlns=\"urn:o\"><head lang=\"en\"><title/></head>|</r>"),
            "dflt" => at(&s, "<r xmlns=\"urn:o\"><head lang=\"en\"><title/></head><fixed/>|</r>"),
            "nothing" => at(&s, "<r xmlns=\"urn:o\"><head lang=\"en\"><title/></head><fixed/><dflt/>|</r>"),
            _ => at(&s, "<r xmlns=\"urn:o\"><head lang=\"en\"><title/></head><fixed/><dflt/><nothing/>|</r>"),
        };
        xsd_element_snippet(&s, &place.scope, &choice(&place, name), mandatory)
    };
    // The required children, and the required attribute with the first value it can have; the optional child is left out.
    let head = snippet("head", true);
    assert_eq!(head.text, "<head lang=\"en\"><title></title></head>");
    // The value is there already (the first the attribute can have), so the caret is in the content.
    assert_eq!(head.caret, "<head lang=\"en\">".len());
    assert_eq!(snippet("head", false).text, "<head lang=\"en\"></head>", "without the content, the attributes it needs are still there");
    // A fixed value is the content the element has to have.
    assert_eq!(snippet("fixed", true).text, "<fixed>yes</fixed>");
    assert_eq!(snippet("fixed", false).text, "<fixed></fixed>");
    // A default is not content the element has to have.
    assert_eq!(snippet("dflt", true).text, "<dflt></dflt>");
    // No content at all: an empty tag, the caret after the name.
    let nothing = snippet("nothing", true);
    assert_eq!((nothing.text.as_str(), nothing.caret), ("<nothing/>", "<nothing".len()), "before the slash");
    // The shortest way through a choice.
    assert_eq!(snippet("pick", true).text, "<pick><x></x></pick>");
}

#[test]
fn a_name_that_needs_a_namespace_declaration_brings_it() {
    let s = schema(
        "qualified",
        "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"a\" type=\"xs:string\"/></xs:sequence></xs:complexType></xs:element>",
    );
    // The root, put in an empty document: it declares the namespace, and its required content is in it.
    let root = at(&s, "|");
    let snippet = xsd_element_snippet(&s, &root.scope, &choice(&root, "r"), true);
    assert_eq!(snippet.text, "<r xmlns=\"urn:o\"><a></a></r>");
    // A prefix for it where the default namespace means something else.
    let other = at(&s, "<x xmlns=\"urn:other\">|</x>");
    assert!(other.elements.is_empty(), "x is not declared; there is nothing to put in it");
}

#[test]
fn what_is_offered_never_panics_whatever_the_text() {
    let s = schema("qualified", ORDER);
    let text = "<order xmlns=\"urn:o\" id=\"o1\"><customer/><item sku=\"1\"><note/></item><total>1</total></order><!-- é -->";
    // Every position of a document (and of broken ones made from it) gives an answer.
    for broken in
        [text.to_owned(), text.replace('>', ""), text.replace("</order>", ""), text.replace('<', "<<"), text.chars().rev().collect::<String>()]
    {
        let parsed = parse(&broken).syntax();
        for offset in (0..=broken.len()).filter(|o| broken.is_char_boundary(*o)) {
            let c = xsd_choices_at(&parsed, offset, &s);
            for e in &c.elements {
                let _ = xsd_element_snippet(&s, &c.scope, e, true);
            }
        }
    }
}

#[test]
fn a_required_cycle_is_cut_at_the_depth_and_what_is_written_is_balanced() {
    let s = schema(
        "qualified",
        "<xs:element name=\"n\"><xs:complexType><xs:sequence><xs:element ref=\"o:n\"/></xs:sequence></xs:complexType></xs:element>",
    );
    let root = at(&s, "|");
    let snippet = xsd_element_snippet(&s, &root.scope, &choice(&root, "n"), true);
    let opened = snippet.text.matches("<n").count();
    assert_eq!(opened, xmlcraft_complete::MAX_SNIPPET_DEPTH + 1, "{}", snippet.text);
    assert_eq!(snippet.text.matches("</n>").count(), opened, "every tag that was opened is closed");
    assert!(snippet.text.len() <= xmlcraft_complete::MAX_SNIPPET_BYTES);
}

#[test]
fn fixed_and_default_values_of_attributes_are_what_they_are_offered_and_written_with() {
    let s = schema(
        "qualified",
        "<xs:element name=\"r\"><xs:complexType>\
           <xs:attribute name=\"d\" default=\"dd\"><xs:simpleType><xs:restriction base=\"xs:string\"><xs:enumeration value=\"aa\"/><xs:enumeration value=\"dd\"/></xs:restriction></xs:simpleType></xs:attribute>\
           <xs:attribute name=\"f\" type=\"xs:string\" fixed=\"ff\"/>\
           <xs:attribute name=\"req\" type=\"xs:string\" use=\"required\" fixed=\"rr\"/>\
         </xs:complexType></xs:element>",
    );
    let c = at(&s, "<r xmlns=\"urn:o\" |/>");
    let values: Vec<_> = c.attributes.iter().map(|a| (a.name.as_str(), a.values.clone())).collect();
    // A default comes first, once; a fixed value is the only one.
    assert_eq!(values, [("d", vec!["dd".to_owned(), "aa".to_owned()]), ("f", vec!["ff".to_owned()]), ("req", vec!["rr".to_owned()])]);
    // What is written: the required attribute, with its fixed value.
    let root = at(&s, "|");
    assert_eq!(xsd_element_snippet(&s, &root.scope, &choice(&root, "r"), false).text, "<r xmlns=\"urn:o\" req=\"rr\"/>");
}

#[test]
fn a_qualified_attribute_needs_a_prefix_and_its_declaration() {
    let s = schema_with(
        "qualified",
        "qualified",
        "<xs:element name=\"r\"><xs:complexType><xs:attribute name=\"a\" type=\"xs:string\" use=\"required\"/></xs:complexType></xs:element>",
    );
    let root = at(&s, "|");
    assert_eq!(xsd_element_snippet(&s, &root.scope, &choice(&root, "r"), true).text, "<r xmlns=\"urn:o\" xmlns:ns1=\"urn:o\" ns1:a=\"\"/>");
    // Where the namespace has a prefix already, it is used and nothing is declared.
    let c = at(&s, "<o:r xmlns:o=\"urn:o\" |/>");
    assert_eq!(c.attributes.iter().map(|a| (a.name.as_str(), a.declare.clone())).collect::<Vec<_>>(), [("o:a", None)]);
    // `used` is by expanded name: the prefix does not matter.
    let used = at(&s, "<o:r xmlns:o=\"urn:o\" xmlns:p=\"urn:o\" p:a=\"1\" |/>");
    assert!(used.attributes.iter().all(|a| a.used), "{:?}", used.attributes);
    // Declarations are not attributes of the type.
    assert!(!at(&s, "<o:r xmlns:o=\"urn:o\" |/>").attributes.iter().any(|a| a.used));
}

#[test]
fn a_wildcard_takes_a_declared_element_and_an_undeclared_one_is_not_typed() {
    let s = schema(
        "qualified",
        "<xs:element name=\"any\"><xs:complexType><xs:sequence><xs:any processContents=\"lax\" minOccurs=\"0\" maxOccurs=\"unbounded\"/></xs:sequence></xs:complexType></xs:element>\
         <xs:element name=\"skip\"><xs:complexType><xs:sequence><xs:any processContents=\"skip\"/></xs:sequence></xs:complexType></xs:element>\
         <xs:element name=\"known\"><xs:complexType><xs:sequence><xs:element name=\"leaf\" type=\"xs:string\"/></xs:sequence></xs:complexType></xs:element>",
    );
    // A wildcard offers no names (any would do), but what is in it, if declared, is typed.
    assert!(at(&s, "<any xmlns=\"urn:o\">|</any>").elements.is_empty());
    assert_eq!(names(&at(&s, "<any xmlns=\"urn:o\"><known>|</known></any>")), [pair("leaf", true)]);
    assert!(at(&s, "<any xmlns=\"urn:o\"><stranger>|</stranger></any>").elements.is_empty());
    // A `skip` wildcard does not type what is in it, though the name is declared.
    assert!(at(&s, "<skip xmlns=\"urn:o\"><known>|</known></skip>").elements.is_empty());
}

#[test]
fn an_element_of_an_abstract_head_is_offered_through_its_substitutes() {
    let s = schema(
        "qualified",
        "<xs:element name=\"shape\" abstract=\"true\" type=\"xs:string\"/>\
         <xs:element name=\"circle\" substitutionGroup=\"o:shape\" type=\"xs:string\"/>\
         <xs:element name=\"square\" substitutionGroup=\"o:shape\" type=\"xs:string\"/>\
         <xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element ref=\"o:shape\"/></xs:sequence></xs:complexType></xs:element>",
    );
    let c = at(&s, "<r xmlns=\"urn:o\">|</r>");
    // None of the alternatives is mandatory by itself: one of them is.
    assert_eq!(names(&c), [pair("circle", false), pair("square", false)], "the abstract head itself cannot be written");
}

#[test]
fn the_prefix_an_attribute_needed_is_in_scope_for_the_children_that_follow() {
    let s = schema_with(
        "qualified",
        "qualified",
        "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"c\"><xs:complexType><xs:attribute name=\"a\" type=\"xs:string\" use=\"required\"/></xs:complexType></xs:element></xs:sequence>\
           <xs:attribute name=\"a\" type=\"xs:string\" use=\"required\"/></xs:complexType></xs:element>",
    );
    let root = at(&s, "|");
    assert_eq!(
        xsd_element_snippet(&s, &root.scope, &choice(&root, "r"), true).text,
        "<r xmlns=\"urn:o\" xmlns:ns1=\"urn:o\" ns1:a=\"\"><c ns1:a=\"\"/></r>",
        "the child uses the prefix its parent declared"
    );
}

#[test]
fn what_is_written_for_an_element_with_very_many_required_children_is_capped() {
    let children: String = (0..3000).map(|i| format!("<xs:element name=\"c{i:04}\" type=\"xs:string\"/>")).collect();
    let s =
        schema("qualified", &format!("<xs:element name=\"r\"><xs:complexType><xs:sequence>{children}</xs:sequence></xs:complexType></xs:element>"));
    let root = at(&s, "|");
    let snippet = xsd_element_snippet(&s, &root.scope, &choice(&root, "r"), true);
    assert!(snippet.text.len() < xmlcraft_complete::MAX_SNIPPET_BYTES + 64, "{} bytes", snippet.text.len());
    assert!(snippet.text.ends_with("</r>") && snippet.text.matches("<c").count() > 100, "it is cut, and still closed");
    assert_eq!(snippet.text.matches("<c").count(), snippet.text.matches("</c").count());
}

#[test]
fn an_attribute_is_written_with_the_declaration_its_name_needs() {
    let plain = xsd_attribute_snippet("kind", Some("pen"), None);
    assert_eq!((plain.text.as_str(), plain.caret), (" kind=\"pen\"", " kind=\"pen\"".len() - 1));
    let empty = xsd_attribute_snippet("kind", None, None);
    assert_eq!((empty.text.as_str(), empty.caret), (" kind=\"\"", " kind=\"".len()), "the caret is between the quotes");
    let declared = xsd_attribute_snippet("ns1:a", None, Some(&(Some("ns1".to_owned()), "urn:o".to_owned())));
    assert_eq!(declared.text, " xmlns:ns1=\"urn:o\" ns1:a=\"\"");
    assert_eq!(declared.caret, declared.text.len() - 1);
    // A value is escaped.
    assert_eq!(xsd_attribute_snippet("a", Some("x\"<&"), None).text, " a=\"x&quot;&lt;&amp;\"");
}
