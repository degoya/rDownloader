//! Changing the stored settings document: one writer at a time, the entity tag of what is
//! stored, and the event every change announces (RD-1480-07).

use std::sync::Arc;

use axum::http::{HeaderMap, header};
use rd_db::Database;

use crate::{ApiError, dto::SettingsResponse};

/// Held from reading the settings document to storing its successor.
///
/// Every writer reads the whole document, changes what it was asked to and stores the whole
/// document again. Two of them side by side each laid their change over the state before the
/// other's, and the first one was gone.
#[derive(Clone, Debug, Default)]
pub struct SettingsTurn(Arc<tokio::sync::Mutex<()>>);

impl SettingsTurn {
    /// Waits for the writers before this one.
    pub async fn take(&self) -> tokio::sync::OwnedMutexGuard<()> {
        Arc::clone(&self.0).lock_owned().await
    }
}

/// The strong entity tag of a settings document, quotes included.
pub fn settings_etag(settings: &SettingsResponse) -> Result<String, ApiError> {
    let bytes = serde_json::to_vec(settings).map_err(anyhow::Error::new)?;
    let digest = rd_authn::sha256_hex(bytes);
    Ok(format!("\"{}\"", digest.get(..32).unwrap_or(&digest)))
}

/// Refuses a write whose `If-Match` names another document than the stored one, as
/// `412 settings.document_changed`. No header, no condition.
///
/// A weakened tag matches as well: a proxy that compresses the answer weakens the tag in it, and
/// the client behind that proxy has no other one to send.
pub fn require_match(headers: &HeaderMap, stored: &SettingsResponse) -> Result<(), ApiError> {
    if !headers.contains_key(header::IF_MATCH) {
        return Ok(());
    }
    let etag = settings_etag(stored)?;
    let named = headers
        .get_all(header::IF_MATCH)
        .into_iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|tag| tag == "*" || tag.strip_prefix("W/").unwrap_or(tag) == etag);
    if named {
        return Ok(());
    }
    Err(ApiError::precondition_failed(
        "settings.document_changed",
        "The settings were changed since they were read; read them again",
    ))
}

/// The names of the settings fields whose stored value differs between two documents.
///
/// Compared as JSON rather than field by field so a field added later is covered without
/// anybody remembering to add it here, and so nothing in this function ever has to hold a
/// value long enough to log it by accident.
#[must_use]
pub fn changed_fields(before: &SettingsResponse, after: &SettingsResponse) -> Vec<String> {
    let (Ok(serde_json::Value::Object(before)), Ok(serde_json::Value::Object(after))) =
        (serde_json::to_value(before), serde_json::to_value(after))
    else {
        return Vec::new();
    };
    let mut names: Vec<String> = after
        .iter()
        .filter(|(key, value)| before.get(*key) != Some(*value))
        .map(|(key, _)| key.clone())
        .collect();
    names.extend(
        before
            .keys()
            .filter(|key| !after.contains_key(*key))
            .cloned(),
    );
    names.sort();
    names
}

/// Tells every open view that the stored document is another one (`settings.changed`).
///
/// The payload names the fields and never their values: the document holds secret references,
/// paths and proxy addresses, and a subscriber reads the document itself at the scope the
/// document costs. `before` is `None` when the previous document could not be read; the list is
/// then empty, which says that something changed and not what. A write that changed nothing
/// announces nothing. Broadcast only: the stored document is the state, not the event.
pub fn announce(database: &Database, before: Option<&SettingsResponse>, after: &SettingsResponse) {
    let fields = match before {
        Some(before) => {
            let fields = changed_fields(before, after);
            if fields.is_empty() {
                return;
            }
            fields
        }
        None => Vec::new(),
    };
    database.broadcast(rd_core::EventEnvelope::new(
        rd_core::EventKind::SettingsChanged,
        serde_json::json!({ "fields": fields }),
    ));
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, header};

    use super::{changed_fields, require_match, settings_etag};
    use crate::dto::SettingsResponse;

    fn document(max_retries: u32) -> SettingsResponse {
        SettingsResponse {
            max_retries,
            ..SettingsResponse::default()
        }
    }

    fn if_match(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(header::IF_MATCH, value.parse().expect("header value"));
        }
        headers
    }

    #[test]
    fn the_tag_is_the_documents_and_survives_a_round_trip_through_the_store() {
        let tag = settings_etag(&document(4)).expect("tag");
        assert!(tag.starts_with('"') && tag.ends_with('"'), "{tag}");
        assert_ne!(tag, settings_etag(&document(5)).expect("tag"));
        let stored = serde_json::to_value(document(4)).expect("serialize");
        let read: SettingsResponse = serde_json::from_value(stored).expect("parse");
        assert_eq!(tag, settings_etag(&read).expect("tag"));
    }

    #[test]
    fn a_condition_holds_for_the_stored_tag_a_weakened_one_and_a_star() {
        let stored = document(4);
        let tag = settings_etag(&stored).expect("tag");
        for headers in [
            HeaderMap::new(),
            if_match(&[&tag]),
            if_match(&[&format!("W/{tag}")]),
            if_match(&["\"other\"", &tag]),
            if_match(&[&format!("\"other\", {tag}")]),
            if_match(&["*"]),
        ] {
            assert!(require_match(&headers, &stored).is_ok(), "{headers:?}");
        }
        for headers in [if_match(&["\"other\""]), if_match(&[""])] {
            let refused = require_match(&headers, &stored).expect_err("another document");
            assert_eq!(refused.code(), "settings.document_changed");
        }
    }

    #[test]
    fn the_changed_fields_are_names_and_a_value_taken_away_counts() {
        assert!(changed_fields(&document(4), &document(4)).is_empty());
        let mut templated = document(5);
        templated.media_output_template = Some("%(title)s".to_owned());
        assert_eq!(
            changed_fields(&document(4), &templated),
            ["max_retries", "media_output_template"]
        );
        // `skip_serializing_if` leaves the cleared template out of the document altogether.
        assert_eq!(
            changed_fields(&templated, &document(5)),
            ["media_output_template"]
        );
    }
}
