//! Validating a document while it is typed in: when a document that names a grammar has been left alone for a moment, it is validated in
//! the background, as F8 would, and what is found is underlined and listed without a word in the status bar. The decision is here, free
//! of the app, so that every way it can go is tested.

use std::time::Duration;

/// How long the text has to be left alone before it is validated: a keystroke does not start a validation, a pause does.
pub(crate) const DELAY: Duration = Duration::from_millis(700);

/// Documents larger than this are validated on request only (F8): a pause in typing should not start the whole validation of a big text.
pub(crate) const MAX_BYTES: usize = 1024 * 1024;

/// What the decision is made from.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Inputs {
    /// "Validate While Typing" is on.
    pub enabled: bool,
    /// The document has a DOCTYPE or names a schema, and the analysis of this very text found it well-formed (a text that is not has its
    /// own problems listed already, and nothing to be validated against what it says).
    pub ready: bool,
    /// The size of the text in bytes.
    pub size: usize,
    /// This version of the text was validated or asked for already.
    pub done: bool,
    /// How long this version of the text has stood as it is.
    pub quiet_for: Duration,
    pub delay: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    /// Nothing to do.
    Not,
    /// Not yet: look again in this long.
    Wait(Duration),
    /// Validate now.
    Now,
}

/// Whether `analysis` lets the text of version `version` be validated while it is typed in: it is the analysis of this very text (one of
/// older text says nothing of this one), the text is well-formed, and it names a grammar (a DOCTYPE or a schema).
pub(crate) fn ready(analysis: &xmlcraft_engine::analysis::Analysis, version: u64) -> bool {
    analysis.version == version && analysis.is_well_formed() && (analysis.doctype.is_some() || !analysis.schema_hints.is_empty())
}

pub(crate) fn decide(i: &Inputs) -> Decision {
    if !i.enabled || !i.ready || i.done || i.size > MAX_BYTES {
        return Decision::Not;
    }
    match i.delay.checked_sub(i.quiet_for) {
        Some(left) if !left.is_zero() => Decision::Wait(left),
        _ => Decision::Now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn due() -> Inputs {
        Inputs { enabled: true, ready: true, size: 1000, done: false, quiet_for: DELAY, delay: DELAY }
    }

    fn analysed(text: &str, version: u64) -> xmlcraft_engine::analysis::Analysis {
        xmlcraft_engine::analysis::analyze(text, version)
    }

    #[test]
    fn a_text_is_ready_when_its_own_analysis_says_it_is_well_formed_and_names_a_grammar() {
        let dtd = "<!DOCTYPE a [<!ELEMENT a EMPTY>]><a/>";
        let schema = "<a xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"a.xsd\"/>";
        assert!(ready(&analysed(dtd, 4), 4));
        assert!(ready(&analysed(schema, 4), 4), "a schema named is a grammar named");
        assert!(!ready(&analysed(dtd, 3), 4), "the analysis of older text says nothing of this text");
        assert!(!ready(&analysed(dtd, 5), 4), "nor does one of newer text");
        assert!(!ready(&analysed("<a><b/></a>", 4), 4), "it names no grammar");
        assert!(!ready(&analysed("<!DOCTYPE a [<!ELEMENT a EMPTY>]><a>", 4), 4), "it is not well-formed");
    }

    #[test]
    fn a_text_that_has_been_left_alone_long_enough_is_validated() {
        assert_eq!(decide(&due()), Decision::Now);
        assert_eq!(decide(&Inputs { quiet_for: DELAY + Duration::from_secs(5), ..due() }), Decision::Now);
        assert_eq!(decide(&Inputs { size: 0, ..due() }), Decision::Now);
        assert_eq!(decide(&Inputs { size: MAX_BYTES, ..due() }), Decision::Now, "the limit itself is allowed");
    }

    #[test]
    fn a_text_that_was_just_changed_waits_for_what_is_left_of_the_delay() {
        assert_eq!(decide(&Inputs { quiet_for: Duration::ZERO, ..due() }), Decision::Wait(DELAY));
        assert_eq!(decide(&Inputs { quiet_for: Duration::from_millis(200), ..due() }), Decision::Wait(Duration::from_millis(500)));
        assert_eq!(decide(&Inputs { quiet_for: DELAY - Duration::from_millis(1), ..due() }), Decision::Wait(Duration::from_millis(1)));
        // No delay at all: at once.
        assert_eq!(decide(&Inputs { delay: Duration::ZERO, quiet_for: Duration::ZERO, ..due() }), Decision::Now);
    }

    #[test]
    fn what_is_not_to_be_validated_while_typing_is_not() {
        assert_eq!(decide(&Inputs { enabled: false, ..due() }), Decision::Not, "the person turned it off");
        assert_eq!(decide(&Inputs { ready: false, ..due() }), Decision::Not, "no grammar, or not well-formed, or the analysis is of older text");
        assert_eq!(decide(&Inputs { done: true, ..due() }), Decision::Not, "this version was done");
        assert_eq!(decide(&Inputs { size: MAX_BYTES + 1, ..due() }), Decision::Not, "too big: on request only");
        // Neither does a wait begin for what will not be done.
        assert_eq!(decide(&Inputs { enabled: false, quiet_for: Duration::ZERO, ..due() }), Decision::Not);
        assert_eq!(decide(&Inputs { done: true, quiet_for: Duration::ZERO, ..due() }), Decision::Not);
    }
}
