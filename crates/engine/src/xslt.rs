//! The XSLT stylesheet a document names: `<?xml-stylesheet type="text/xsl" href="style.xsl"?>` before its root element (what F10 uses when
//! no stylesheet is chosen).

use xmlcraft_query::{TransformError, Transformed, transform};
use xmlcraft_syntax::{SyntaxKind, SyntaxNode, parse};

use crate::worker::Worker;

/// The value of a pseudo-attribute in the text of a processing instruction.
fn pseudo(text: &str, name: &str) -> Option<String> {
    let mut rest = text;
    loop {
        rest = rest.trim_start();
        let name_end = rest.find(|c: char| c == '=' || c.is_whitespace())?;
        let (found, after) = rest.split_at_checked(name_end)?;
        let value_part = after.trim_start().strip_prefix('=')?.trim_start();
        let quote = value_part.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        let (value, remainder) = value_part.get(1..)?.split_once(quote)?;
        if found == name {
            return Some(decode(value));
        }
        rest = remainder;
    }
}

fn decode(value: &str) -> String {
    xmlcraft_syntax::decode_references(value)
}

/// The `href` of the first `xml-stylesheet` instruction before the root element whose `type` is an XSLT one (or has none), and
/// that is not an `alternate` or a CSS style sheet.
pub fn stylesheet_named(root: &SyntaxNode, text: &str) -> Option<String> {
    for node in root.children() {
        if node.kind() == SyntaxKind::Element {
            break;
        }
        if node.kind() != SyntaxKind::ProcessingInstruction {
            continue;
        }
        let range = usize::from(node.text_range().start())..usize::from(node.text_range().end());
        let Some(pi) = text.get(range) else { continue };
        let Some(body) = pi.strip_prefix("<?xml-stylesheet").filter(|rest| rest.starts_with(char::is_whitespace)) else { continue };
        let body = body.strip_suffix("?>").unwrap_or(body);
        if pseudo(body, "alternate").as_deref() == Some("yes") {
            continue;
        }
        let xslt_type = match pseudo(body, "type").as_deref() {
            None => true,
            Some(t) => matches!(t.to_ascii_lowercase().as_str(), "text/xsl" | "text/xml" | "application/xml" | "application/xslt+xml"),
        };
        if xslt_type && let Some(href) = pseudo(body, "href").filter(|h| !h.trim().is_empty()) {
            return Some(href);
        }
    }
    None
}

/// [`stylesheet_named`] for a text.
pub fn stylesheet_of(text: &str) -> Option<String> {
    let parsed = parse(text);
    stylesheet_named(&parsed.syntax(), text)
}

/// One request: a stylesheet applied to a document.
#[derive(Clone, Debug)]
pub struct XsltJob {
    /// Told back in the outcome, so that the caller can tell a result it still waits for from one it gave up on.
    pub serial: u64,
    pub xml: String,
    pub xslt: String,
}

/// What came of a request.
#[derive(Clone, Debug)]
pub struct XsltOutcome {
    pub serial: u64,
    pub result: Result<Transformed, TransformError>,
}

/// Applies stylesheets on a worker thread: what the Transform window waits for.
pub struct XsltRunner {
    worker: Worker<XsltJob, XsltOutcome>,
}

fn run(job: XsltJob) -> XsltOutcome {
    let serial = job.serial;
    let result = match crate::guard(|| transform(&job.xml, &job.xslt)) {
        Ok(done) => done,
        Err(panic) => {
            log::error!("a transformation failed: {panic}");
            Err(TransformError { message: "the transformation failed (a bug); nothing was changed".to_owned() })
        }
    };
    XsltOutcome { serial, result }
}

impl XsltRunner {
    pub fn new() -> Self {
        Self { worker: Worker::new("xslt transformation", run) }
    }

    /// A runner with no thread: `submit` transforms at once.
    pub fn inline() -> Self {
        Self { worker: Worker::inline(run) }
    }

    pub fn on_done(&self, notify: impl Fn() + Send + Sync + 'static) {
        self.worker.on_done(notify);
    }

    pub fn submit(&self, job: XsltJob) {
        self.worker.submit(0, job);
    }

    pub fn poll(&self) -> Vec<XsltOutcome> {
        self.worker.poll().into_iter().map(|(_, outcome)| outcome).collect()
    }
}

impl Default for XsltRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stylesheet_is_applied_on_a_worker_and_the_outcome_says_which_request_it_is_for() {
        let runner = XsltRunner::inline();
        let xsl = "<xsl:stylesheet version=\"3.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\"><xsl:template match=\"/\"><n><xsl:value-of select=\"count(//i)\"/></n></xsl:template></xsl:stylesheet>";
        runner.submit(XsltJob { serial: 4, xml: "<r><i/><i/></r>".to_owned(), xslt: xsl.to_owned() });
        runner.submit(XsltJob { serial: 5, xml: "<r>".to_owned(), xslt: xsl.to_owned() });
        let outcomes = runner.poll();
        assert_eq!(outcomes.iter().map(|o| o.serial).collect::<Vec<_>>(), [4, 5]);
        assert_eq!(outcomes[0].result.as_ref().expect("transforms").text, "<n>2</n>");
        assert!(outcomes[1].result.is_err());
    }

    #[test]
    fn the_first_xslt_stylesheet_before_the_root_is_the_one_named() {
        let named = |pi: &str| stylesheet_of(&format!("<?xml version=\"1.0\"?>\n{pi}\n<r/>"));
        assert_eq!(named("<?xml-stylesheet type=\"text/xsl\" href=\"a.xsl\"?>").as_deref(), Some("a.xsl"));
        assert_eq!(named("<?xml-stylesheet href='b.xslt'?>").as_deref(), Some("b.xslt"), "no type: taken for XSLT");
        assert_eq!(named("<?xml-stylesheet type=\"text/css\" href=\"c.css\"?>"), None);
        assert_eq!(named("<?xml-stylesheet type=\"text/xsl\" href=\"d.xsl\" alternate=\"yes\"?>"), None);
        assert_eq!(
            named("<?xml-stylesheet type=\"text/css\" href=\"c.css\"?><?xml-stylesheet type=\"application/xslt+xml\" href=\"e&amp;f.xsl\"?>")
                .as_deref(),
            Some("e&f.xsl")
        );
        assert_eq!(named("<?xml-stylesheet type=\"text/xsl\"?>"), None, "no href");
        assert_eq!(stylesheet_of("<r/><?xml-stylesheet href=\"x.xsl\"?>"), None, "after the root it is not a prolog instruction");
        assert_eq!(stylesheet_of(""), None);
    }
}
