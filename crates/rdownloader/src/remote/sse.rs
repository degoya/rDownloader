//! Reading `text/event-stream` for `rdownloader events` (RD-1240-18).
//!
//! Fed with whatever chunks the connection delivers, so a line or a `\r\n` split between two
//! chunks has to come out the same as one that arrived whole. It follows the specification
//! rather than what `rd-api` sends today, for the reasons `rd-capture`'s parser gives
//! (RD-109-09): a line ends at `\n`, `\r\n` or a lone `\r`, several `data:` lines join with
//! `\n`, exactly one space after the colon is dropped, and a comment line is skipped.

use std::time::Duration;

use super::client::{CommandError, Failure};

/// Longest line held while waiting for its end. A captcha announcement carries its image as a
/// `data:` URI, so this is generous; it only stops a broken peer from growing the buffer forever.
const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// One frame of the stream, up to the blank line that ends it.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Frame {
    pub(super) event: Option<String>,
    /// `None` for a frame without a `data:` line: it carries no event, only `id` or `retry`.
    pub(super) data: Option<String>,
    pub(super) id: Option<String>,
    pub(super) retry: Option<Duration>,
}

/// The incremental parser.
#[derive(Default)]
pub(super) struct Parser {
    line: Vec<u8>,
    frame: Frame,
    /// The previous chunk ended a line with `\r`; a `\n` that opens this one belongs to it.
    after_cr: bool,
}

impl Parser {
    /// Reads one chunk and returns the frames it completed.
    pub(super) fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Frame>, CommandError> {
        let mut frames = Vec::new();
        for &byte in bytes {
            if std::mem::take(&mut self.after_cr) && byte == b'\n' {
                continue;
            }
            if byte == b'\r' || byte == b'\n' {
                self.after_cr = byte == b'\r';
                let line = std::mem::take(&mut self.line);
                if let Some(frame) = self.end_line(&line) {
                    frames.push(frame);
                }
                continue;
            }
            if self.line.len() >= MAX_LINE_BYTES {
                return Err(CommandError::new(
                    Failure::Other,
                    format!("an event stream line exceeded {MAX_LINE_BYTES} bytes"),
                ));
            }
            self.line.push(byte);
        }
        Ok(frames)
    }

    fn end_line(&mut self, line: &[u8]) -> Option<Frame> {
        if line.is_empty() {
            return (self.frame != Frame::default()).then(|| std::mem::take(&mut self.frame));
        }
        let line = String::from_utf8_lossy(line);
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line.as_ref(), ""),
        };
        match field {
            "event" => self.frame.event = Some(value.to_owned()),
            "data" => match &mut self.frame.data {
                Some(data) => {
                    data.push('\n');
                    data.push_str(value);
                }
                None => self.frame.data = Some(value.to_owned()),
            },
            // A NUL in an id is the one case the specification tells a reader to ignore.
            "id" if !value.contains('\0') => self.frame.id = Some(value.to_owned()),
            "retry" => {
                if let Ok(milliseconds) = value.parse::<u64>() {
                    self.frame.retry = Some(Duration::from_millis(milliseconds));
                }
            }
            _ => {}
        }
        None
    }
}
