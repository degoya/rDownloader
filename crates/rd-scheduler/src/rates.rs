//! Live transfer rates, and the remaining time they imply.
//!
//! The rate used to be derived twice, independently and differently: the web UI smoothed the
//! change in committed bytes between two `/api/v1/downloads` reads, while the tray agent took
//! the raw difference between two `/api/v1/capture/summary` reads with no smoothing at all. A
//! third caller — MCP, an automation, a notification — would have been a third formula. It is
//! derived here instead, once, and everything reads the same figure.
//!
//! In memory only. A rate describes what is happening right now; after a restart there is
//! nothing to say until the queue has moved again, and persisting a stale figure would only
//! make the first estimate after a restart a wrong one.
//!
//! The smoothing matches what the browser did (`web/src/utils/transferRates.ts`), so the
//! displayed figure did not change when it moved here.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use rd_core::DownloadId;

/// Weight of the newest measurement in the exponential average.
///
/// Taken from the browser implementation this replaces. Lower means steadier and slower to
/// react; an estimate that jumps with every read is not an estimate.
pub const SMOOTHING_WEIGHT: f64 = 0.65;

/// How long an unchanged byte count keeps its last rate before it reads as zero.
///
/// Checkpoints are written less often than the sampler runs, so an unchanged counter usually
/// means "nothing was written yet", not "nothing moved". Holding the last figure briefly keeps
/// the display from alternating between a rate and zero.
pub const STALE_AFTER: Duration = Duration::from_secs(5);

/// One download's byte counter as the sampler last saw it.
#[derive(Clone, Copy, Debug)]
struct Sample {
    bytes: u64,
    /// When `bytes` was last observed to have changed — the base of the next measurement.
    sampled_at: Instant,
    /// When the counter last moved, which is what the staleness rule is measured against.
    last_progress_at: Instant,
    bytes_per_second: f64,
    transferring: bool,
}

/// What the sampler is handed per pass, for one download.
#[derive(Clone, Copy, Debug)]
pub struct RateObservation {
    pub id: DownloadId,
    pub committed_bytes: u64,
    /// Whether the state is one that actually moves bytes over the wire right now.
    pub transferring: bool,
}

/// Smoothed per-download rates, rebuilt from every sampling pass.
#[derive(Debug, Default)]
pub struct RateSampler {
    samples: Mutex<HashMap<DownloadId, Sample>>,
}

impl RateSampler {
    /// Folds one pass of byte counters into the running averages.
    ///
    /// Downloads absent from `observations` are forgotten, so the map never outgrows the queue.
    pub fn observe(&self, now: Instant, observations: &[RateObservation]) {
        let Ok(mut samples) = self.samples.lock() else {
            return;
        };
        let previous = std::mem::take(&mut *samples);
        for observation in observations {
            let next = next_sample(previous.get(&observation.id).copied(), *observation, now);
            samples.insert(observation.id, next);
        }
    }

    /// The current rate of every sampled download, in bytes per second.
    ///
    /// Entries that are not moving report zero rather than being left out, so a caller can tell
    /// "known to be still" apart from "never seen".
    #[must_use]
    pub fn rates(&self) -> HashMap<DownloadId, u64> {
        let Ok(samples) = self.samples.lock() else {
            return HashMap::new();
        };
        samples
            .iter()
            .map(|(id, sample)| (*id, sample.bytes_per_second.round().max(0.0) as u64))
            .collect()
    }
}

/// The exponential average, mirroring the browser implementation it replaces.
fn next_sample(prior: Option<Sample>, observation: RateObservation, now: Instant) -> Sample {
    let restart = Sample {
        bytes: observation.committed_bytes,
        sampled_at: now,
        last_progress_at: now,
        bytes_per_second: 0.0,
        transferring: observation.transferring,
    };
    let Some(prior) = prior else {
        return restart;
    };
    // A counter that fell went backwards for a reason we cannot measure across (a reset, a
    // restarted transfer); a state that is not transferring has no rate at all.
    if !observation.transferring || !prior.transferring || observation.committed_bytes < prior.bytes
    {
        return restart;
    }
    if observation.committed_bytes > prior.bytes {
        let elapsed = now
            .saturating_duration_since(prior.sampled_at)
            .as_secs_f64()
            .max(0.001);
        let current = (observation.committed_bytes - prior.bytes) as f64 / elapsed;
        let smoothed = if prior.bytes_per_second > 0.0 {
            current * SMOOTHING_WEIGHT + prior.bytes_per_second * (1.0 - SMOOTHING_WEIGHT)
        } else {
            current
        };
        return Sample {
            bytes: observation.committed_bytes,
            sampled_at: now,
            last_progress_at: now,
            bytes_per_second: smoothed,
            transferring: true,
        };
    }
    // Unchanged: keep the previous measurement window, and let the rate expire on its own.
    Sample {
        bytes_per_second: if now.saturating_duration_since(prior.last_progress_at) >= STALE_AFTER {
            0.0
        } else {
            prior.bytes_per_second
        },
        ..prior
    }
}

/// Seconds left at `bytes_per_second`, or `None` where there is nothing honest to say.
///
/// `None` covers all three blanks the estimate has to admit to: an unknown total size
/// (`remaining_bytes` is `None`), a transfer that is not moving, and a rate that has fallen to
/// zero. No infinity, no placeholder, no "calculating" — a number that would be wrong is worse
/// than no number.
#[must_use]
pub fn estimate_seconds(remaining_bytes: Option<u64>, bytes_per_second: u64) -> Option<u64> {
    let remaining = remaining_bytes?;
    if bytes_per_second == 0 {
        return None;
    }
    Some(remaining.div_ceil(bytes_per_second))
}

