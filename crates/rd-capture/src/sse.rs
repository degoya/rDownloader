//! Framing for the capture-scoped event stream.
//!
//! The agent reads one capture-scoped stream, for desktop notifications about arriving links.
//! The framing stays in its own module because the stream carries other events too — a captcha
//! signal among them — and a reader has to skip what it does not act on. Nothing here talks to
//! the network: it turns bytes into a parsed frame and stops there, which is also why it is
//! testable on every host.
//!
//! The reconnect policy lives here too. `retry:` is a field of the stream, so what the agent
//! does with it belongs beside the parser that reads it rather than in the watcher that
//! happens to hold the connection.
//!
//! It follows the `text/event-stream` specification rather than what `rd-api` happens to send
//! today. The stream is a contract with whatever is on the other end — a proxy that normalises
//! line endings, a later service, a payload that grows a newline — and every place this parser
//! used to take the easier reading was a place where such a change would have lost events
//! silently instead of loudly (RD-109-09).

use std::{
    hash::{BuildHasher, RandomState},
    time::Duration,
};

/// Cap on unterminated stream data held while waiting for a frame boundary. A well-behaved
/// server never comes close; this only stops a broken peer from growing the buffer forever.
pub(crate) const MAX_PENDING_BYTES: usize = 64 * 1024;

/// Offset just past the first frame terminator.
///
/// A frame ends at a blank line, and a line ends at `\n`, `\r\n` or a lone `\r` — so `\n\n`,
/// `\r\n\r\n`, `\r\r` and the mixed forms all terminate one. This walks the line endings
/// instead of searching for two fixed byte pairs, which is what used to make a `\r\r` stream
/// unreadable: no boundary was ever found, the buffer grew to [`MAX_PENDING_BYTES`], and the
/// watcher reconnected into the same wall forever without a single event arriving.
///
/// A `\r` at the very end of the buffer is deliberately not a boundary: the next chunk may
/// begin with `\n`, which would make it one line ending rather than two.
pub(crate) fn find_frame_end(buffer: &[u8]) -> Option<usize> {
    let mut index = 0;
    let mut line_start = 0;
    while index < buffer.len() {
        let past_ending = match buffer[index] {
            b'\n' => index + 1,
            b'\r' if buffer.get(index + 1) == Some(&b'\n') => index + 2,
            // Ambiguous until the next chunk says whether a `\n` follows.
            b'\r' if index + 1 == buffer.len() => return None,
            b'\r' => index + 1,
            _ => {
                index += 1;
                continue;
            }
        };
        if index == line_start {
            return Some(past_ending);
        }
        line_start = past_ending;
        index = past_ending;
    }
    None
}

/// One frame of the stream, with every field the specification defines.
///
/// `id` and `retry` are kept rather than acted on here: `retry` is the service's say over the
/// agent's reconnect interval, and `id` is the resume point for the day `rd-api` offers one.
/// Dropping them was how a service that knows it is overloaded had no way to tell its agents
/// to back off.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Frame {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
    pub retry: Option<Duration>,
}

/// Parses one frame's fields.
///
/// Three details are the specification's and not the obvious reading: several `data:` lines of
/// one frame join with `\n` between them rather than running together, exactly one leading
/// space is removed from a value rather than all of them, and a line without a colon is a field
/// with an empty value. All three only bite once a payload stops being a single line of
/// space-free JSON — and then they bite as an unparsable payload, not as an error.
pub(crate) fn parse_frame(frame: &str) -> Frame {
    let mut parsed = Frame::default();
    // Splitting on both characters leaves an empty string inside every `\r\n`. That is
    // harmless: an empty line carries no field, and a whole frame is parsed at once here, so
    // the blank line that ends it is nothing to dispatch on either.
    for line in frame.split(['\r', '\n']) {
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => parsed.event = Some(value.to_owned()),
            "data" => {
                if !parsed.data.is_empty() {
                    parsed.data.push('\n');
                }
                parsed.data.push_str(value);
            }
            // A NUL in an id is the one case the specification tells a parser to ignore.
            "id" => {
                if !value.contains('\0') {
                    parsed.id = Some(value.to_owned());
                }
            }
            "retry" => {
                if let Ok(milliseconds) = value.parse::<u64>() {
                    parsed.retry = Some(Duration::from_millis(milliseconds));
                }
            }
            _ => {}
        }
    }
    parsed
}

