//! Writing audit events (RD-110-03).
//!
//! One function, [`record`], and the shapes it takes. Everything a handler knows about who
//! asked — the actor the authentication middleware established, the address the request came
//! from, the trace it belongs to — is already on the request, so a call site says only what
//! happened and to what.
//!
//! Three rules hold here rather than at each call site:
//!
//! * **Awaited, not detached.** The write goes through the serialized writer and the handler
//!   waits for it. An action that reports success has its record; a log line spawned into the
//!   background would not be able to promise that, which is the difference between an audit
//!   log and a log.
//! * **Redacted.** Every string a caller passes goes through `rd_core::redact_text`, and a
//!   detail whose *name* says it holds a credential is replaced whole, exactly as the capture
//!   layer does it. A canary test in `tests/audit.rs` holds the line.
//! * **Never a secret.** There is no field on [`AuditEvent`] that could hold a password, a
//!   token or a digest, because the only way to add one would be to add a field here.

use std::collections::BTreeMap;

use axum::http::request::Parts;
use rd_core::{AuditAction, AuditActorKind, AuditOutcome, TraceContext, redact_text};
use rd_db::NewAuditRecord;

use crate::AppState;

/// Longest stored string. The log store's limit, for the same reason: a name is useful at two
/// kilobytes and only bloats the store beyond that.
pub(crate) const MAX_VALUE_LENGTH: usize = 2048;

/// Who acted. Established once by the authentication middleware and carried on the request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Actor {
    pub kind: AuditActorKind,
    /// An opaque handle: a token id, a session id. Never a credential and never a digest.
    pub id: Option<String>,
    /// What a person called it, when there is such a name.
    pub label: Option<String>,
}

impl Actor {
    pub(crate) fn anonymous() -> Self {
        Self {
            kind: AuditActorKind::Anonymous,
            id: None,
            label: None,
        }
    }

    pub(crate) fn system() -> Self {
        Self {
            kind: AuditActorKind::System,
            id: None,
            label: None,
        }
    }

    pub(crate) fn session(id: impl Into<String>) -> Self {
        Self {
            kind: AuditActorKind::Session,
            id: Some(id.into()),
            label: None,
        }
    }

    pub(crate) fn token(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            kind: AuditActorKind::Token,
            id: Some(id.into()),
            label: Some(label.into()),
        }
    }
}

/// What a handler says happened.
#[derive(Clone, Debug)]
pub(crate) struct AuditEvent {
    pub action: AuditAction,
    pub outcome: AuditOutcome,
    pub actor: Actor,
    pub client_address: Option<String>,
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    pub target_name: Option<String>,
    pub trace_id: Option<String>,
    pub details: BTreeMap<String, String>,
}

impl AuditEvent {
    /// An event with nothing filled in but what happened; the builders below do the rest.
    pub(crate) fn new(action: AuditAction, outcome: AuditOutcome) -> Self {
        Self {
            action,
            outcome,
            actor: Actor::system(),
            client_address: None,
            target_kind: None,
            target_id: None,
            target_name: None,
            trace_id: None,
            details: BTreeMap::new(),
        }
    }

    pub(crate) fn success(action: AuditAction) -> Self {
        Self::new(action, AuditOutcome::Success)
    }

    pub(crate) fn failure(action: AuditAction) -> Self {
        Self::new(action, AuditOutcome::Failure)
    }

    pub(crate) fn actor(mut self, actor: Actor) -> Self {
        self.actor = actor;
        self
    }

    pub(crate) fn client(mut self, address: std::net::IpAddr) -> Self {
        self.client_address = Some(address.to_string());
        self
    }

    /// What was acted on: a family and an id inside it.
    pub(crate) fn target(mut self, kind: &str, id: impl std::fmt::Display) -> Self {
        self.target_kind = Some(kind.to_owned());
        self.target_id = Some(id.to_string());
        self
    }

    /// The name a person gave the target. User data, not a credential; redacted like the rest.
    pub(crate) fn named(mut self, name: impl Into<String>) -> Self {
        self.target_name = Some(name.into());
        self
    }

    pub(crate) fn trace(mut self, context: Option<TraceContext>) -> Self {
        self.trace_id = context.map(|context| context.trace_id_hex());
        self
    }

    /// Who acted and which trace, from the request. The shape almost every handler uses.
    pub(crate) fn by(self, context: &AuditContext) -> Self {
        self.actor(context.actor.clone()).trace(context.trace)
    }

    pub(crate) fn detail(mut self, key: &str, value: impl std::fmt::Display) -> Self {
        self.details.insert(key.to_owned(), value.to_string());
        self
    }
}

/// Everything the request already knows: who is asking and which trace it belongs to.
///
/// An extractor rather than a lookup, so a handler cannot accidentally attribute an action to
/// somebody else by asking the database again; both values were established once, by the
/// authentication middleware and by `crate::trace_context`.
#[derive(Clone, Debug)]
pub(crate) struct AuditContext {
    pub actor: Actor,
    pub trace: Option<TraceContext>,
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for AuditContext {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self {
            actor: parts
                .extensions
                .get::<Actor>()
                .cloned()
                .unwrap_or_else(Actor::anonymous),
            trace: parts.extensions.get::<TraceContext>().copied(),
        })
    }
}

