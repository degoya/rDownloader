//! Progress reporting shared by the extraction backends.

use tokio::sync::mpsc;

/// One progress sample of a running extraction.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExtractProgress {
    pub done_bytes: u64,
    pub total_bytes: Option<u64>,
    /// 0–100 when derivable (byte totals or tool output).
    pub percent: Option<u8>,
    /// Entry currently being written.
    pub current: Option<String>,
}

/// Unbounded channel: backends never block on a slow consumer.
pub type ProgressSender = mpsc::UnboundedSender<ExtractProgress>;

/// Sends a sample when a sender is attached; a closed receiver is ignored.
pub(crate) fn report(sender: Option<&ProgressSender>, sample: ExtractProgress) {
    if let Some(sender) = sender {
        let _ = sender.send(sample);
    }
}

/// Byte-based percent, saturating at 100.
#[must_use]
pub fn percent_of(done: u64, total: Option<u64>) -> Option<u8> {
    let total = total.filter(|value| *value > 0)?;
    Some(u8::try_from((done.min(total) * 100) / total).unwrap_or(100))
}

/// Extracts the last percentage printed by unrar/7z in a chunk of console output.
/// Both tools rewrite the same line with `\r`/backspaces, so the last token wins.
#[must_use]
pub fn parse_tool_percent(chunk: &str) -> Option<u8> {
    let mut last = None;
    let bytes = chunk.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let mut start = index;
            while start > 0 && bytes[start - 1].is_ascii_digit() && index - start < 3 {
                start -= 1;
            }
            if start < index
                && let Ok(value) = chunk[start..index].parse::<u16>()
            {
                last = Some(u8::try_from(value.min(100)).unwrap_or(100));
            }
        }
        index += 1;
    }
    last
}

#[cfg(test)]
mod tests {
    use super::{parse_tool_percent, percent_of};

    #[test]
    fn parses_rewritten_progress_lines() {
        assert_eq!(parse_tool_percent("  5%\r 45%\r100%\n"), Some(100));
        assert_eq!(
            parse_tool_percent("\x08\x08\x08 12%\x08\x08\x08 13%"),
            Some(13)
        );
        assert_eq!(parse_tool_percent(" 27% 3 - file.bin"), Some(27));
        assert_eq!(parse_tool_percent("All OK"), None);
        assert_eq!(parse_tool_percent("999%"), Some(100));
    }

    #[test]
    fn percent_saturates_and_handles_unknown_totals() {
        assert_eq!(percent_of(50, Some(200)), Some(25));
        assert_eq!(percent_of(500, Some(200)), Some(100));
        assert_eq!(percent_of(5, None), None);
        assert_eq!(percent_of(5, Some(0)), None);
    }
}
