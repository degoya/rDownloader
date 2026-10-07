//! The stored service settings document, for every caller that reads it.

use crate::{ApiError, AppState, dto::SettingsResponse};

pub async fn read_settings(state: &AppState) -> Result<SettingsResponse, ApiError> {
    stored_settings(&state.database).await
}

/// The settings blob for callers that hold a database but no `AppState`, such as the
/// hotfolder watcher.
pub async fn stored_settings(database: &rd_db::Database) -> Result<SettingsResponse, ApiError> {
    // Refuses a malformed blob with `settings.invalid`, as before: this is what the settings
    // view reads, and showing silent defaults there would invite saving them back over the
    // stored configuration.
    let value = database.get_setting(rd_db::SERVICE_SETTINGS_KEY).await?;
    value
        .map(|blob| rd_db::parse_service_settings(&blob))
        .transpose()
        .map_err(|error| ApiError::bad_request("settings.invalid", error.to_string()))
        .map(Option::unwrap_or_default)
}

/// The scheduler's runtime settings this settings document expresses.
///
/// The one mapping (audit 1.9.1, INTAKE-06): the binary kept a mirror of the document with
/// defaults of its own and clamped `max_retries` at start-up, while saving validated it — two
/// readings of one setting. Both now map here and both validate the result with
/// `SchedulerHandle::validate_runtime_settings`, so a value saving refuses is also one a start
/// refuses, instead of being quietly bent into range.
#[must_use]
pub fn runtime_settings(settings: &SettingsResponse) -> rd_scheduler::RuntimeSettings {
    rd_scheduler::RuntimeSettings {
        max_active_files: settings.max_active_files as usize,
        max_chunks_per_file: settings.max_chunks_per_file as usize,
        max_connections_per_host: settings.max_connections_per_host as usize,
        external_connections_per_file: settings.nntp_connections_per_file as usize,
        external_parallel_files: settings.nntp_parallel_files as usize,
        speed_limit_bytes_per_second: settings
            .speed_limit_bytes_per_second
            .map(rd_core::ByteCount::get),
        upload_limit_bytes_per_second: settings
            .upload_limit_bytes_per_second
            .map(rd_core::ByteCount::get),
        generate_sha256: settings.generate_sha256,
        global_proxy_profile_id: settings.global_proxy_profile_id,
        custom_ca_pem: settings.custom_ca_pem.clone(),
        max_retries: settings.max_retries,
        pause_during_postprocess: settings.pause_during_postprocess,
        auto_retry_failed: settings.auto_retry_failed,
        auto_retry_interval_hours: settings.auto_retry_interval_hours,
        auto_retry_max_rounds: settings.auto_retry_max_rounds,
        disabled_kinds: service_switches(settings).disabled_kinds(),
    }
}

/// The fields of the settings document [`runtime_settings`] reads: the slice a start refuses
/// to run without (owner, 2026-10-04, RA-DB-02: only runtime values are strict).
///
/// Every other field reads as its default at a start when it does not parse, with a warning
/// naming it; refusing the start over one of those would leave an installation that cannot be
/// repaired from its own settings view.
pub const RUNTIME_FIELDS: &[&str] = &[
    "max_active_files",
    "max_chunks_per_file",
    "max_connections_per_host",
    "nntp_connections_per_file",
    "nntp_parallel_files",
    "speed_limit_bytes_per_second",
    "upload_limit_bytes_per_second",
    "generate_sha256",
    "global_proxy_profile_id",
    "custom_ca_pem",
    "max_retries",
    "pause_during_postprocess",
    "auto_retry_failed",
    "auto_retry_interval_hours",
    "auto_retry_max_rounds",
    "torrent_service_enabled",
    "usenet_service_enabled",
    "media_service_enabled",
    "gallery_service_enabled",
    "recording_service_enabled",
    "remote_service_enabled",
];

/// A stored settings value the service does not start with, and how to repair it.
///
/// There is no settings command on the command line and the settings view needs a running
/// service, so the repair named is removing the one value with the service stopped; it then
/// reads as its default.
#[derive(Debug)]
pub struct RefusedSetting {
    /// The top-level field, `None` for the document as a whole.
    pub field: Option<String>,
    pub reason: String,
}

