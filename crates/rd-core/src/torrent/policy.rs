//! Seeding policy with three levels of inheritance: the global settings, an optional
//! per-category override and an optional per-torrent override.
//!
//! Every field inherits independently, so a category can raise the ratio while the seed
//! time still comes from the global settings. [`EffectiveSeedingPolicy`] carries the
//! source of each field so the UI can show where a value came from.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::settings::TorrentSettings;

/// Lowest ratio a user may configure (0 disables the ratio stop entirely).
pub const MIN_SEED_RATIO: f64 = 0.0;

/// Highest ratio a user may configure.
pub const MAX_SEED_RATIO: f64 = 100.0;

/// Highest seed time a user may configure, in minutes (one year).
pub const MAX_SEED_TIME_MINUTES: u32 = 365 * 24 * 60;

/// How long a finished torrent keeps seeding.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SeedTimeLimit {
    /// Seed until the ratio target or a manual stop ends it.
    #[default]
    Unlimited,
    Minutes(u32),
}

impl SeedTimeLimit {
    /// Builds the limit from the nullable representation used in the settings blob.
    #[must_use]
    pub fn from_minutes(minutes: Option<u32>) -> Self {
        minutes.map_or(Self::Unlimited, Self::Minutes)
    }

    /// The nullable representation used in the settings blob.
    #[must_use]
    pub fn minutes(self) -> Option<u32> {
        match self {
            Self::Unlimited => None,
            Self::Minutes(minutes) => Some(minutes),
        }
    }
}

/// A partial seeding policy. Every `None` field inherits from the level above.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct SeedingPolicyOverride {
    pub enabled: Option<bool>,
    /// Stored as milli-ratio so the override stays `Eq` and round-trips exactly.
    pub ratio_milli: Option<u32>,
    pub time: Option<SeedTimeLimit>,
}

impl SeedingPolicyOverride {
    /// Whether the override sets nothing and can be dropped.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.enabled.is_none() && self.ratio_milli.is_none() && self.time.is_none()
    }

    /// The ratio as a floating point number.
    #[must_use]
    pub fn ratio(&self) -> Option<f64> {
        self.ratio_milli.map(|milli| f64::from(milli) / 1000.0)
    }

    /// Sets the ratio from a floating point number, rounding to the stored precision.
    pub fn set_ratio(&mut self, ratio: Option<f64>) {
        self.ratio_milli = ratio.map(|value| (value * 1000.0).round().max(0.0) as u32);
    }
}

/// Which level supplied one effective value.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PolicySource {
    Global,
    Category,
    Torrent,
}

/// The resolved policy applied to one torrent, with the origin of every field.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, ToSchema)]
pub struct EffectiveSeedingPolicy {
    pub enabled: bool,
    pub enabled_source: PolicySource,
    pub ratio: f64,
    pub ratio_source: PolicySource,
    pub time: SeedTimeLimit,
    pub time_source: PolicySource,
}

impl EffectiveSeedingPolicy {
    /// Whether the ratio target is reached for the given transfer counters.
    #[must_use]
    pub fn ratio_reached(&self, uploaded_bytes: u64, total_bytes: u64) -> bool {
        self.ratio > 0.0
            && total_bytes > 0
            && (uploaded_bytes as f64 / total_bytes as f64) >= self.ratio
    }

    /// Whether the seed time is exhausted after the given number of seeded seconds.
    #[must_use]
    pub fn time_reached(&self, seeded_seconds: u64) -> bool {
        match self.time {
            SeedTimeLimit::Unlimited => false,
            SeedTimeLimit::Minutes(minutes) => seeded_seconds >= u64::from(minutes) * 60,
        }
    }
}

/// Resolves global settings, category override and torrent override into one policy.
#[must_use]
pub fn resolve_seeding_policy(
    global: &TorrentSettings,
    category: Option<&SeedingPolicyOverride>,
    torrent: Option<&SeedingPolicyOverride>,
) -> EffectiveSeedingPolicy {
    let (enabled, enabled_source) = pick(
        global.torrent_seeding_enabled,
        category.and_then(|policy| policy.enabled),
        torrent.and_then(|policy| policy.enabled),
    );
    // Sharing is the outer switch. With it off the engine uploads nothing at all, so a
    // category or per-torrent override saying "seed this one" would promise something that
    // cannot happen — and would show a torrent as seeding while nothing leaves the machine.
    // Only an override that would have turned seeding on is overruled: when the inherited
    // decision is already "no", nothing is being overruled and the level that said so keeps
    // the credit, which is what the interface reports back.
    let (enabled, enabled_source) = if global.torrent_sharing_enabled || !enabled {
        (enabled, enabled_source)
    } else {
        (false, PolicySource::Global)
    };
    let (ratio, ratio_source) = pick(
        global.torrent_seed_ratio,
        category.and_then(SeedingPolicyOverride::ratio),
        torrent.and_then(SeedingPolicyOverride::ratio),
    );
    let (time, time_source) = pick(
        SeedTimeLimit::from_minutes(global.torrent_seed_time_minutes),
        category.and_then(|policy| policy.time),
        torrent.and_then(|policy| policy.time),
    );
    EffectiveSeedingPolicy {
        enabled,
        enabled_source,
        ratio,
        ratio_source,
        time,
        time_source,
    }
}

