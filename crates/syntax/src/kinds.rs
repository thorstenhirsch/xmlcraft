//! The kinds of the syntax tree: the lexer's tokens, and the nodes the parser builds from them.

use crate::lexer::TokenKind;

/// What a node or token of the syntax tree is. The first group are tokens (one for each [`TokenKind`], in the same order), the second
/// are nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u16)]
pub enum SyntaxKind {
    // --- tokens ---
    Text,
    CharRef,
    EntityRef,
    BareAmp,
    BareLt,
    TagOpen,
    EndTagOpen,
    Gt,
    SlashGt,
    Name,
    Eq,
    Whitespace,
    Quote,
    AttrText,
    LiteralText,
    CommentStart,
    CommentText,
    CommentEnd,
    CdataStart,
    CdataText,
    CdataEnd,
    PiStart,
    PiTarget,
    PiText,
    PiEnd,
    DoctypeStart,
    DeclStart,
    LBracket,
    RBracket,
    PercentRef,
    Punct,
    Error,
    // --- nodes ---
    /// The root: everything in the text.
    Document,
    /// `<?xml ... ?>` at the very start.
    XmlDecl,
    /// `<!DOCTYPE ... >`.
    Doctype,
    /// `[` ... `]` in a DOCTYPE.
    DoctypeSubset,
    /// `<!ELEMENT ...>`, `<!ATTLIST ...>`, `<!ENTITY ...>` or `<!NOTATION ...>` in the subset.
    MarkupDecl,
    /// A start tag, the content, and the end tag (missing if the element is not closed).
    Element,
    StartTag,
    EndTag,
    /// `name="value"` in a start tag.
    Attribute,
    /// The value of an attribute: the quoted text (quotes included), or the bare name token of an unquoted value, which is reported.
    AttrValue,
    Comment,
    CData,
    ProcessingInstruction,
}

/// Every kind, in discriminant order: the way back from a raw number.
const KINDS: &[SyntaxKind] = &[
    SyntaxKind::Text,
    SyntaxKind::CharRef,
    SyntaxKind::EntityRef,
    SyntaxKind::BareAmp,
    SyntaxKind::BareLt,
    SyntaxKind::TagOpen,
    SyntaxKind::EndTagOpen,
    SyntaxKind::Gt,
    SyntaxKind::SlashGt,
    SyntaxKind::Name,
    SyntaxKind::Eq,
    SyntaxKind::Whitespace,
    SyntaxKind::Quote,
    SyntaxKind::AttrText,
    SyntaxKind::LiteralText,
    SyntaxKind::CommentStart,
    SyntaxKind::CommentText,
    SyntaxKind::CommentEnd,
    SyntaxKind::CdataStart,
    SyntaxKind::CdataText,
    SyntaxKind::CdataEnd,
    SyntaxKind::PiStart,
    SyntaxKind::PiTarget,
    SyntaxKind::PiText,
    SyntaxKind::PiEnd,
    SyntaxKind::DoctypeStart,
    SyntaxKind::DeclStart,
    SyntaxKind::LBracket,
    SyntaxKind::RBracket,
    SyntaxKind::PercentRef,
    SyntaxKind::Punct,
    SyntaxKind::Error,
    SyntaxKind::Document,
    SyntaxKind::XmlDecl,
    SyntaxKind::Doctype,
    SyntaxKind::DoctypeSubset,
    SyntaxKind::MarkupDecl,
    SyntaxKind::Element,
    SyntaxKind::StartTag,
    SyntaxKind::EndTag,
    SyntaxKind::Attribute,
    SyntaxKind::AttrValue,
    SyntaxKind::Comment,
    SyntaxKind::CData,
    SyntaxKind::ProcessingInstruction,
];

impl SyntaxKind {
    /// Whether this is a leaf (a token) rather than a node.
    pub fn is_token(self) -> bool {
        (self as u16) < (SyntaxKind::Document as u16)
    }

    /// The kind with this number; an unknown number (which the tree never contains) reads as [`SyntaxKind::Error`].
    pub fn from_raw(raw: u16) -> Self {
        KINDS.get(usize::from(raw)).copied().unwrap_or(SyntaxKind::Error)
    }
}