impl std::fmt::Display for RefusedSetting {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let key = rd_db::SERVICE_SETTINGS_KEY;
        match &self.field {
            Some(field) => write!(
                formatter,
                "the stored setting `{field}` ({key}) cannot be used: {}. To repair it, stop \
                 the service and remove the value so it reads as its default, for example with \
                 the sqlite3 shell on the database file: UPDATE settings SET value_json = \
                 json_remove(value_json, '$.{field}') WHERE key = '{key}'",
                self.reason
            ),
            None => write!(
                formatter,
                "the stored settings document ({key}) cannot be used: {}. To repair it, stop \
                 the service and remove it so every setting reads as its default, for example \
                 with the sqlite3 shell on the database file: DELETE FROM settings WHERE key = \
                 '{key}'",
                self.reason
            ),
        }
    }
}

impl std::error::Error for RefusedSetting {}

/// The settings document and runtime settings a start runs with.
///
/// The runtime slice ([`RUNTIME_FIELDS`]) is strict: a value that does not parse, or that
/// `SchedulerHandle::validate_runtime_settings` refuses, refuses the start and names the field.
/// Any other field that does not parse reads as its default (`parse_service_settings_per_field`
/// warns with its name).
pub async fn startup_settings(
    database: &rd_db::Database,
) -> anyhow::Result<(SettingsResponse, rd_scheduler::RuntimeSettings)> {
    let blob = database.get_setting(rd_db::SERVICE_SETTINGS_KEY).await?;
    Ok(startup_settings_of(blob.as_ref())?)
}

/// What `rdownloader doctor` reads: the document with every field that does not parse at its
/// default, and the reason a start would refuse it, if one would.
pub async fn diagnosed_settings(
    database: &rd_db::Database,
) -> anyhow::Result<(SettingsResponse, Option<RefusedSetting>)> {
    let blob = database.get_setting(rd_db::SERVICE_SETTINGS_KEY).await?;
    Ok(match startup_settings_of(blob.as_ref()) {
        Ok((settings, _)) => (settings, None),
        Err(refused) => {
            let settings = blob
                .and_then(|blob| rd_db::parse_service_settings_per_field(&blob, &[]).ok())
                .unwrap_or_default();
            (settings, Some(refused))
        }
    })
}

/// [`startup_settings`] over an already-loaded blob; `None` is a first start.
pub fn startup_settings_of(
    blob: Option<&serde_json::Value>,
) -> Result<(SettingsResponse, rd_scheduler::RuntimeSettings), RefusedSetting> {
    let settings = match blob {
        Some(blob) => {
            rd_db::parse_service_settings_per_field(blob, RUNTIME_FIELDS).map_err(|error| {
                RefusedSetting {
                    field: error.field.clone(),
                    reason: error.error.to_string(),
                }
            })?
        }
        None => SettingsResponse::default(),
    };
    let runtime = runtime_settings(&settings);
    match rd_scheduler::SchedulerHandle::validate_runtime_settings(&runtime) {
        Ok(()) => Ok((settings, runtime)),
        Err(error) => Err(RefusedSetting {
            field: refused_runtime_field(&settings).map(str::to_owned),
            reason: format!("{error:#}"),
        }),
    }
}

/// The runtime field the validation refuses: each one alone over the defaults, so the error
/// names the stored field (`nntp_parallel_files`) rather than the scheduler's own name for it
/// (`external_parallel_files`). `None` if no single field explains the refusal.
fn refused_runtime_field(settings: &SettingsResponse) -> Option<&'static str> {
    let stored = serde_json::to_value(settings).ok()?;
    let defaults = serde_json::to_value(SettingsResponse::default()).ok()?;
    RUNTIME_FIELDS.iter().copied().find(|field| {
        let mut candidate = defaults.clone();
        if let (Some(candidate), Some(value)) = (candidate.as_object_mut(), stored.get(*field)) {
            candidate.insert((*field).to_owned(), value.clone());
        }
        serde_json::from_value::<SettingsResponse>(candidate).is_ok_and(|alone| {
            rd_scheduler::SchedulerHandle::validate_runtime_settings(&runtime_settings(&alone))
                .is_err()
        })
    })
}