/// The most specific level that supplied a value wins.
fn pick<T>(global: T, category: Option<T>, torrent: Option<T>) -> (T, PolicySource) {
    if let Some(value) = torrent {
        return (value, PolicySource::Torrent);
    }
    if let Some(value) = category {
        return (value, PolicySource::Category);
    }
    (global, PolicySource::Global)
}

#[cfg(test)]
mod tests {
    use super::{
        PolicySource, SeedTimeLimit, SeedingPolicyOverride, TorrentSettings, resolve_seeding_policy,
    };

    fn global() -> TorrentSettings {
        TorrentSettings {
            torrent_seeding_enabled: true,
            // Seeding without sharing is not a configuration that exists: uploading is the
            // outer switch, and it is off by default.
            torrent_sharing_enabled: true,
            torrent_seed_ratio: 2.0,
            torrent_seed_time_minutes: Some(60),
            ..TorrentSettings::default()
        }
    }

    #[test]
    fn without_overrides_everything_comes_from_the_global_settings() {
        let policy = resolve_seeding_policy(&global(), None, None);
        assert!(policy.enabled);
        assert_eq!(policy.ratio, 2.0);
        assert_eq!(policy.time, SeedTimeLimit::Minutes(60));
        assert_eq!(policy.ratio_source, PolicySource::Global);
    }

    #[test]
    fn nothing_can_seed_while_sharing_is_off() {
        // The most specific level normally wins. Sharing is the exception: with uploading
        // off the engine sends nothing at all, so a category or per-torrent "seed this one"
        // would show a torrent as seeding while nothing leaves the machine.
        let settings = TorrentSettings {
            torrent_sharing_enabled: false,
            ..global()
        };
        let insistent = SeedingPolicyOverride {
            enabled: Some(true),
            ..SeedingPolicyOverride::default()
        };
        for (category, torrent) in [
            (None, None),
            (Some(&insistent), None),
            (None, Some(&insistent)),
            (Some(&insistent), Some(&insistent)),
        ] {
            let policy = resolve_seeding_policy(&settings, category, torrent);
            assert!(!policy.enabled, "an override re-enabled seeding");
            assert_eq!(policy.enabled_source, PolicySource::Global);
        }

        // An override that says "no" is not overruled by anything, so it keeps the credit:
        // reporting the global switch there would hide a decision the category did make.
        let refusing = SeedingPolicyOverride {
            enabled: Some(false),
            ..SeedingPolicyOverride::default()
        };
        let policy = resolve_seeding_policy(&settings, Some(&refusing), None);
        assert!(!policy.enabled);
        assert_eq!(policy.enabled_source, PolicySource::Category);
    }

    #[test]
    fn category_and_torrent_override_fields_independently() {
        let mut category = SeedingPolicyOverride::default();
        category.set_ratio(Some(0.5));
        let torrent = SeedingPolicyOverride {
            time: Some(SeedTimeLimit::Unlimited),
            ..SeedingPolicyOverride::default()
        };
        let policy = resolve_seeding_policy(&global(), Some(&category), Some(&torrent));
        assert_eq!(policy.ratio, 0.5);
        assert_eq!(policy.ratio_source, PolicySource::Category);
        assert_eq!(policy.time, SeedTimeLimit::Unlimited);
        assert_eq!(policy.time_source, PolicySource::Torrent);
        // Untouched fields still inherit from the global level.
        assert!(policy.enabled);
        assert_eq!(policy.enabled_source, PolicySource::Global);
    }

    #[test]
    fn the_torrent_level_beats_the_category_level() {
        let mut category = SeedingPolicyOverride::default();
        category.set_ratio(Some(0.5));
        let mut torrent = SeedingPolicyOverride::default();
        torrent.set_ratio(Some(3.25));
        let policy = resolve_seeding_policy(&global(), Some(&category), Some(&torrent));
        assert_eq!(policy.ratio, 3.25);
        assert_eq!(policy.ratio_source, PolicySource::Torrent);
    }

    #[test]
    fn an_emptied_override_falls_back_to_the_level_above() {
        let empty = SeedingPolicyOverride::default();
        assert!(empty.is_empty());
        let policy = resolve_seeding_policy(&global(), Some(&empty), Some(&empty));
        assert_eq!(policy.ratio_source, PolicySource::Global);
    }

    #[test]
    fn limits_decide_when_seeding_ends() {
        let policy = resolve_seeding_policy(&global(), None, None);
        assert!(!policy.ratio_reached(100, 100));
        assert!(policy.ratio_reached(200, 100));
        assert!(!policy.time_reached(3_599));
        assert!(policy.time_reached(3_600));
    }

    #[test]
    fn a_zero_ratio_never_stops_seeding() {
        let mut torrent = SeedingPolicyOverride::default();
        torrent.set_ratio(Some(0.0));
        let policy = resolve_seeding_policy(&global(), None, Some(&torrent));
        assert!(!policy.ratio_reached(u64::MAX, 1));
    }
}