#[cfg(test)]
mod tests {
    use super::{RateObservation, RateSampler, estimate_seconds};
    use std::time::{Duration, Instant};

    fn observation(committed_bytes: u64, transferring: bool) -> RateObservation {
        RateObservation {
            id: rd_core::DownloadId::new(),
            committed_bytes,
            transferring,
        }
    }

    /// The same figures the browser implementation was tested with: 2 000 B/s measured, then
    /// 1 000 B/s smoothed against it gives 1 350 B/s.
    #[test]
    fn a_rate_is_smoothed_against_the_one_before_it() {
        let sampler = RateSampler::default();
        let id = rd_core::DownloadId::new();
        let start = Instant::now();
        let at = |bytes: u64| RateObservation {
            id,
            committed_bytes: bytes,
            transferring: true,
        };

        sampler.observe(start, &[at(100)]);
        sampler.observe(start + Duration::from_secs(1), &[at(2_100)]);
        assert_eq!(sampler.rates().get(&id).copied(), Some(2_000));

        sampler.observe(start + Duration::from_secs(2), &[at(3_100)]);
        assert_eq!(sampler.rates().get(&id).copied(), Some(1_350));
    }

    #[test]
    fn an_unchanged_counter_keeps_its_rate_briefly_and_then_reads_as_still() {
        let sampler = RateSampler::default();
        let id = rd_core::DownloadId::new();
        let start = Instant::now();
        let at = |bytes: u64| RateObservation {
            id,
            committed_bytes: bytes,
            transferring: true,
        };

        sampler.observe(start, &[at(0)]);
        sampler.observe(start + Duration::from_secs(1), &[at(1_000)]);
        sampler.observe(start + Duration::from_secs(3), &[at(1_000)]);
        assert_eq!(sampler.rates().get(&id).copied(), Some(1_000));

        sampler.observe(start + Duration::from_secs(6), &[at(1_000)]);
        assert_eq!(sampler.rates().get(&id).copied(), Some(0));
    }

    #[test]
    fn a_transfer_that_stops_transferring_loses_its_rate_at_once() {
        let sampler = RateSampler::default();
        let id = rd_core::DownloadId::new();
        let start = Instant::now();

        sampler.observe(
            start,
            &[RateObservation {
                id,
                committed_bytes: 0,
                transferring: true,
            }],
        );
        sampler.observe(
            start + Duration::from_secs(1),
            &[RateObservation {
                id,
                committed_bytes: 1_000,
                transferring: true,
            }],
        );
        sampler.observe(
            start + Duration::from_millis(1_100),
            &[RateObservation {
                id,
                committed_bytes: 1_000,
                transferring: false,
            }],
        );

        assert_eq!(sampler.rates().get(&id).copied(), Some(0));
    }

    /// A counter that fell (a reset, a restarted transfer) is not negative progress.
    #[test]
    fn a_falling_counter_starts_the_measurement_over_rather_than_going_negative() {
        let sampler = RateSampler::default();
        let id = rd_core::DownloadId::new();
        let start = Instant::now();
        let at = |bytes: u64| RateObservation {
            id,
            committed_bytes: bytes,
            transferring: true,
        };

        sampler.observe(start, &[at(10_000)]);
        sampler.observe(start + Duration::from_secs(1), &[at(20_000)]);
        sampler.observe(start + Duration::from_secs(2), &[at(500)]);

        assert_eq!(sampler.rates().get(&id).copied(), Some(0));
    }

    #[test]
    fn a_download_that_left_the_queue_is_forgotten() {
        let sampler = RateSampler::default();
        let first = observation(0, true);
        let start = Instant::now();

        sampler.observe(start, &[first]);
        sampler.observe(start + Duration::from_secs(1), &[observation(0, true)]);

        assert!(!sampler.rates().contains_key(&first.id));
    }

    #[test]
    fn an_estimate_is_the_remainder_over_the_rate() {
        assert_eq!(estimate_seconds(Some(10_000), 1_000), Some(10));
        // Rounded up: a part second left is still a second of waiting.
        assert_eq!(estimate_seconds(Some(10_001), 1_000), Some(11));
        assert_eq!(estimate_seconds(Some(0), 1_000), Some(0));
    }

    /// The three blanks, each of which must stay blank rather than become a number.
    #[test]
    fn an_unknown_size_yields_no_estimate() {
        assert_eq!(estimate_seconds(None, 5_000_000), None);
    }

    #[test]
    fn a_rate_of_zero_yields_no_estimate() {
        assert_eq!(estimate_seconds(Some(10_000), 0), None);
    }

    /// A paused transfer is observed as not transferring, so its rate is zero and no estimate
    /// follows from it.
    #[test]
    fn a_paused_transfer_yields_no_estimate() {
        let sampler = RateSampler::default();
        let id = rd_core::DownloadId::new();
        let start = Instant::now();

        sampler.observe(
            start,
            &[RateObservation {
                id,
                committed_bytes: 0,
                transferring: true,
            }],
        );
        sampler.observe(
            start + Duration::from_secs(1),
            &[RateObservation {
                id,
                committed_bytes: 5_000,
                transferring: true,
            }],
        );
        sampler.observe(
            start + Duration::from_secs(2),
            &[RateObservation {
                id,
                committed_bytes: 5_000,
                transferring: false,
            }],
        );

        let rate = sampler.rates().get(&id).copied().unwrap_or_default();
        assert_eq!(estimate_seconds(Some(1_000_000), rate), None);
    }
}