/// The service switches this settings document expresses.
pub fn service_switches(settings: &crate::dto::SettingsResponse) -> rd_core::ServiceSwitches {
    rd_core::ServiceSwitches {
        torrent: settings.torrent_service_enabled,
        usenet: settings.usenet_service_enabled,
        media: settings.media_service_enabled,
        gallery: settings.gallery_service_enabled,
        recording: settings.recording_service_enabled,
        remote: settings.remote_service_enabled,
    }
}

#[cfg(test)]
mod tests {
    use super::{RUNTIME_FIELDS, runtime_settings, startup_settings_of};
    use crate::dto::SettingsResponse;

    #[test]
    fn every_runtime_field_is_a_field_of_the_settings_document() {
        let document = serde_json::to_value(SettingsResponse::default()).expect("serialize");
        for field in RUNTIME_FIELDS {
            assert!(
                document.get(*field).is_some(),
                "{field} is no settings field"
            );
        }
    }

    /// RD-1140-05: the store reads the global package-name rules out of the blob by this name,
    /// so the document has to carry them under it, every switch off by default.
    #[test]
    fn the_package_name_rules_sit_where_the_store_reads_them() {
        let document = serde_json::to_value(SettingsResponse::default()).expect("serialize");
        let rules: rd_core::PackageNameRules =
            serde_json::from_value(document[rd_db::PACKAGE_NAME_RULES_FIELD].clone())
                .expect("package-name rules");
        assert_eq!(rules, rd_core::PackageNameRules::default());
        assert_eq!(
            document[rd_db::PACKAGE_NAME_REGEX_FIELD],
            serde_json::json!([])
        );
    }

    /// RA-DB-02, owner 2026-10-04: a value outside the runtime slice that no longer parses
    /// (here an enum variant a release removed) starts with its default, the rest as stored.
    #[test]
    fn an_unknown_enum_variant_outside_the_runtime_slice_starts_with_its_default() {
        let blob = serde_json::json!({
            "storage_collision_policy": "shred",
            "max_retries": 7,
        });
        let (settings, runtime) = startup_settings_of(Some(&blob)).expect("the start goes on");
        assert_eq!(
            settings.storage_collision_policy,
            SettingsResponse::default().storage_collision_policy
        );
        assert_eq!(settings.max_retries, 7);
        assert_eq!(runtime.max_retries, 7);
    }

    #[test]
    fn a_retry_count_of_500_refuses_the_start_and_names_the_field() {
        let blob = serde_json::json!({ "max_retries": 500 });
        let refused = startup_settings_of(Some(&blob)).err().expect("refused");
        assert_eq!(refused.field.as_deref(), Some("max_retries"));
        let message = refused.to_string();
        assert!(message.contains("`max_retries`"), "{message}");
        assert!(
            message.contains("json_remove(value_json, '$.max_retries')"),
            "{message}"
        );
    }

    /// The scheduler calls it `external_parallel_files`; the error names what is stored.
    #[test]
    fn a_refused_runtime_value_is_named_by_its_stored_field() {
        let blob = serde_json::json!({ "nntp_parallel_files": 9 });
        let refused = startup_settings_of(Some(&blob)).err().expect("refused");
        assert_eq!(refused.field.as_deref(), Some("nntp_parallel_files"));

        let blob = serde_json::json!({ "max_connections_per_host": "many" });
        let refused = startup_settings_of(Some(&blob))
            .err()
            .expect("a runtime type error refuses");
        assert_eq!(refused.field.as_deref(), Some("max_connections_per_host"));
    }

