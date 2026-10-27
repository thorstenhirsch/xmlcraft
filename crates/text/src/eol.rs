//! Line endings: what a file uses, and how to write new ones to match.

/// One kind of line ending.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Eol {
    /// `\n`: Unix, macOS.
    #[default]
    Lf,
    /// `\r\n`: Windows.
    CrLf,
    /// `\r`: classic Mac OS. XML parsers read it as a line end; this editor shows such a file as one line.
    Cr,
}

impl Eol {
    pub fn as_str(self) -> &'static str {
        match self {
            Eol::Lf => "\n",
            Eol::CrLf => "\r\n",
            Eol::Cr => "\r",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Eol::Lf => "LF",
            Eol::CrLf => "CRLF",
            Eol::Cr => "CR",
        }
    }
}

/// How many of each line ending a text has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineEndings {
    pub lf: usize,
    pub crlf: usize,
    pub cr: usize,
}

impl LineEndings {
    /// Count the line endings in `text`.
    pub fn count(text: &str) -> Self {
        Self::count_chunks([text])
    }

    /// Count across pieces of a text (a piece may end in `\r` and the next start with `\n`).
    pub fn count_chunks<'a>(chunks: impl IntoIterator<Item = &'a str>) -> Self {
        let mut counts = Self::default();
        let mut pending_cr = false;
        for chunk in chunks {
            for &b in chunk.as_bytes() {
                match b {
                    b'\n' => {
                        if pending_cr {
                            counts.crlf += 1;
                        } else {
                            counts.lf += 1;
                        }
                        pending_cr = false;
                    }
                    b'\r' => {
                        if pending_cr {
                            counts.cr += 1;
                        }
                        pending_cr = true;
                    }
                    _ => {
                        if pending_cr {
                            counts.cr += 1;
                        }
                        pending_cr = false;
                    }
                }
            }
        }
        if pending_cr {
            counts.cr += 1;
        }
        counts
    }

    /// The most common ending (Lf when there are none or on a tie, the platform-neutral choice).
    pub fn dominant(&self) -> Eol {
        if self.crlf > self.lf && self.crlf > self.cr {
            Eol::CrLf
        } else if self.cr > self.lf && self.cr > self.crlf {
            Eol::Cr
        } else {
            Eol::Lf
        }
    }

    /// Whether more than one kind is present.
    pub fn is_mixed(&self) -> bool {
        usize::from(self.lf > 0) + usize::from(self.crlf > 0) + usize::from(self.cr > 0) > 1
    }

    pub fn total(&self) -> usize {
        self.lf + self.crlf + self.cr
    }
}

/// `text` with every line ending replaced by `eol`.
pub fn convert(text: &str, eol: Eol) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 32);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str(eol.as_str());
            }
            '\n' => out.push_str(eol.as_str()),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_is_counted() {
        assert_eq!(LineEndings::count("a\nb\r\nc\rd"), LineEndings { lf: 1, crlf: 1, cr: 1 });
        assert_eq!(LineEndings::count(""), LineEndings::default());
        assert_eq!(LineEndings::count("no breaks"), LineEndings::default());
        assert_eq!(LineEndings::count("\r\r\n\n"), LineEndings { lf: 1, crlf: 1, cr: 1 });
        assert_eq!(LineEndings::count("trailing\r"), LineEndings { lf: 0, crlf: 0, cr: 1 });
    }

    #[test]
    fn a_crlf_split_across_chunks_is_one_ending() {
        assert_eq!(LineEndings::count_chunks(["a\r", "\nb"]), LineEndings { lf: 0, crlf: 1, cr: 0 });
        assert_eq!(LineEndings::count_chunks(["a\r", "b"]), LineEndings { lf: 0, crlf: 0, cr: 1 });
        assert_eq!(LineEndings::count_chunks(["", "\r", "", "\n"]), LineEndings { lf: 0, crlf: 1, cr: 0 });
    }

    #[test]
    fn the_dominant_ending_and_mixing() {
        assert_eq!(LineEndings::count("a\r\nb\r\nc\n").dominant(), Eol::CrLf);
        assert_eq!(LineEndings::count("a\nb\nc\r\n").dominant(), Eol::Lf);
        assert_eq!(LineEndings::count("a\rb\rc").dominant(), Eol::Cr);
        assert_eq!(LineEndings::count("a\r\nb\r\nc\rd\re\nf").dominant(), Eol::Lf, "CRLF and CR tie, and both beat LF only by count: a tie is LF");
        assert_eq!(LineEndings { lf: 1, crlf: 2, cr: 2 }.dominant(), Eol::Lf);
        assert_eq!(LineEndings::count("x").dominant(), Eol::Lf);
        assert_eq!(LineEndings::count("a\nb\r\n").dominant(), Eol::Lf, "a tie is LF");
        assert!(LineEndings::count("a\nb\r\n").is_mixed());
        assert!(!LineEndings::count("a\nb\n").is_mixed());
        assert_eq!(LineEndings::count("a\nb\r\n").total(), 2);
    }

    #[test]
    fn conversion_normalises_every_ending() {
        assert_eq!(convert("a\nb\r\nc\rd", Eol::CrLf), "a\r\nb\r\nc\r\nd");
        assert_eq!(convert("a\r\nb\rc", Eol::Lf), "a\nb\nc");
        assert_eq!(convert("a\nb", Eol::Cr), "a\rb");
        assert_eq!(convert("é\n日", Eol::Lf), "é\n日");
        assert_eq!(convert("", Eol::CrLf), "");
        assert_eq!(Eol::CrLf.label(), "CRLF");
    }
}
