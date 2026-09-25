//! Schedule-wide bandwidth settings (part of the `service.settings` blob, keys prefixed
//! `bandwidth_`). The profiles and windows themselves live in their own tables.

use serde::{Deserialize, Serialize};

/// Timezone the weekly schedule is interpreted in when none is configured.
///
/// A real zone rather than UTC: quiet hours, bandwidth windows and reconnect windows are
/// written as wall-clock times, and UTC silently shifts them by an hour for half the year.
/// Somebody in another zone changes it once; somebody here never has to notice it exists.
pub const DEFAULT_BANDWIDTH_TIMEZONE: &str = "Europe/Berlin";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct BandwidthSettings {
    /// IANA timezone the schedule's local times and budget periods are read in.
    pub bandwidth_timezone: String,
    /// Profile applied whenever no window matches; empty = no limits outside the windows.
    pub bandwidth_default_profile_id: Option<crate::BandwidthProfileId>,
}

impl Default for BandwidthSettings {
    fn default() -> Self {
        Self {
            bandwidth_timezone: DEFAULT_BANDWIDTH_TIMEZONE.to_owned(),
            bandwidth_default_profile_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BandwidthSettings, DEFAULT_BANDWIDTH_TIMEZONE};

    #[test]
    fn a_blob_without_the_keys_schedules_in_utc() {
        let legacy: BandwidthSettings = serde_json::from_str("{}").expect("empty blob");
        assert_eq!(legacy.bandwidth_timezone, DEFAULT_BANDWIDTH_TIMEZONE);
        assert!(legacy.bandwidth_default_profile_id.is_none());
    }
}
