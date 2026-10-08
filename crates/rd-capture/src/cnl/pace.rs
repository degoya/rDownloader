//! How often a page may hand links over, and how much one hand-over may name (RD-1190-22).
//!
//! Every page may reach Click'n'Load without a click, and every hand-over becomes a LinkGrabber
//! batch with its online checks. The body limits bound one request; nothing bounded how many a
//! page sent, or how long the package name and the password inside one were. Real buttons send
//! one request per click, so the ceiling below is far above a person and far below a flood.

use std::{
    collections::VecDeque,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

/// Hand-overs one agent accepts within [`WINDOW`], across all its listeners.
pub(super) const MAX_HAND_OVERS: usize = 30;

/// The sliding window [`MAX_HAND_OVERS`] counts in.
pub(super) const WINDOW: Duration = Duration::from_secs(60);

/// The longest package name passed on, in characters -- the length a package name may have
/// in the queue; a longer one is cut there, not refused.
pub(super) const MAX_PACKAGE_NAME_CHARS: usize = 200;

/// The longest archive password passed on, in characters. A longer one is refused: a cut
/// password opens nothing.
pub(super) const MAX_PASSWORD_CHARS: usize = 1024;

/// The pace every listener of this process shares, so the IPv4 and the IPv6 listener do not
/// each grant the whole allowance.
pub(super) static SHARED: LazyLock<Arc<Pace>> =
    LazyLock::new(|| Arc::new(Pace::new(MAX_HAND_OVERS, WINDOW)));

/// A sliding-window count of accepted hand-overs.
pub(super) struct Pace {
    limit: usize,
    window: Duration,
    recent: Mutex<VecDeque<Instant>>,
}

impl Pace {
    pub(super) fn new(limit: usize, window: Duration) -> Self {
        Self {
            limit,
            window,
            recent: Mutex::new(VecDeque::new()),
        }
    }

    /// Whether one more hand-over fits at `now`; counts it when it does.
    pub(super) fn admit(&self, now: Instant) -> bool {
        let Ok(mut recent) = self.recent.lock() else {
            return false;
        };
        while recent
            .front()
            .is_some_and(|seen| now.saturating_duration_since(*seen) >= self.window)
        {
            recent.pop_front();
        }
        if recent.len() >= self.limit {
            return false;
        }
        recent.push_back(now);
        true
    }
}

/// The package name to pass on: trimmed, empty as none, cut at [`MAX_PACKAGE_NAME_CHARS`].
pub(super) fn package_name(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .chars()
                .take(MAX_PACKAGE_NAME_CHARS)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
}

/// The first non-empty line of `passwords`, or `Err` when it is longer than
/// [`MAX_PASSWORD_CHARS`].
pub(super) fn password(raw: Option<&str>) -> Result<Option<&str>, usize> {
    let first = raw.and_then(|value| value.lines().map(str::trim).find(|line| !line.is_empty()));
    match first {
        Some(line) if line.chars().count() > MAX_PASSWORD_CHARS => Err(line.chars().count()),
        other => Ok(other),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{MAX_PACKAGE_NAME_CHARS, MAX_PASSWORD_CHARS, Pace, package_name, password};

    #[test]
    fn the_pace_admits_its_allowance_and_then_waits_for_the_window() {
        let pace = Pace::new(3, Duration::from_secs(60));
        let start = Instant::now();
        for _ in 0..3 {
            assert!(pace.admit(start));
        }
        assert!(!pace.admit(start + Duration::from_secs(59)));
        assert!(pace.admit(start + Duration::from_secs(60)));
    }

    #[test]
    fn a_long_package_name_is_cut_and_a_long_password_refused() {
        let long = "n".repeat(MAX_PACKAGE_NAME_CHARS + 50);
        assert_eq!(
            package_name(Some(long.as_str())).map(|name| name.chars().count()),
            Some(MAX_PACKAGE_NAME_CHARS)
        );
        assert_eq!(package_name(Some("  ")), None);
        assert_eq!(package_name(Some(" Release ")).as_deref(), Some("Release"));

        let fits = format!("\n{}\nsecond", "p".repeat(MAX_PASSWORD_CHARS));
        assert_eq!(
            password(Some(fits.as_str())).map(|value| value.map(str::len)),
            Ok(Some(MAX_PASSWORD_CHARS))
        );
        let too_long = "p".repeat(MAX_PASSWORD_CHARS + 1);
        assert!(password(Some(too_long.as_str())).is_err());
        assert_eq!(password(None), Ok(None));
    }
}
