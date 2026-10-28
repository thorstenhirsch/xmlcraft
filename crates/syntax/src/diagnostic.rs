//! Diagnostics: what is wrong with a document, and where.
//!
//! Every problem has a [`Code`]. The code fixes the stable key (for translation and for scripts), the severity and an English message
//! template; the arguments fill the template's `{0}`, `{1}`. The span is a byte range of the document.

use std::ops::Range;

/// How serious a problem is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

macro_rules! codes {
    ($($variant:ident => ($key:literal, $severity:ident, $message:literal)),* $(,)?) => {
        /// What kind of problem a diagnostic reports.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Code {
            $(#[doc = $message] $variant),*
        }

        impl Code {
            /// Every code.
            pub const ALL: &'static [Code] = &[$(Code::$variant),*];

            /// A stable identifier such as `xml.mismatched-end-tag`.
            pub fn key(self) -> &'static str {
                match self { $(Code::$variant => $key),* }
            }

            pub fn severity(self) -> Severity {
                match self { $(Code::$variant => Severity::$severity),* }
            }

            /// The English message, with `{0}`, `{1}` for the arguments.
            pub fn template(self) -> &'static str {
                match self { $(Code::$variant => $message),* }
            }
        }
    };
}

codes! {
    // Well-formedness: elements and tags
    MismatchedEndTag => ("xml.mismatched-end-tag", Error, "The end tag </{1}> does not match the start tag <{0}>."),
    UnmatchedEndTag => ("xml.unmatched-end-tag", Error, "There is no open element to close with </{0}>."),
    UnclosedElement => ("xml.unclosed-element", Error, "The element <{0}> is not closed."),
    UnterminatedTag => ("xml.unterminated-tag", Error, "The tag is not closed: expected '>'."),
    UnterminatedEndTag => ("xml.unterminated-end-tag", Error, "The end tag is not closed: expected '>'."),
    MissingEndTagName => ("xml.missing-end-tag-name", Error, "The end tag has no name."),
    InvalidName => ("xml.invalid-name", Error, "'{0}' is not a valid name."),
    // Well-formedness: attributes
    UnterminatedAttrValue => ("xml.unterminated-attribute-value", Error, "The attribute value is not closed: expected {0}."),
    AttrWithoutValue => ("xml.attribute-without-value", Error, "The attribute '{0}' has no value: expected =\"...\"."),
    MissingAttrValue => ("xml.missing-attribute-value", Error, "The attribute '{0}' has no value after '='."),
    UnquotedAttrValue => ("xml.unquoted-attribute-value", Error, "The attribute value must be in quotes."),
    MissingAttrName => ("xml.missing-attribute-name", Error, "A value needs an attribute name before it."),
    DuplicateAttribute => ("xml.duplicate-attribute", Error, "The attribute '{0}' is given more than once."),
    // Well-formedness: character data
    BareAmpersand => ("xml.bare-ampersand", Error, "'&' must start a reference; write '&amp;' for a literal ampersand."),
    BareLessThan => ("xml.bare-less-than", Error, "'<' must start a tag; write '&lt;' for a literal less-than sign."),
    CdataEndInContent => ("xml.cdata-end-in-content", Error, "']]>' is not allowed in text; write ']]&gt;'."),
    UnexpectedCharacter => ("xml.unexpected-character", Error, "Unexpected '{0}'."),
    InvalidChar => ("xml.invalid-character", Error, "The character U+{0} is not allowed in XML."),
    InvalidCharRef => ("xml.invalid-character-reference", Error, "The character reference '{0}' is not a character XML allows."),
    UndeclaredEntity => ("xml.undeclared-entity", Error, "The entity '{0}' is not declared."),
    EntityUnparsed => ("xml.unparsed-entity-reference", Error, "The entity '{0}' is unparsed (it has a notation), so it cannot be referenced in text."),
    EntityExternalInAttribute => ("xml.external-entity-in-attribute", Error, "The external entity '{0}' cannot be referenced in an attribute value."),
    EntityLtInAttribute => ("xml.less-than-in-attribute-entity", Error, "The replacement text of the entity '{0}' contains '<', which an attribute value cannot."),
    EntityNotWellFormed => ("xml.entity-not-well-formed", Error, "The replacement text of the entity '{0}' is not well-formed: {1}"),
    EntityExternalStandalone => ("xml.entity-declared-externally", Error, "The entity '{0}' is declared outside the document, but the document is standalone."),
    // Well-formedness: constructs
    UnterminatedComment => ("xml.unterminated-comment", Error, "The comment is not closed: expected '-->'."),
    DoubleHyphenInComment => ("xml.double-hyphen-in-comment", Error, "'--' is not allowed inside a comment."),
    UnterminatedCdata => ("xml.unterminated-cdata", Error, "The CDATA section is not closed: expected ']]>'."),
    UnterminatedPi => ("xml.unterminated-pi", Error, "The processing instruction is not closed: expected '?>'."),
    MissingPiTarget => ("xml.missing-pi-target", Error, "The processing instruction has no target name."),
    ReservedPiTarget => ("xml.reserved-pi-target", Error, "The processing instruction target '{0}' is reserved."),
    UnterminatedLiteral => ("xml.unterminated-literal", Error, "The quoted string is not closed: expected {0}."),
    MissingSpace => ("xml.missing-space", Error, "Whitespace is needed before '{0}'."),
    BadPubidLiteral => ("xml.bad-public-identifier", Error, "The character '{0}' is not allowed in a public identifier."),
    UnknownDeclaration => ("xml.unknown-declaration", Error, "'{0}' is not a markup declaration."),
    BadEntityValue => ("xml.bad-entity-value", Error, "In an entity value, '{0}' must start a reference."),
    PeInDeclaration => ("xml.parameter-entity-in-declaration", Error, "A parameter-entity reference cannot be used inside a declaration of the internal subset."),
    UnterminatedDecl => ("xml.unterminated-declaration", Error, "The declaration is not closed: expected '>'."),
    // Well-formedness: document structure
    ContentOutsideRoot => ("xml.content-outside-root", Error, "Text is not allowed outside the root element."),
    MultipleRoots => ("xml.multiple-roots", Error, "A document can have only one root element."),
    NoRootElement => ("xml.no-root-element", Error, "The document has no root element."),
    MisplacedXmlDecl => ("xml.misplaced-xml-declaration", Error, "The XML declaration must be at the very start of the document."),
    UnterminatedXmlDecl => ("xml.unterminated-xml-declaration", Error, "The XML declaration is not closed: expected '?>'."),
    MissingVersion => ("xml.missing-version", Error, "The XML declaration needs a version."),
    BadVersion => ("xml.bad-version", Error, "'{0}' is not a valid XML version; expected 1.0."),
    UnsupportedVersion => ("xml.unsupported-version", Warning, "The document is checked as XML 1.0; version '{0}' is not fully supported."),
    BadEncodingName => ("xml.bad-encoding-name", Error, "'{0}' is not a valid encoding name."),
    BadStandalone => ("xml.bad-standalone", Error, "The value of standalone must be \"yes\" or \"no\", not '{0}'."),
    PseudoAttrOrder => ("xml.declaration-attribute-order", Error, "In the XML declaration, '{0}' must come before '{1}'."),
    UnknownPseudoAttr => ("xml.unknown-declaration-attribute", Error, "'{0}' is not allowed in the XML declaration."),
    DuplicatePseudoAttr => ("xml.duplicate-declaration-attribute", Error, "'{0}' is given more than once in the XML declaration."),
    MisplacedDoctype => ("xml.misplaced-doctype", Error, "The DOCTYPE must come before the root element."),
    MultipleDoctype => ("xml.multiple-doctype", Error, "A document can have only one DOCTYPE."),
    MissingDoctypeName => ("xml.missing-doctype-name", Error, "The DOCTYPE needs the name of the root element."),
    BadDoctype => ("xml.bad-doctype", Error, "The DOCTYPE is malformed: {0}."),
    UnterminatedDoctype => ("xml.unterminated-doctype", Error, "The DOCTYPE is not closed: expected '>'."),
    // The DTD: declarations, entities and conditional sections (read by `xmlcraft-dtd`)
    DtdBadContentModel => ("dtd.bad-content-model", Error, "The content model is malformed: {0}."),
    DtdBadAttlist => ("dtd.bad-attribute-declaration", Error, "The attribute-list declaration is malformed: {0}."),
    DtdBadEntityDecl => ("dtd.bad-entity-declaration", Error, "The entity declaration is malformed: {0}."),
    DtdBadNotation => ("dtd.bad-notation-declaration", Error, "The notation declaration is malformed: {0}."),
    DtdBadConditional => ("dtd.bad-conditional-section", Error, "The conditional section is malformed: {0}."),
    DtdConditionalInInternal => ("dtd.conditional-in-internal-subset", Error, "A conditional section is only allowed in the external subset."),
    DtdUndeclaredPe => ("dtd.undeclared-parameter-entity", Error, "The parameter entity '%{0};' is not declared."),
    DtdUndeclaredPeUnread => ("dtd.undeclared-parameter-entity-unread", Warning, "The parameter entity '%{0};' is not declared (it may be declared in a part of the DTD that was not read)."),
    DtdBadPeReference => ("dtd.bad-parameter-entity-reference", Error, "The parameter-entity reference is malformed: {0}."),
    DtdPeBetweenDeclarations => ("dtd.parameter-entity-between-declarations", Error, "A parameter entity used between declarations must hold whole declarations, but one begins in it and ends outside it."),
    DtdRecursiveEntity => ("dtd.recursive-entity", Error, "The entity '{0}' contains a reference to itself."),
    DtdBadTextDecl => ("dtd.bad-text-declaration", Error, "The text declaration is malformed: {0}."),
    DtdExternalNotRead => ("dtd.external-not-read", Warning, "The external {0} was not read: {1}."),
    DtdLimit => ("dtd.limit", Error, "The DTD is too large or nested too deeply: {0}."),
    // The declarations of a DTD: validity constraints (XML 1.0 §3.2 to §4.7) and the non-normative determinism rule (appendix E)
    VcDuplicateElement => ("vc.duplicate-element-declaration", Error, "The element '{0}' is declared more than once."),
    VcDuplicateMixedName => ("vc.duplicate-mixed-content-name", Error, "The name '{0}' appears more than once in the mixed content model."),
    DtdNonDeterministic => ("dtd.non-deterministic-content-model", Warning, "The content model of '{0}' is not deterministic: an element '{1}' can be matched in more than one way."),
    VcOneIdPerElement => ("vc.one-id-attribute-per-element", Error, "The element '{0}' has more than one attribute of type ID."),
    VcIdAttributeDefault => ("vc.id-attribute-default", Error, "The ID attribute '{1}' of '{0}' must be #IMPLIED or #REQUIRED."),
    VcOneNotationPerElement => ("vc.one-notation-attribute-per-element", Error, "The element '{0}' has more than one attribute of type NOTATION."),
    VcNotationOnEmpty => ("vc.notation-attribute-on-empty-element", Error, "The NOTATION attribute '{1}' is declared for the EMPTY element '{0}'."),
    VcDuplicateToken => ("vc.duplicate-enumeration-token", Error, "'{1}' is listed more than once in the type of the attribute '{0}'."),
    VcNotationNotDeclared => ("vc.notation-not-declared", Error, "The notation '{0}' is not declared."),
    VcAttributeDefault => ("vc.attribute-default-value", Error, "The default value of the attribute '{0}' is not valid for its type: {1}."),
    VcDuplicateNotation => ("vc.duplicate-notation-declaration", Error, "The notation '{0}' is declared more than once."),
    // Namespaces
    InvalidQName => ("ns.invalid-qname", Error, "'{0}' is not a valid qualified name."),
    UnboundPrefix => ("ns.unbound-prefix", Error, "The namespace prefix '{0}' is not declared."),
    EmptyNamespaceName => ("ns.empty-namespace-name", Error, "The prefix '{0}' cannot be bound to an empty namespace name."),
    ColonInName => ("ns.colon-in-name", Error, "The name '{0}' cannot contain a colon in a namespace-aware document."),
    ReservedPrefix => ("ns.reserved-prefix", Error, "The prefix '{0}' is reserved and cannot be declared this way."),
    ReservedNamespace => ("ns.reserved-namespace", Error, "The namespace name '{0}' cannot be bound to the prefix '{1}'."),
    DuplicateExpandedAttribute => ("ns.duplicate-expanded-attribute", Error, "The attributes '{0}' and '{1}' have the same expanded name."),
    // Limits
    NestingTooDeep => ("limit.nesting-too-deep", Warning, "Elements nested deeper than {0} levels are not shown as a tree."),
    TooManyDiagnostics => ("limit.too-many-diagnostics", Info, "Further problems are not shown."),
    TooLarge => ("limit.too-large", Error, "The document is too large to analyse."),
}