const FIRST_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Fraction of a reconnect delay used as spread, in percent.
///
/// The same ±10% `rd-subscription`'s schedule uses, and for the same reason: without it every
/// watcher that lost the connection to a restarting service comes back at exactly 2, 4, 8 …
/// seconds, all together, and hands the service its whole load in one moment.
const JITTER_PERCENT: u64 = 10;

/// When to try the stream again after it dropped.
///
/// Exponential, bounded, spread — and overridable by the service, which is what the stream's
/// `retry:` field is for. The spread is deterministic in a per-watcher seed rather than random:
/// the point is that two watchers with the same failure history wait different lengths, not
/// that either is unpredictable, and a deterministic rule is one a test can hold still.
pub(crate) struct Reconnect {
    backoff: Duration,
    requested: Option<Duration>,
    seed: u64,
    attempt: u64,
}

impl Reconnect {
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            backoff: FIRST_BACKOFF,
            requested: None,
            seed,
            attempt: 0,
        }
    }

    /// A seed of this watcher's own, so two agents restarted together do not march in step.
    pub(crate) fn seeded() -> Self {
        Self::new(RandomState::new().hash_one(0_u8))
    }

    /// Adopts the interval the service asked for, capped like the local backoff.
    pub(crate) fn requested(&mut self, interval: Duration) {
        self.requested = Some(interval.min(MAX_BACKOFF));
    }

    /// How long to wait before the next attempt.
    pub(crate) fn delay(&mut self) -> Duration {
        self.attempt = self.attempt.saturating_add(1);
        let base = self.requested.unwrap_or(self.backoff);
        // A `retry:` is the service's instruction, so it is not doubled behind its back; only
        // the agent's own guess grows.
        if self.requested.is_none() {
            self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
        }
        spread(base, self.seed, self.attempt)
    }
}

/// Deterministic spread of ±[`JITTER_PERCENT`] around a delay.
fn spread(delay: Duration, seed: u64, attempt: u64) -> Duration {
    let milliseconds = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
    let span = milliseconds / 100 * JITTER_PERCENT;
    if span == 0 {
        return delay;
    }
    // The same cheap integer mix `rd-subscription::schedule` uses: neighbouring seeds must not
    // produce neighbouring offsets, or the spreading does nothing.
    let mixed = seed
        .wrapping_add(attempt)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .rotate_left(31)
        .wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let offset = mixed % (span * 2 + 1);
    Duration::from_millis(milliseconds.saturating_sub(span).saturating_add(offset))
}

#[cfg(test)]
mod tests {
    use super::{Frame, Reconnect, find_frame_end, parse_frame, spread};
    use std::time::Duration;

    #[test]
    fn a_frame_ends_at_the_first_blank_line_in_either_line_ending() {
        assert_eq!(find_frame_end(b"data: 1\n\nrest"), Some(9));
        assert_eq!(find_frame_end(b"data: 1\r\n\r\nrest"), Some(11));
        assert_eq!(find_frame_end(b"data: 1\n"), None);
    }

    /// The specification allows a lone `\r` as a line ending, so `\r\r` ends a frame. A peer
    /// that sends it used to stall the reader until the oversized-frame guard tripped.
    #[test]
    fn a_lone_carriage_return_pair_ends_a_frame() {
        assert_eq!(find_frame_end(b"data: 1\r\rrest"), Some(9));
    }

    #[test]
    fn the_mixed_line_endings_end_a_frame_too() {
        assert_eq!(find_frame_end(b"data: 1\r\n\nrest"), Some(10));
        assert_eq!(find_frame_end(b"data: 1\n\r\nrest"), Some(10));
        assert_eq!(find_frame_end(b"data: 1\n\rrest"), Some(9));
    }

