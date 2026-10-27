//! Our well-formedness verdict against `xmllint`'s, used strictly as a black box (its source is never read).
//!
//! Skipped with a notice when `xmllint` is not installed, unless `XMLCRAFT_ORACLES_REQUIRED=1` (CI sets it on Linux).

use std::io::Write;
use std::process::{Command, Stdio};

use proptest::prelude::*;
use xmlcraft_syntax::parse;

/// Whether `xmllint` accepts `src` as well-formed; `None` if it is not installed.
fn xmllint_accepts(src: &str) -> Option<bool> {
    let spawned = Command::new("xmllint").args(["--noout", "--nonet", "-"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(_) => {
            assert!(std::env::var_os("XMLCRAFT_ORACLES_REQUIRED").is_none(), "XMLCRAFT_ORACLES_REQUIRED is set but xmllint is not installed");
            eprintln!("note: xmllint is not installed; skipping the differential check");
            return None;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        // xmllint may stop reading early on a fatal error; a broken pipe is fine.
        let _ = stdin.write_all(src.as_bytes());
    }
    child.wait().ok().map(|status| status.success())
}

fn ours(src: &str) -> bool {
    !parse(src).has_errors()
}

/// Documents whose verdict follows from XML 1.0: (document, well-formed). Namespace constraint violations are left out: libxml2
/// reports them as non-fatal, where this parser reports them as errors under Namespaces in XML 1.0 (see `tests/parse.rs`).
const KNOWN: &[(&str, bool)] = &[
    ("<a/>", true),
    ("<a></a>", true),
    ("<a>text</a>", true),
    ("  \n<a/>\n  ", true),
    ("\u{FEFF}<a/>", true),
    ("\u{FEFF}\n<a/>", true),
    ("<?xml version=\"1.0\" encoding=\"UTF-8\"?><a/>", true),
    ("<a b=\"1\" c='2'>t<!--c--><![CDATA[x<y]]><?pi data?>&amp;&#65;&#x41;</a>", true),
    ("<a xmlns=\"urn:x\" xmlns:p=\"urn:p\" p:b=\"1\"><p:c/></a>", true),
    ("<!DOCTYPE a [<!ENTITY e \"x\">]><a>&e;</a>", true),
    ("<日本 é=\"1\">😀</日本>", true),
    ("<a b = \"x\"\n   c\n = 'y' />", true),
    ("", false),
    ("<a>", false),
    ("</a>", false),
    ("<a></b>", false),
    ("<a><b></a></b>", false),
    ("<a/><b/>", false),
    ("text<a/>", false),
    ("<a/>text", false),
    ("<a b=\"1\" b=\"2\"/>", false),
    ("<a b/>", false),
    ("<a b=c/>", false),
    ("<a b=\"<\"/>", false),
    ("<a b=\"x/>", false),
    ("<a>&nbsp;</a>", false),
    ("<a>&</a>", false),
    ("<a>a < b</a>", false),
    ("<a>]]></a>", false),
    ("<a>&#0;</a>", false),
    ("<a>&#xD800;</a>", false),
    ("<a>\u{1}</a>", false),
    ("<!-- a -- b --><a/>", false),
    ("<!-- a ---><a/>", false),
    ("<!-- open", false),
    ("<a><![CDATA[x</a>", false),
    ("<a/><?xml version=\"1.0\"?>", false),
    (" <?xml version=\"1.0\"?><a/>", false),
    ("<?xml encoding=\"UTF-8\" version=\"1.0\"?><a/>", false),
    ("<?XML x?><a/>", false),
    ("<a/><!DOCTYPE a>", false),
    ("<a 1b=\"x\"/>", false),
    ("<a><b></c></a>", false),
    ("<a></a b>", false),
    ("<a></>", false),
    ("<a:b xmlns:a=\"urn:x\"/>", true),
];

#[test]
fn known_documents_have_the_verdict_the_specifications_give() {
    for (src, well_formed) in KNOWN {
        assert_eq!(ours(src), *well_formed, "our verdict on {src:?}");
        if let Some(theirs) = xmllint_accepts(src) {
            assert_eq!(theirs, *well_formed, "xmllint's verdict on {src:?} (is the table wrong?)");
        }
    }
}

/// Pieces without namespaces or DTD declarations (their bodies are the DTD parser's, not this one's).
const PIECES: &[&str] = &[
    "<",
    "</",
    ">",
    "/>",
    "/",
    "\"",
    "'",
    "=",
    "&amp;",
    "&#65;",
    "&",
    ";",
    "<!--",
    "-->",
    "--",
    "<![CDATA[",
    "]]>",
    "<?xml",
    "<?",
    "?>",
    " ",
    "\n",
    "a",
    "b",
    "é",
    "日",
    "\u{1}",
    "version=\"1.0\"",
    "<a>",
    "</a>",
    "<a/>",
    "<b>",
    "</b>",
    "x=\"1\"",
    "-",
    "1",
];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn verdicts_agree_on_random_markup(pieces in proptest::collection::vec(proptest::sample::select(PIECES), 0..14)) {
        let src = pieces.concat();
        if let Some(theirs) = xmllint_accepts(&src) {
            prop_assert_eq!(ours(&src), theirs, "{:?}", src);
        }
    }
}
