//! The one typed reader for the `service.settings` blob.
//!
//! The blob holds the whole runtime configuration, and every layer reads its own slice out of
//! it: bandwidth, storage capacity, media, gallery, torrent, post-processing, managed tools.
//! Each of those slices used to deserialize the blob itself, and each swallowed the failure
//! with its own `.ok()` or `unwrap_or_default()`. One renamed or retyped field then left the
//! service running with no bandwidth limits and no storage thresholds, with nothing logged and
//! nothing failing — twenty-one independent places to fail silently.
//!
//! Everything here goes through [`report`], so a malformed slice is named once with the serde
//! error that rejected it. What happens after the report is the caller's decision, and the two
//! accessors exist because that decision genuinely differs:
//!
//! * [`Database::service_settings`] refuses. For start-up and one-shot reads, where an
//!   unusable configuration must stop the thing that needs it rather than be papered over.
//! * [`Database::service_settings_or_default`] falls back to the slice's defaults. For
//!   supervision loops and request handlers, where refusing would turn one bad field into a
//!   dead scheduler or a 500 on every poll. The report is what makes the fallback honest.
//!
//! Nothing here caches. Several callers re-read on purpose — switching metadata enrichment off
//! has to take effect on the next link, not after a restart — and a cache would have to invent
//! an invalidation rule that the writer does not offer today.

