//! Reading newline-delimited JSON with a hard limit on a line's length, so a peer that never sends a newline cannot make us buffer
//! without end. Shared by the MCP server and the control channel.

use std::io::{BufRead, Read};

/// What [`read_line_limited`] found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LineRead {
    /// A line, now in the buffer (newline and a trailing `\r` stripped).
    Line,
    /// End of stream.
    End,
    /// The line is longer than the limit; at most limit + 1 bytes of it were read.
    TooLong,
}

/// Read one `\n`-terminated line of at most `max` bytes into `buf`.
pub(crate) fn read_line_limited(reader: &mut impl BufRead, buf: &mut Vec<u8>, max: usize) -> std::io::Result<LineRead> {
    buf.clear();
    // `take` bounds what is buffered for one line: never more than `max` + 1 bytes.
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    if reader.take(limit).read_until(b'\n', buf)? == 0 {
        return Ok(LineRead::End);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        return Ok(LineRead::Line);
    }
    // No newline: either the limit was passed, or this is a last line before end of stream.
    Ok(if buf.len() > max { LineRead::TooLong } else { LineRead::Line })
}

/// Discard the rest of the current line, without buffering it.
#[cfg(feature = "mcp")]
pub(crate) fn drain_line(reader: &mut impl BufRead) -> std::io::Result<()> {
    loop {
        let (consumed, done) = {
            let chunk = reader.fill_buf()?;
            if chunk.is_empty() {
                return Ok(());
            }
            match chunk.iter().position(|b| *b == b'\n') {
                Some(i) => (i + 1, true),
                None => (chunk.len(), false),
            }
        };
        reader.consume(consumed);
        if done {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn lines_are_read_up_to_the_limit() {
        let mut r = Cursor::new(b"abc\r\ndef\nlast".to_vec());
        let mut buf = Vec::new();
        assert_eq!(read_line_limited(&mut r, &mut buf, 16).expect("read"), LineRead::Line);
        assert_eq!(buf, b"abc");
        assert_eq!(read_line_limited(&mut r, &mut buf, 16).expect("read"), LineRead::Line);
        assert_eq!(buf, b"def");
        assert_eq!(read_line_limited(&mut r, &mut buf, 16).expect("read"), LineRead::Line, "a last line without a newline");
        assert_eq!(buf, b"last");
        assert_eq!(read_line_limited(&mut r, &mut buf, 16).expect("read"), LineRead::End);
    }

    #[test]
    fn a_line_over_the_limit_is_reported_without_reading_it_all() {
        let mut r = Cursor::new(vec![b'x'; 1_000_000]);
        let mut buf = Vec::new();
        assert_eq!(read_line_limited(&mut r, &mut buf, 100).expect("read"), LineRead::TooLong);
        assert_eq!(buf.len(), 101, "only limit + 1 bytes were buffered");
        // A line exactly at the limit is fine.
        let mut r = Cursor::new([vec![b'y'; 100], b"\n".to_vec()].concat());
        assert_eq!(read_line_limited(&mut r, &mut buf, 100).expect("read"), LineRead::Line);
        assert_eq!(buf.len(), 100);
    }

    #[cfg(feature = "mcp")]
    #[test]
    fn draining_skips_to_the_end_of_the_line() {
        let mut r = Cursor::new(b"long long line\nnext\n".to_vec());
        drain_line(&mut r).expect("drain");
        let mut buf = Vec::new();
        assert_eq!(read_line_limited(&mut r, &mut buf, 16).expect("read"), LineRead::Line);
        assert_eq!(buf, b"next");
        drain_line(&mut r).expect("drain at the end");
    }
}