    /// A `\r` at the edge of a chunk may still turn out to be the first half of a `\r\n`.
    #[test]
    fn a_trailing_carriage_return_waits_for_the_next_chunk() {
        assert_eq!(find_frame_end(b"data: 1\r\r"), None);
        assert_eq!(find_frame_end(b"data: 1\r\r\n"), Some(10));
    }

    #[test]
    fn a_frame_carries_its_event_and_data() {
        assert_eq!(
            parse_frame("id: 1\nevent: captcha.changed\ndata: {\"widgets\":2}\n\n"),
            Frame {
                event: Some("captcha.changed".to_owned()),
                data: "{\"widgets\":2}".to_owned(),
                id: Some("1".to_owned()),
                retry: None,
            }
        );
    }

    #[test]
    fn a_comment_and_an_empty_frame_carry_nothing() {
        assert_eq!(parse_frame(": keep-alive\n\n"), Frame::default());
        assert_eq!(
            parse_frame("event: captcha.changed\n\n").data,
            String::new()
        );
    }

    /// Without the newline the two lines would arrive run together, and a JSON payload that
    /// contains a line break would arrive as invalid JSON and be dropped without a word.
    #[test]
    fn several_data_lines_join_with_a_newline() {
        assert_eq!(parse_frame("data: {\ndata: }\n\n").data, "{\n}");
        assert_eq!(parse_frame("data: one\ndata: two\n\n").data, "one\ntwo");
    }

    /// Exactly one space, not every space: a payload that starts with indentation keeps it.
    #[test]
    fn exactly_one_leading_space_is_removed_from_a_value() {
        assert_eq!(parse_frame("data:  two spaces\n\n").data, " two spaces");
        assert_eq!(parse_frame("data:no space\n\n").data, "no space");
        assert_eq!(parse_frame("data\n\n").data, String::new());
    }

    #[test]
    fn a_retry_field_is_read_as_a_reconnect_interval() {
        assert_eq!(
            parse_frame("retry: 4500\ndata: x\n\n").retry,
            Some(Duration::from_millis(4500))
        );
        assert_eq!(parse_frame("retry: soon\ndata: x\n\n").retry, None);
    }

    #[test]
    fn an_id_with_a_nul_is_ignored_rather_than_kept() {
        assert_eq!(parse_frame("id: 7\ndata: x\n\n").id, Some("7".to_owned()));
        assert_eq!(parse_frame("id: 7\0\ndata: x\n\n").id, None);
    }

    /// The failure history is identical; only the seed differs. Without the spread both
    /// watchers came back at exactly the same moment, which is what turns a service restart
    /// into a thundering herd.
    #[test]
    fn two_watchers_with_the_same_failure_history_wait_different_lengths() {
        let mut first = Reconnect::new(1);
        let mut second = Reconnect::new(2);
        let delays: Vec<(Duration, Duration)> =
            (0..4).map(|_| (first.delay(), second.delay())).collect();
        assert!(
            delays.iter().any(|(left, right)| left != right),
            "two seeds produced the same four delays: {delays:?}"
        );
    }

    #[test]
    fn the_spread_stays_within_a_tenth_of_the_delay() {
        let base = Duration::from_secs(8);
        for seed in 0..64_u64 {
            let spread = spread(base, seed, 1);
            assert!(
                spread >= Duration::from_millis(7_200) && spread <= Duration::from_millis(8_800),
                "seed {seed} left the ±10% band: {spread:?}"
            );
        }
    }

    /// The service's `retry:` is an instruction, not a suggestion: it replaces the agent's own
    /// guess and is not doubled behind the service's back.
    #[test]
    fn a_retry_from_the_service_sets_the_reconnect_interval() {
        let mut reconnect = Reconnect::new(7);
        let local = reconnect.delay();
        reconnect.requested(Duration::from_secs(20));
        for _ in 0..4 {
            let delay = reconnect.delay();
            assert!(
                delay >= Duration::from_secs(18) && delay <= Duration::from_secs(22),
                "a requested interval was not honoured: {delay:?}"
            );
        }
        assert!(
            local < Duration::from_secs(18),
            "the local backoff is smaller"
        );
    }
}
