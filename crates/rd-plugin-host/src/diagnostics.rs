//! Sanitised record of what a plugin invocation did.
//!
//! From the outside a misbehaving plugin looks like "the download failed again". The reason
//! existed, in a log line nobody kept, on a machine nobody was watching. This keeps the part
//! that can be acted on — which plugin, which version, which failure class, how long — and
//! puts the message through the same redaction every persisted failure goes through, so a
//! credential or a session token in a resolver's error text never reaches the table.
//!
//! Writing a record is best effort by construction. A download must never fail because its
//! diagnostics could not be written.

use std::{sync::Arc, time::Instant};

use rd_core::Failure;

/// Longest message kept. Long enough for a sentence, short enough that a plugin cannot use
/// the history as storage.
const MAX_MESSAGE_CHARS: usize = 300;

/// How one invocation ended, in terms a user can act on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionOutcome {
    /// The call returned a result.
    Ok,
    /// The plugin reported a failure of its own: the hoster said no, the link is dead. The
    /// plugin worked; kept apart from the classes below because a run of these means
    /// something quite different from a run of crashes.
    Failed,
    /// The sandbox stopped it: a trap, an unreachable, a panic in the guest.
    Crash,
    /// It ran out of wall-clock budget.
    Timeout,
    /// It ran out of instruction budget.
    Fuel,
    /// It asked for more memory than its manifest allows.
    Memory,
    /// A host function failed under it.
    HostError,
    /// It asked for something its manifest does not grant.
    Denied,
}

impl ExecutionOutcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Crash => "crash",
            Self::Timeout => "timeout",
            Self::Fuel => "fuel",
            Self::Memory => "memory",
            Self::HostError => "host_error",
            Self::Denied => "denied",
        }
    }

    /// Classifies a failure by the stable code the runtime attached to it.
    #[must_use]
    fn of(failure: &Failure) -> Self {
        match failure.code.as_deref() {
            Some("plugin.fuel_exhausted") => Self::Fuel,
            Some("plugin.timeout") => Self::Timeout,
            Some("plugin.trapped" | "plugin.execution_failed") => {
                // wasmtime reports a refused memory growth as an ordinary trap, so the only
                // thing separating the two is what it says about it.
                if failure.message.to_ascii_lowercase().contains("memory") {
                    Self::Memory
                } else {
                    Self::Crash
                }
            }
            Some(
                "plugin.capability_not_granted"
                | "plugin.url_outside_domains"
                | "plugin.http_target_not_allowed"
                | "plugin.redirect_outside_domains"
                | "plugin.resolved_url_not_allowed"
                | "plugin.secret_target_not_allowed"
                | "plugin.net_target_not_allowed"
                | "plugin.net_local_target"
                | "plugin.net_too_many_connections",
            ) => Self::Denied,
            Some(
                "plugin.host_disconnected"
                | "plugin.wait_budget_exhausted"
                | "plugin.http_timeout"
                | "plugin.response_too_large"
                | "plugin.sink_write_failed"
                | "plugin.sink_sync_failed"
                | "plugin.net_timeout"
                | "plugin.net_io_failed",
            ) => Self::HostError,
            _ => Self::Failed,
        }
    }
}

/// Where execution records go. `None` disables recording entirely, which is what the
/// packaging CLI and the tests use.
#[derive(Clone, Default)]
pub struct ExecutionLog {
    database: Option<rd_db::Database>,
}

impl ExecutionLog {
    #[must_use]
    pub fn new(database: rd_db::Database) -> Arc<Self> {
        Arc::new(Self {
            database: Some(database),
        })
    }

    /// A log that records nothing, for callers with no database at hand.
    #[must_use]
    pub fn disabled() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Starts timing one invocation of `operation`.
    #[must_use]
    pub fn begin(
        &self,
        plugin_id: &str,
        plugin_name: &str,
        plugin_version: &str,
        plugin_type: &str,
        operation: &'static str,
    ) -> Invocation {
        Invocation {
            plugin_id: plugin_id.to_owned(),
            plugin_name: plugin_name.to_owned(),
            plugin_version: plugin_version.to_owned(),
            plugin_type: plugin_type.to_owned(),
            operation,
            // A bare UUID: it names this entry and identifies nothing else, so quoting it in
            // a bug report gives away nothing about the download it came from.
            correlation_id: uuid::Uuid::now_v7().to_string(),
            started: Instant::now(),
            started_at: chrono::Utc::now(),
        }
    }

    /// Files a finished invocation. Never blocks the caller and never fails it.
    pub fn finish<T>(self: &Arc<Self>, invocation: Invocation, result: &Result<T, Failure>) {
        let (outcome, error_class, message) = match result {
            Ok(_) => (ExecutionOutcome::Ok, None, None),
            Err(failure) => {
                let redacted = rd_core::redact_failure(failure.clone());
                (
                    ExecutionOutcome::of(failure),
                    failure.code.clone(),
                    Some(redacted.message.chars().take(MAX_MESSAGE_CHARS).collect()),
                )
            }
        };
        if outcome != ExecutionOutcome::Ok {
            tracing::debug!(
                plugin = %invocation.plugin_name,
                plugin_id = %invocation.plugin_id,
                version = %invocation.plugin_version,
                operation = invocation.operation,
                correlation_id = %invocation.correlation_id,
                outcome = outcome.as_str(),
                "plugin invocation ended"
            );
        }
        let Some(database) = self.database.clone() else {
            return;
        };
        let entry = rd_db::NewPluginExecution {
            plugin_id: invocation.plugin_id,
            plugin_version: invocation.plugin_version,
            plugin_type: invocation.plugin_type,
            operation: invocation.operation.to_owned(),
            correlation_id: invocation.correlation_id,
            outcome: outcome.as_str().to_owned(),
            error_class,
            message,
            started_at: invocation.started_at,
            duration_ms: i64::try_from(invocation.started.elapsed().as_millis())
                .unwrap_or(i64::MAX),
        };
        tokio::spawn(async move {
            if let Err(error) = database.record_plugin_execution(entry).await {
                tracing::warn!(error = %error, "could not record a plugin execution");
            }
        });
    }
}

