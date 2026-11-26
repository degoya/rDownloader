//! An internal error logs why it happened, not only which step failed (audit 1.9.1, API-06),
//! and still answers with nothing of it.

use std::{
    fmt::Debug,
    sync::{Arc, Mutex},
};

use anyhow::Context as _;
use axum::http::StatusCode;
use tracing::{
    Event, Metadata, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};

use super::ApiError;

/// Collects the fields of every event as `name=value` text; no tracing-subscriber needed.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<String>>>);

struct Fields(String);

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.0.push_str(&format!("{}={value:?} ", field.name()));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.push_str(&format!("{}={value} ", field.name()));
    }
}

impl Subscriber for Recorder {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut fields = Fields(format!("{} ", event.metadata().level()));
        event.record(&mut fields);
        self.0.lock().expect("recorder").push(fields.0);
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

#[test]
fn an_internal_error_logs_its_whole_cause_chain_and_answers_none_of_it() {
    let failure = Err::<(), _>(std::io::Error::other("the disk is full"))
        .context("write the package row")
        .expect_err("a failure");
    let recorder = Recorder::default();
    let error = tracing::subscriber::with_default(recorder.clone(), || ApiError::from(failure));
    let lines = recorder.0.lock().expect("recorder").clone();

    assert!(
        lines
            .iter()
            .any(|line| line.contains("write the package row: the disk is full")),
        "the cause is missing from the log: {lines:?}"
    );
    assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(error.code(), crate::error_codes::INTERNAL_ERROR);
    assert!(
        !error.message().contains("disk"),
        "the cause reached the answer: {}",
        error.message()
    );
}

/// RD-1240-36: a stored credential the vault master key cannot open answers its own code, `409`
/// -- through every context a caller added -- and never `internal.error`.
#[tokio::test]
async fn an_unreadable_secret_answers_its_code_not_an_internal_error() {
    let directory = tempfile::tempdir().expect("tempdir");
    let vault = rd_secrets::SecretStore::open(directory.path().to_owned())
        .await
        .expect("vault");
    let reference = rd_secrets::SecretStore::new_reference();
    std::fs::write(
        directory.path().join(format!(
            "{}.secret",
            reference.trim_start_matches("vault://")
        )),
        b"sealed elsewhere",
    )
    .expect("entry");
    let failure = vault
        .get(&reference)
        .await
        .context("load the NNTP server")
        .expect_err("unreadable");
    let error = ApiError::from(failure);

    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(error.code(), rd_secrets::SECRET_UNREADABLE);
    assert!(!error.message().contains("vault://"), "{}", error.message());
}

/// A `429` that names its wait says it in `Retry-After` as well; nothing else does (RD-1480-08).
#[test]
fn a_rate_limit_names_its_wait_in_the_header() {
    use axum::{http::header, response::IntoResponse as _};

    let header_of = |error: ApiError| {
        error
            .into_response()
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let limited = || ApiError::too_many_requests("auth.too_many_attempts", "Too many attempts");
    assert_eq!(
        header_of(limited().with_param("seconds", 42)).as_deref(),
        Some("42")
    );
    assert_eq!(header_of(limited()), None);
    assert_eq!(header_of(limited().with_param("seconds", "soon")), None);
    assert_eq!(
        header_of(ApiError::conflict("request.invalid_id", "Later").with_param("seconds", 42)),
        None
    );
}
