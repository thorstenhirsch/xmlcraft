//! The whiteSpace facet: how a lexical representation is normalised before anything else is done with it (XML Schema Part 2, 4.3.6).

use std::borrow::Cow;

/// What to do with white space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Whitespace {
    /// Nothing.
    Preserve,
    /// Each tab, line feed and carriage return becomes a space.
    Replace,
    /// As `Replace`, then runs of spaces become one and the ends are trimmed.
    Collapse,
}

impl Whitespace {
    /// The value of the `whiteSpace` facet attribute (`preserve`, `replace`, `collapse`).
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "preserve" => Some(Self::Preserve),
            "replace" => Some(Self::Replace),
            "collapse" => Some(Self::Collapse),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Preserve => "preserve",
            Self::Replace => "replace",
            Self::Collapse => "collapse",
        }
    }

    /// `text` normalised. (Borrowed when nothing changes.)
    pub fn apply(self, text: &str) -> Cow<'_, str> {
        match self {
            Self::Preserve => Cow::Borrowed(text),
            Self::Replace => {
                if text.contains(['\t', '\n', '\r']) {
                    Cow::Owned(text.chars().map(|c| if matches!(c, '\t' | '\n' | '\r') { ' ' } else { c }).collect())
                } else {
                    Cow::Borrowed(text)
                }
            }
            Self::Collapse => {
                let collapsed = collapse(text);
                if collapsed == text { Cow::Borrowed(text) } else { Cow::Owned(collapsed) }
            }
        }
    }

    /// How strict the setting is: a type may only make it stricter (`preserve` < `replace` < `collapse`).
    pub fn strictness(self) -> u8 {
        match self {
            Self::Preserve => 0,
            Self::Replace => 1,
            Self::Collapse => 2,
        }
    }
}

/// Tabs, line feeds and carriage returns as spaces, the runs of spaces as one, the ends trimmed.
pub fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split([' ', '\t', '\n', '\r']).filter(|w| !w.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}