impl From<TokenKind> for SyntaxKind {
    fn from(kind: TokenKind) -> Self {
        match kind {
            TokenKind::Text => SyntaxKind::Text,
            TokenKind::CharRef => SyntaxKind::CharRef,
            TokenKind::EntityRef => SyntaxKind::EntityRef,
            TokenKind::BareAmp => SyntaxKind::BareAmp,
            TokenKind::BareLt => SyntaxKind::BareLt,
            TokenKind::TagOpen => SyntaxKind::TagOpen,
            TokenKind::EndTagOpen => SyntaxKind::EndTagOpen,
            TokenKind::Gt => SyntaxKind::Gt,
            TokenKind::SlashGt => SyntaxKind::SlashGt,
            TokenKind::Name => SyntaxKind::Name,
            TokenKind::Eq => SyntaxKind::Eq,
            TokenKind::Whitespace => SyntaxKind::Whitespace,
            TokenKind::Quote => SyntaxKind::Quote,
            TokenKind::AttrText => SyntaxKind::AttrText,
            TokenKind::LiteralText => SyntaxKind::LiteralText,
            TokenKind::CommentStart => SyntaxKind::CommentStart,
            TokenKind::CommentText => SyntaxKind::CommentText,
            TokenKind::CommentEnd => SyntaxKind::CommentEnd,
            TokenKind::CdataStart => SyntaxKind::CdataStart,
            TokenKind::CdataText => SyntaxKind::CdataText,
            TokenKind::CdataEnd => SyntaxKind::CdataEnd,
            TokenKind::PiStart => SyntaxKind::PiStart,
            TokenKind::PiTarget => SyntaxKind::PiTarget,
            TokenKind::PiText => SyntaxKind::PiText,
            TokenKind::PiEnd => SyntaxKind::PiEnd,
            TokenKind::DoctypeStart => SyntaxKind::DoctypeStart,
            TokenKind::DeclStart => SyntaxKind::DeclStart,
            TokenKind::LBracket => SyntaxKind::LBracket,
            TokenKind::RBracket => SyntaxKind::RBracket,
            TokenKind::PercentRef => SyntaxKind::PercentRef,
            TokenKind::Punct => SyntaxKind::Punct,
            TokenKind::Error => SyntaxKind::Error,
        }
    }
}

/// The language marker that ties [`SyntaxKind`] to `rowan`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum XmlLanguage {}

impl rowan::Language for XmlLanguage {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> SyntaxKind {
        SyntaxKind::from_raw(raw.0)
    }

    fn kind_to_raw(kind: SyntaxKind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}

pub type SyntaxNode = rowan::SyntaxNode<XmlLanguage>;
pub type SyntaxToken = rowan::SyntaxToken<XmlLanguage>;
pub type SyntaxElement = rowan::SyntaxElement<XmlLanguage>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_matches_the_discriminants() {
        for (i, kind) in KINDS.iter().enumerate() {
            assert_eq!(*kind as usize, i, "{kind:?}");
        }
        // `ProcessingInstruction` is the last kind, so a kind added without a table entry shows up as a length mismatch.
        assert_eq!(KINDS.len(), SyntaxKind::ProcessingInstruction as usize + 1);
        assert_eq!(SyntaxKind::from_raw(u16::MAX), SyntaxKind::Error, "an unknown number is not a panic");
        assert_eq!(SyntaxKind::from_raw(SyntaxKind::Element as u16), SyntaxKind::Element);
    }

    #[test]
    fn tokens_come_before_nodes() {
        assert!(SyntaxKind::Error.is_token() && SyntaxKind::Text.is_token());
        assert!(!SyntaxKind::Document.is_token() && !SyntaxKind::ProcessingInstruction.is_token());
        assert_eq!(SyntaxKind::from(TokenKind::Punct), SyntaxKind::Punct);
        assert_eq!(SyntaxKind::from(TokenKind::Error), SyntaxKind::Error);
    }
}