/// One problem found in a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: Code,
    /// Where it is, as a byte range of the document (empty for a problem at a point).
    pub span: Range<usize>,
    /// Fills the placeholders of the message template.
    pub args: Vec<String>,
}

impl Diagnostic {
    pub fn new(code: Code, span: Range<usize>, args: Vec<String>) -> Self {
        Self { code, span, args }
    }

    pub fn severity(&self) -> Severity {
        self.code.severity()
    }

    /// The English message: the code's template with each `{N}` replaced by argument `N`. It is filled in one pass, so an argument that
    /// looks like a placeholder stays as it is, and a placeholder without an argument stays visible.
    pub fn message(&self) -> String {
        let mut out = String::new();
        let mut rest = self.code.template();
        while let Some(open) = rest.find('{') {
            let (head, tail) = rest.split_at(open);
            out.push_str(head);
            let filled = tail
                .strip_prefix('{')
                .and_then(|t| t.split_once('}'))
                .and_then(|(index, after)| Some((self.args.get(index.parse::<usize>().ok()?)?, after)));
            match filled {
                Some((arg, after)) => {
                    out.push_str(arg);
                    rest = after;
                }
                None => {
                    out.push('{');
                    rest = tail.strip_prefix('{').unwrap_or_default();
                }
            }
        }
        out.push_str(rest);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_templates_use_at_most_two_arguments_without_gaps() {
        let mut seen = std::collections::HashSet::new();
        for code in Code::ALL {
            assert!(seen.insert(code.key()), "duplicate key {}", code.key());
            assert!(code.key().contains('.') && code.key().is_ascii());
            let template = code.template();
            assert!(!template.contains("{2}"), "{} uses an argument no code supplies", code.key());
            // The arguments used start at `{0}` and have no gaps.
            let used: Vec<usize> = template.match_indices('{').filter_map(|(i, _)| template.get(i + 1..)?.split_once('}')?.0.parse().ok()).collect();
            let count = used.iter().max().map_or(0, |max| max + 1);
            assert!((0..count).all(|n| used.contains(&n)), "{} skips an argument", code.key());
        }
    }

    #[test]
    fn messages_fill_their_arguments() {
        let d = Diagnostic::new(Code::MismatchedEndTag, 3..7, vec!["a".into(), "b".into()]);
        assert_eq!(d.message(), "The end tag </b> does not match the start tag <a>.");
        assert_eq!(d.severity(), Severity::Error);
        assert_eq!(Diagnostic::new(Code::NoRootElement, 0..0, vec![]).message(), "The document has no root element.");
        assert_eq!(Code::NestingTooDeep.severity(), Severity::Warning);
        // A missing argument leaves the placeholder visible rather than failing.
        assert_eq!(Diagnostic::new(Code::UnclosedElement, 0..1, vec![]).message(), "The element <{0}> is not closed.");
    }

    #[test]
    fn arguments_that_look_like_placeholders_stay_literal() {
        let d = Diagnostic::new(Code::MismatchedEndTag, 0..1, vec!["{1}".into(), "b".into()]);
        assert_eq!(d.message(), "The end tag </b> does not match the start tag <{1}>.");
        let d = Diagnostic::new(Code::MismatchedEndTag, 0..1, vec!["a".into(), "{0}".into()]);
        assert_eq!(d.message(), "The end tag </{0}> does not match the start tag <a>.");
    }
}
