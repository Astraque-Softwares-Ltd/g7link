// SPDX-License-Identifier: MPL-2.0
//! Newline-delimited reading with a hard cap on the line length, so a peer
//! that never sends a newline can't make us buffer without bound.

use std::io::{self, BufRead};

use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// What one read produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineRead {
    /// A line is in the buffer, without its newline.
    Line,
    /// The stream ended with nothing buffered.
    Eof,
    /// The line passed the limit; the stream should be dropped.
    TooLong,
}

/// Reads one line into `buf` (cleared first). A line longer than `max` bytes
/// (newline excluded) gives [`LineRead::TooLong`]. Not cancel-safe: if the
/// future is dropped part way, the partial line is lost.
pub async fn read_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    max: usize,
) -> io::Result<LineRead> {
    buf.clear();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(if buf.is_empty() { LineRead::Eof } else { LineRead::Line });
        }
        match available.iter().position(|&b| b == b'\n') {
            Some(pos) => {
                if buf.len() + pos > max {
                    return Ok(LineRead::TooLong);
                }
                buf.extend_from_slice(&available[..pos]);
                reader.consume(pos + 1);
                return Ok(LineRead::Line);
            }
            None => {
                let len = available.len();
                if buf.len() + len > max {
                    return Ok(LineRead::TooLong);
                }
                buf.extend_from_slice(available);
                reader.consume(len);
            }
        }
    }
}

/// The blocking twin of [`read_line`]. `buf` is not cleared, so a read that
/// times out part way keeps what it got; clear it after using a line.
pub fn read_line_blocking<R: BufRead>(reader: &mut R, buf: &mut Vec<u8>, max: usize) -> io::Result<LineRead> {
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(if buf.is_empty() { LineRead::Eof } else { LineRead::Line });
        }
        match available.iter().position(|&b| b == b'\n') {
            Some(pos) => {
                if buf.len() + pos > max {
                    return Ok(LineRead::TooLong);
                }
                buf.extend_from_slice(&available[..pos]);
                reader.consume(pos + 1);
                return Ok(LineRead::Line);
            }
            None => {
                let len = available.len();
                if buf.len() + len > max {
                    return Ok(LineRead::TooLong);
                }
                buf.extend_from_slice(available);
                reader.consume(len);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn partial_lines_and_two_in_one_read() {
        let (mut tx, rx) = tokio::io::duplex(64);
        let mut reader = BufReader::new(rx);
        let mut buf = Vec::new();
        tx.write_all(b"{\"a\":1}\n{\"b\"").await.unwrap();
        assert_eq!(read_line(&mut reader, &mut buf, 100).await.unwrap(), LineRead::Line);
        assert_eq!(buf, b"{\"a\":1}");
        tx.write_all(b":2}\n").await.unwrap();
        assert_eq!(read_line(&mut reader, &mut buf, 100).await.unwrap(), LineRead::Line);
        assert_eq!(buf, b"{\"b\":2}");
        drop(tx);
        assert_eq!(read_line(&mut reader, &mut buf, 100).await.unwrap(), LineRead::Eof);
    }

    #[tokio::test]
    async fn a_line_at_the_limit_passes_and_one_over_does_not() {
        let data = format!("{}\n{}\n", "a".repeat(10), "b".repeat(11));
        let mut reader = BufReader::with_capacity(4, data.as_bytes());
        let mut buf = Vec::new();
        assert_eq!(read_line(&mut reader, &mut buf, 10).await.unwrap(), LineRead::Line);
        assert_eq!(buf.len(), 10);
        assert_eq!(read_line(&mut reader, &mut buf, 10).await.unwrap(), LineRead::TooLong);
    }

    #[test]
    fn blocking_reader_keeps_partial_data() {
        let mut reader = std::io::BufReader::with_capacity(3, &b"abcdef\nxy"[..]);
        let mut buf = Vec::new();
        assert_eq!(read_line_blocking(&mut reader, &mut buf, 100).unwrap(), LineRead::Line);
        assert_eq!(buf, b"abcdef");
        buf.clear();
        assert_eq!(read_line_blocking(&mut reader, &mut buf, 100).unwrap(), LineRead::Line);
        assert_eq!(buf, b"xy");
        buf.clear();
        assert_eq!(read_line_blocking(&mut reader, &mut buf, 100).unwrap(), LineRead::Eof);
    }
}