use std::{
    any::type_name,
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;

use crate::Database;

/// Settings key holding the runtime configuration blob.
pub const SERVICE_SETTINGS_KEY: &str = "service.settings";

impl Database {
    /// Reads one typed slice of the blob, refusing a malformed one.
    ///
    /// A missing blob (first start, before anything was ever saved) is not a failure: it reads
    /// as the slice's defaults, which is what every caller did by hand before.
    pub async fn service_settings<T>(&self) -> Result<T>
    where
        T: DeserializeOwned + Default,
    {
        let Some(blob) = self.get_setting(SERVICE_SETTINGS_KEY).await? else {
            return Ok(T::default());
        };
        parse_service_settings(&blob)
    }

    /// Reads one typed slice of the blob, falling back to its defaults after reporting.
    ///
    /// The error case is still an error, it is just not fatal here: it is logged once, with the
    /// slice it failed for and the field serde rejected, instead of vanishing.
    pub async fn service_settings_or_default<T>(&self) -> Result<T>
    where
        T: DeserializeOwned + Default,
    {
        let Some(blob) = self.get_setting(SERVICE_SETTINGS_KEY).await? else {
            return Ok(T::default());
        };
        Ok(parse_service_settings(&blob).unwrap_or_default())
    }

    /// Reads one typed field of the blob.
    ///
    /// An absent field and an absent blob both read as `None`. A field that is *present* with
    /// the wrong type is the silent case this exists for: it is reported and then also reads as
    /// `None`, so the caller keeps its own documented default for a missing setting.
    pub async fn service_setting_field<T>(&self, field: &str) -> Result<Option<T>>
    where
        T: DeserializeOwned,
    {
        let Some(blob) = self.get_setting(SERVICE_SETTINGS_KEY).await? else {
            return Ok(None);
        };
        Ok(service_setting_field_of(&blob, field))
    }
}

/// Deserializes one typed slice of an already-loaded blob, reporting a failure once.
///
/// Separate from the accessors for the callers that hold the blob already, because they read a
/// field out of the same `Value` for something else or loaded it inside their own transaction.
pub fn parse_service_settings<T>(blob: &serde_json::Value) -> Result<T>
where
    T: DeserializeOwned,
{
    let target = type_name::<T>();
    // Through `serde_path_to_error` so the report names the field. Several of these slices hold
    // types with their own deserializer -- `ByteCount` answers a bad value with "invalid digit
    // found in string" and nothing else -- so a plain `from_value` tells an operator which
    // struct failed and leaves them to guess which of its dozen fields did it.
    match serde_path_to_error::deserialize(blob.clone()) {
        Ok(parsed) => {
            clear_report(target);
            Ok(parsed)
        }
        Err(error) => {
            let path = error.path().to_string();
            let inner = error.into_inner();
            report(target, &format!("{path}: {inner}"));
            Err(inner)
                .with_context(|| format!("decode {target}.{path} from {SERVICE_SETTINGS_KEY}"))
        }
    }
}

/// Reads one typed field of an already-loaded blob, reporting a present-but-wrong-typed value.
///
/// An explicit `null` reads as absent, not as a wrong type. The settings document serializes
/// every `Option` field it holds, so saving the form once writes `"excluded_domains_file":
/// null` for "no file configured" -- and reading that as a malformed `String` logged an ERROR
/// on every LinkGrabber intake after an ordinary save (RD-120-48).
pub fn service_setting_field_of<T>(blob: &serde_json::Value, field: &str) -> Option<T>
where
    T: DeserializeOwned,
{
    let value = blob.get(field)?;
    if value.is_null() {
        clear_report(field);
        return None;
    }
    match serde_json::from_value(value.clone()) {
        Ok(parsed) => {
            clear_report(field);
            Some(parsed)
        }
        Err(error) => {
            report(field, &error);
            None
        }
    }
}

/// Last reported failure per target, so a supervision loop that reads the same broken blob
/// every few seconds logs once rather than filling the log with one line per cycle.
fn reported() -> &'static Mutex<HashMap<String, String>> {
    static REPORTED: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    REPORTED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Logs a malformed slice or field once per distinct failure.
///
/// Keyed by target *and* message: a blob that breaks in a new way after being fixed is a new
/// failure and says so, and [`clear_report`] forgets a target that parsed, so the same fault
/// reappearing later is reported again instead of being deduplicated against a stale entry.
fn report(target: &str, error: &dyn std::fmt::Display) {
    let message = error.to_string();
    let mut seen = reported().lock().unwrap_or_else(|poisoned| {
        // A panic while logging must not disable the reporting for the rest of the process.
        reported().clear_poison();
        poisoned.into_inner()
    });
    if seen.get(target).is_some_and(|last| *last == message) {
        return;
    }
    seen.insert(target.to_owned(), message.clone());
    drop(seen);
    tracing::error!(
        setting_target = target,
        error = %message,
        "a stored value in {SERVICE_SETTINGS_KEY} has the wrong type or shape; the reader either \
         refuses or falls back to its default until the stored value is corrected"
    );
}

/// Forgets a target that parsed, so the next failure is reported even if it reads the same.
fn clear_report(target: &str) {
    let mut seen = reported().lock().unwrap_or_else(|poisoned| {
        reported().clear_poison();
        poisoned.into_inner()
    });
    seen.remove(target);
}

/// The failure last reported for this target, for the test that proves a malformed blob is
/// reported rather than swallowed.
#[cfg(test)]
fn last_report(target: &str) -> Option<String> {
    let seen = reported().lock().expect("report registry");
    seen.get(target).cloned()
}

#[cfg(test)]
mod tests {
    use rd_core::StorageSettings;

    use super::{SERVICE_SETTINGS_KEY, last_report};
    use crate::Database;

    /// The whole point of the accessor: a retyped field must not read as "no thresholds
    /// configured" without a word in the log. If this ever passes with an empty report, the
    /// silent swallow is back.
    #[tokio::test]
    async fn a_malformed_slice_is_reported_and_then_defaults() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = Database::open(directory.path().join("settings.sqlite"))
            .await
            .expect("database");
        database
            .set_setting(
                SERVICE_SETTINGS_KEY.to_owned(),
                serde_json::json!({ "storage_minimum_free_bytes": "plenty" }),
            )
            .await
            .expect("settings");

        let target = std::any::type_name::<StorageSettings>();
        let settings: StorageSettings = database
            .service_settings_or_default()
            .await
            .expect("read falls back");
        assert_eq!(settings, StorageSettings::default());
        let reported = last_report(target).expect("the failure was reported, not swallowed");
        assert!(
            reported.contains("storage_minimum_free_bytes"),
            "the report names the field that failed: {reported}"
        );

        // The refusing accessor sees the same blob as an error rather than as defaults.
        let refused = database.service_settings::<StorageSettings>().await;
        assert!(refused.is_err(), "a malformed slice must not read as valid");

        // A blob that parses clears the target again, so a later regression is reported.
        database
            .set_setting(
                SERVICE_SETTINGS_KEY.to_owned(),
                serde_json::json!({ "storage_auto_resume": false }),
            )
            .await
            .expect("settings");
        let settings: StorageSettings = database.service_settings().await.expect("valid blob");
        assert!(!settings.storage_auto_resume);
        assert!(
            last_report(target).is_none(),
            "a good read clears the target"
        );
    }

    /// Own slice type, so this test's registry entry cannot race the one above.
    #[derive(Debug, Default, PartialEq, serde::Deserialize)]
    #[serde(default)]
    struct FirstStartSlice {}

    /// A missing blob is the first start, not a fault: no report, plain defaults.
    #[tokio::test]
    async fn a_missing_blob_reads_as_defaults_without_a_report() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = Database::open(directory.path().join("empty.sqlite"))
            .await
            .expect("database");
        let settings: FirstStartSlice = database.service_settings().await.expect("defaults");
        assert_eq!(settings, FirstStartSlice::default());
        assert!(last_report(std::any::type_name::<FirstStartSlice>()).is_none());
    }

    /// A present-but-wrong-typed switch used to read as "off" with nothing logged.
    #[tokio::test]
    async fn a_wrong_typed_field_is_reported_and_reads_as_absent() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = Database::open(directory.path().join("field.sqlite"))
            .await
            .expect("database");
        database
            .set_setting(
                SERVICE_SETTINGS_KEY.to_owned(),
                serde_json::json!({ "admin_login_disabled": "yes" }),
            )
            .await
            .expect("settings");
        let flag: Option<bool> = database
            .service_setting_field("admin_login_disabled")
            .await
            .expect("read");
        assert_eq!(flag, None);
        assert!(last_report("admin_login_disabled").is_some());
    }

    /// `null` is how the settings document stores an unset `Option`, so it is "not configured",
    /// never a wrong type. It used to log an ERROR on every read after an ordinary save
    /// (RD-120-48); a value that is really wrong-typed is still reported, as above.
    #[tokio::test]
    async fn a_null_field_reads_as_absent_without_a_report() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = Database::open(directory.path().join("null.sqlite"))
            .await
            .expect("database");
        database
            .set_setting(
                SERVICE_SETTINGS_KEY.to_owned(),
                serde_json::json!({ "null_field_probe": null }),
            )
            .await
            .expect("settings");
        let path: Option<String> = database
            .service_setting_field("null_field_probe")
            .await
            .expect("read");
        assert_eq!(path, None);
        assert!(
            last_report("null_field_probe").is_none(),
            "an unset optional setting is not a malformed one"
        );
    }
}