tokio::task_local! {
    /// The context of the call being served, for handlers reached without extractors.
    ///
    /// The MCP surface calls the same handler functions with their extractors built by hand
    /// (`crate::mcp`), and the credential is visible one level up — in `call_tool`, which has
    /// the originating request parts — not in the tool method itself. Scoping the context
    /// around the tool call is what keeps an MCP-issued deletion attributed to the token that
    /// issued it instead of to "the service". One task, one scope, no leakage between calls.
    static CURRENT: AuditContext;
}

/// Runs `future` with `context` as the current one.
pub(crate) async fn with_context<F: std::future::Future>(
    context: AuditContext,
    future: F,
) -> F::Output {
    CURRENT.scope(context, future).await
}

impl AuditContext {
    /// The context of the call being served, or the service itself when there is none.
    pub(crate) fn current() -> Self {
        CURRENT.try_with(Clone::clone).unwrap_or_else(|_| Self {
            actor: Actor::system(),
            trace: None,
        })
    }
}

fn clean(value: &str) -> String {
    let mut value = redact_text(value);
    if value.len() > MAX_VALUE_LENGTH {
        let mut end = MAX_VALUE_LENGTH;
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
    value
}

fn clean_option(value: Option<String>) -> Option<String> {
    value.as_deref().map(clean)
}

/// One detail: a credential-named key is replaced whole, everything else goes through the
/// text rules. The same rule `rd_diagnostics::capture::redact_field` applies to a log field.
fn clean_detail(key: &str, value: &str) -> String {
    if rd_core::is_secret_parameter(key) {
        return rd_core::REDACTION_PLACEHOLDER.to_owned();
    }
    clean(value)
}

/// The record this event becomes. Split out from [`record`] so the redaction has a test that
/// needs no database.
pub(crate) fn to_record(event: AuditEvent) -> NewAuditRecord {
    let details = event
        .details
        .iter()
        .map(|(key, value)| (key.clone(), clean_detail(key, value)))
        .collect();
    NewAuditRecord {
        recorded_at: chrono::Utc::now(),
        action: event.action,
        outcome: event.outcome,
        actor_kind: event.actor.kind,
        actor_id: clean_option(event.actor.id),
        actor_label: clean_option(event.actor.label),
        client_address: clean_option(event.client_address),
        target_kind: clean_option(event.target_kind),
        target_id: clean_option(event.target_id),
        target_name: clean_option(event.target_name),
        trace_id: clean_option(event.trace_id),
        details,
    }
}

/// Writes the event, and says so in the log if it could not be written.
///
/// Returns nothing a caller has to handle. A handler whose action succeeded must not start
/// failing because the audit store is momentarily unwritable — the action already happened,
/// and reporting it as failed would be a lie in the other direction. The gap is loud instead:
/// the failure is logged at `warn` with the action that went unrecorded, which the log viewer
/// shows and the diagnostic bundle carries.
pub(crate) async fn record(state: &AppState, event: AuditEvent) {
    let action = event.action;
    let record = to_record(event);
    if let Err(error) = state.database.append_audit_record(record).await {
        tracing::warn!(
            %error,
            code = "audit.not_recorded",
            action = action.as_str(),
            "an audited action was not recorded"
        );
    }
}

#[cfg(test)]
mod tests {
    use rd_core::{AuditAction, AuditActorKind, AuditOutcome};

    use super::{Actor, AuditEvent, to_record};

    #[test]
    fn a_secret_named_detail_is_replaced_whole_and_a_url_is_redacted() {
        let event = AuditEvent::success(AuditAction::SettingsChanged)
            .actor(Actor::token("tok-1", "scraper"))
            .detail("password", "hunter2")
            .detail("api_key", "sk-live-4242")
            .detail(
                "endpoint",
                "https://cdn.example/file.bin?token=sk-live-4242",
            );
        let record = to_record(event);
        let rendered = serde_json::to_string(&record).expect("json");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(!rendered.contains("sk-live-4242"), "{rendered}");
        assert_eq!(record.actor_kind, AuditActorKind::Token);
        assert_eq!(record.actor_label.as_deref(), Some("scraper"));
    }

    #[test]
    fn a_long_name_is_truncated_on_a_character_boundary() {
        let event = AuditEvent::failure(AuditAction::PackageDeleted)
            .target("package", 7)
            // A three-byte character, so the cut lands mid-character unless it is guarded.
            .named("\u{4e2d}".repeat(2000));
        let record = to_record(event);
        let name = record.target_name.expect("a name");
        assert!(name.len() <= super::MAX_VALUE_LENGTH);
        assert!(name.chars().all(|character| character == '\u{4e2d}'));
        assert_eq!(record.outcome, AuditOutcome::Failure);
        assert_eq!(record.target_id.as_deref(), Some("7"));
    }

    #[test]
    fn an_event_with_no_actor_is_the_service_itself() {
        let record = to_record(AuditEvent::success(AuditAction::SettingsReset));
        assert_eq!(record.actor_kind, AuditActorKind::System);
        assert!(record.actor_id.is_none());
    }
}