    /// RD-191-12: the automatic retry is a runtime value, so a stored interval outside its
    /// range refuses the start by name, and the defaults are off, six hours and three rounds.
    #[test]
    fn the_automatic_retry_is_part_of_the_strict_runtime_slice() {
        let (settings, runtime) = startup_settings_of(None).expect("defaults");
        assert!(!settings.auto_retry_failed);
        assert_eq!(runtime.auto_retry_interval_hours, 6);
        assert_eq!(runtime.auto_retry_max_rounds, 3);

        let blob =
            serde_json::json!({ "auto_retry_failed": true, "auto_retry_interval_hours": 12 });
        let (_, runtime) = startup_settings_of(Some(&blob)).expect("valid");
        assert!(runtime.auto_retry_failed);
        assert_eq!(runtime.auto_retry_interval_hours, 12);

        for (field, value) in [
            ("auto_retry_interval_hours", serde_json::json!(0)),
            ("auto_retry_interval_hours", serde_json::json!(25)),
            ("auto_retry_max_rounds", serde_json::json!(101)),
            ("auto_retry_failed", serde_json::json!("sometimes")),
        ] {
            let mut blob = serde_json::json!({});
            blob[field] = value;
            let refused = startup_settings_of(Some(&blob)).err().expect("refused");
            assert_eq!(refused.field.as_deref(), Some(field));
        }
    }

    /// RD-191-13: the NZB hand-over is shown in the LinkGrabber and in the Downloads view
    /// unless switched off, each on its own; a document written before the switches existed
    /// shows both, and a value that does not parse is no reason to refuse a start -- they are
    /// display choices outside the runtime slice.
    #[test]
    fn the_nzb_hand_over_is_shown_by_default_and_a_bad_value_reads_as_shown() {
        let defaults = SettingsResponse::default();
        assert!(defaults.nzb_hand_over_linkgrabber_enabled);
        assert!(defaults.nzb_hand_over_downloads_enabled);
        for field in [
            "nzb_hand_over_linkgrabber_enabled",
            "nzb_hand_over_downloads_enabled",
        ] {
            assert!(!RUNTIME_FIELDS.contains(&field), "{field}");
        }

        let (settings, _) = startup_settings_of(Some(&serde_json::json!({}))).expect("defaults");
        assert!(settings.nzb_hand_over_linkgrabber_enabled);
        assert!(settings.nzb_hand_over_downloads_enabled);

        let blob = serde_json::json!({ "nzb_hand_over_downloads_enabled": false });
        let (settings, _) = startup_settings_of(Some(&blob)).expect("stored");
        assert!(settings.nzb_hand_over_linkgrabber_enabled);
        assert!(!settings.nzb_hand_over_downloads_enabled);
        let document = serde_json::to_value(&settings).expect("serialize");
        assert_eq!(
            document["nzb_hand_over_downloads_enabled"],
            serde_json::json!(false)
        );

        let blob = serde_json::json!({
            "nzb_hand_over_linkgrabber_enabled": "sometimes",
            "nzb_hand_over_downloads_enabled": false,
            "max_retries": 7,
        });
        let (settings, runtime) = startup_settings_of(Some(&blob)).expect("the start goes on");
        assert!(settings.nzb_hand_over_linkgrabber_enabled);
        assert!(!settings.nzb_hand_over_downloads_enabled);
        assert_eq!(runtime.max_retries, 7);
    }

    #[test]
    fn a_first_start_without_a_document_runs_on_the_defaults() {
        let (settings, _) = startup_settings_of(None).expect("defaults");
        assert_eq!(
            settings.max_retries,
            SettingsResponse::default().max_retries
        );
    }

    #[test]
    fn the_defaults_map_to_runtime_settings_the_scheduler_accepts() {
        let runtime = runtime_settings(&SettingsResponse::default());
        rd_scheduler::SchedulerHandle::validate_runtime_settings(&runtime).expect("valid");
        assert_eq!(runtime.max_active_files, 3);
        assert_eq!(runtime.max_chunks_per_file, 4);
        assert_eq!(runtime.max_retries, rd_scheduler::DEFAULT_MAX_RETRIES);
    }

    #[test]
    fn a_retry_count_above_the_ceiling_is_refused_rather_than_bent_into_range() {
        // The binary clamped this at start-up while saving refused it (audit 1.9.1, INTAKE-06).
        let settings = SettingsResponse {
            max_retries: rd_scheduler::MAX_CONFIGURABLE_RETRIES + 1,
            ..SettingsResponse::default()
        };
        let runtime = runtime_settings(&settings);
        assert_eq!(
            runtime.max_retries,
            rd_scheduler::MAX_CONFIGURABLE_RETRIES + 1
        );
        assert!(rd_scheduler::SchedulerHandle::validate_runtime_settings(&runtime).is_err());
    }
}