/// One invocation in flight.
pub struct Invocation {
    plugin_id: String,
    plugin_name: String,
    plugin_version: String,
    plugin_type: String,
    operation: &'static str,
    correlation_id: String,
    started: Instant,
    started_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use rd_core::FailureKind;

    use super::*;

    fn failure(code: &'static str, message: &str) -> Failure {
        Failure::coded(FailureKind::Permanent, code, message.to_owned())
    }

    /// The classes a user acts on differently must not collapse into one another.
    ///
    /// "It crashes" and "it is not allowed to do that" call for opposite responses — one is a
    /// bug report to the plugin's author, the other a manifest that promises too little — so
    /// the history has to tell them apart on its own.
    #[test]
    fn failures_are_classified_by_the_code_the_runtime_attached() {
        for (code, expected) in [
            ("plugin.fuel_exhausted", ExecutionOutcome::Fuel),
            ("plugin.timeout", ExecutionOutcome::Timeout),
            ("plugin.trapped", ExecutionOutcome::Crash),
            ("plugin.capability_not_granted", ExecutionOutcome::Denied),
            ("plugin.net_local_target", ExecutionOutcome::Denied),
            ("plugin.host_disconnected", ExecutionOutcome::HostError),
            ("plugin.sink_write_failed", ExecutionOutcome::HostError),
            // A hoster saying no is not a plugin malfunction, and a run of these means
            // something entirely different from a run of crashes.
            ("plugin.account_invalid", ExecutionOutcome::Failed),
            ("link.offline", ExecutionOutcome::Failed),
        ] {
            assert_eq!(
                ExecutionOutcome::of(&failure(code, "something happened")),
                expected,
                "{code}"
            );
        }
        assert_eq!(
            ExecutionOutcome::of(&failure(
                "plugin.trapped",
                "wasm trap: cannot grow memory beyond the limit"
            )),
            ExecutionOutcome::Memory
        );
    }

    /// The history exists to be quoted in a bug report, so what it stores has to be safe to
    /// quote. A resolver's failure message routinely carries the URL it was working on, and
    /// that URL routinely carries a session token or a basic-auth login.
    ///
    /// The message passes the same boundary every persisted failure passes
    /// (`rd_core::redact_failure`), so credentials and query values go and the host name
    /// stays — the host is what makes the entry diagnosable at all, and it is not a secret.
    #[tokio::test]
    async fn a_recorded_failure_keeps_the_class_and_drops_the_request() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = rd_db::Database::open(directory.path().join("db.sqlite"))
            .await
            .expect("database");
        let log = ExecutionLog::new(database.clone());
        let invocation = log.begin(
            "019d0000-0000-7000-8000-00000000abcd",
            "Example hoster",
            "0.7.0",
            "resolver",
            "resolve",
        );
        let result: Result<(), Failure> = Err(failure(
            "plugin.trapped",
            "failed at https://user:hunter2@files.example.test/download?token=abcdef123456",
        ));
        log.finish(invocation, &result);

        // The write is deliberately off the caller's path, so the test waits for it the same
        // way a reader would: by looking again.
        let mut stored = Vec::new();
        for _ in 0..50 {
            stored = database
                .plugin_executions("019d0000-0000-7000-8000-00000000abcd", 10)
                .await
                .expect("history");
            if !stored.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let entry = stored.first().expect("one entry");
        assert_eq!(entry.plugin_version, "0.7.0");
        assert_eq!(entry.plugin_type, "resolver");
        assert_eq!(entry.operation, "resolve");
        assert_eq!(entry.outcome, "crash");
        assert_eq!(entry.error_class.as_deref(), Some("plugin.trapped"));
        assert!(!entry.correlation_id.is_empty());
        let message = entry.message.as_deref().unwrap_or_default();
        for secret in ["hunter2", "abcdef123456"] {
            assert!(
                !message.contains(secret),
                "the history leaked `{secret}`: {message}"
            );
        }
    }

    /// A plugin that fails in a loop must not turn the history into a disk-space problem.
    #[tokio::test]
    async fn the_history_is_bounded_per_plugin() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = rd_db::Database::open(directory.path().join("db.sqlite"))
            .await
            .expect("database");
        for index in 0..(rd_db::MAX_EXECUTIONS_PER_PLUGIN + 20) {
            database
                .record_plugin_execution(rd_db::NewPluginExecution {
                    plugin_id: "plugin".to_owned(),
                    plugin_version: "0.7.0".to_owned(),
                    plugin_type: "resolver".to_owned(),
                    operation: "resolve".to_owned(),
                    correlation_id: format!("{index}"),
                    outcome: "crash".to_owned(),
                    error_class: None,
                    message: None,
                    started_at: chrono::Utc::now(),
                    duration_ms: 1,
                })
                .await
                .expect("record");
        }
        let stored = database
            .plugin_executions("plugin", rd_db::MAX_EXECUTIONS_PER_PLUGIN)
            .await
            .expect("history");
        assert_eq!(stored.len() as i64, rd_db::MAX_EXECUTIONS_PER_PLUGIN);
    }
}
