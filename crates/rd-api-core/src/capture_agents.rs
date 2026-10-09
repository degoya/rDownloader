//! The capture agents connected right now and the version each one runs (RD-190-07).
//!
//! An agent that finds its program file replaced by an update restarts itself as the new one
//! (`rd_capture::relaunch`, since 1.8.1). Where that cannot happen -- an agent from before that
//! rule, an agent another user started, one whose program file this update did not replace --
//! the update view says so, with both versions. For that the agent names its version in the
//! `User-Agent` of every request (`rd_core::CAPTURE_AGENT_PRODUCT`), and the service keeps it
//! for as long as the agent holds its event stream open: an open stream is what "the agent
//! runs" means here, so a closed one takes its entry with it and no hint outlives its agent.
//!
//! Purely in memory, like the browser-session handovers: after a restart of the service every
//! agent reconnects within seconds and reports again. Keyed by the agent's capture token, so a
//! reconnect that overlaps the stream it replaces counts once, with the version it came back
//! as. No label leaves this module: the status it feeds is readable with `api:read`, and the
//! list of paired agents needs `api:secrets`.
//!
//! An agent installed without the service updates itself (RD-1210-03) and says where that stands
//! on its settings poll (`rd_update::agent::report`): [`CaptureAgents::note_report`] keeps the last
//! report beside the version, found by the digest of the token the stream opened with, so the poll
//! costs no lookup.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::http::{HeaderMap, header};

use crate::{AppState, dto::CaptureAgentVersion};

/// The longest version string kept; anything longer is not a version this project issues.
const MAX_VERSION_LENGTH: usize = 64;

/// The connected agents, by capture token.
#[derive(Clone, Default)]
pub struct CaptureAgents(Arc<Mutex<HashMap<rd_core::CaptureTokenId, Connected>>>);

/// One agent's open streams and the version the newest of them reported.
struct Connected {
    streams: usize,
    version: Option<String>,
    /// The digest of its capture token, which its settings poll is matched by.
    digest: String,
    /// What it last said about its own update (RD-1210-03); `None` before 1.21.
    report: Option<rd_update::agent::report::AgentReport>,
}

/// Held by an open capture event stream; dropping it ends the agent's entry once its last
/// stream has closed.
pub struct Connection {
    agents: CaptureAgents,
    id: rd_core::CaptureTokenId,
}

impl CaptureAgents {
    /// Notes an agent's open stream, the version it reported (`None`: it reported none) and the
    /// digest of its token.
    #[must_use]
    pub fn connect(
        &self,
        id: rd_core::CaptureTokenId,
        version: Option<String>,
        digest: String,
    ) -> Connection {
        if let Ok(mut agents) = self.0.lock() {
            let entry = agents.entry(id).or_insert(Connected {
                streams: 0,
                version: None,
                digest: String::new(),
                report: None,
            });
            entry.digest = digest;
            entry.streams += 1;
            entry.version = version;
        }
        Connection {
            agents: self.clone(),
            id,
        }
    }

    /// The connected agents as the update status shows them, measured against `service`, the
    /// version of this service. Sorted, so the answer does not change order between two reads.
    #[must_use]
    pub fn versions(&self, service: &str) -> Vec<CaptureAgentVersion> {
        let Ok(agents) = self.0.lock() else {
            return Vec::new();
        };
        let mut versions: Vec<CaptureAgentVersion> = agents
            .values()
            .map(|agent| CaptureAgentVersion {
                outdated: outdated(agent.version.as_deref(), service),
                version: agent.version.clone(),
                self_update: agent
                    .report
                    .as_ref()
                    .map(|report| report.state.as_str().to_owned()),
                offered_version: agent
                    .report
                    .as_ref()
                    .and_then(|report| report.offered.clone()),
                remote_update_allowed: agent.report.as_ref().map(|report| report.remote_allowed),
            })
            .collect();
        versions.sort();
        versions
    }

    /// Keeps the update report the settings poll in `headers` carries (RD-1210-03), for the
    /// connected agent whose token it bears. A poll without a report, from a token with no open
    /// stream, or with a report this build cannot read changes nothing.
    pub fn note_report(&self, headers: &HeaderMap) {
        let Some(report) = headers
            .get(rd_update::agent::report::REPORT_HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(rd_update::agent::report::AgentReport::parse)
        else {
            return;
        };
        let Some(token) = crate::auth::bearer_token(headers) else {
            return;
        };
        let digest = crate::auth::digest_of(token);
        let Ok(mut agents) = self.0.lock() else {
            return;
        };
        if let Some(agent) = agents.values_mut().find(|agent| agent.digest == digest) {
            agent.report = Some(report);
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let Ok(mut agents) = self.agents.0.lock() else {
            return;
        };
        if let Some(agent) = agents.get_mut(&self.id) {
            agent.streams = agent.streams.saturating_sub(1);
            if agent.streams == 0 {
                agents.remove(&self.id);
            }
        }
    }
}

/// Whether an agent of `version` is older than a service of `service`. An agent that reports
/// no version predates the report (1.9) and is older by that alone; one whose version does not
/// parse is not called older on a guess.
#[must_use]
pub fn outdated(version: Option<&str>, service: &str) -> bool {
    version.is_none_or(|version| rd_update::is_newer(service, version))
}

/// The version a capture agent named in its `User-Agent`, or `None` when the header is missing
/// or is not the agent's (`rdownloader-capture/<version>`).
#[must_use]
pub fn reported_version(headers: &HeaderMap) -> Option<String> {
    let agent = headers.get(header::USER_AGENT)?.to_str().ok()?;
    let version = agent
        .strip_prefix(rd_core::CAPTURE_AGENT_PRODUCT)?
        .strip_prefix('/')?
        .split_whitespace()
        .next()?;
    let plausible = version.len() <= MAX_VERSION_LENGTH
        && version
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-+".contains(character));
    plausible.then(|| version.to_owned())
}

/// Registers the agent behind this request's capture token, for as long as the returned
/// connection is held. `None` when the token cannot be looked up -- the stream still opens, the
/// update view just does not count it.
pub async fn connect(state: &AppState, headers: &HeaderMap) -> Option<Connection> {
    let token = crate::auth::bearer_token(headers)?;
    let digest = crate::auth::digest_of(token);
    match state.database.capture_token_identity(&digest).await {
        Ok(Some((id, _, _, _))) => Some(state.capture_agents.connect(
            id,
            reported_version(headers),
            digest,
        )),
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(%error, "could not identify the capture agent behind a stream");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};

    use super::{CaptureAgents, outdated, reported_version};

    fn agent(value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::USER_AGENT, HeaderValue::from_static(value));
        headers
    }

    #[test]
    fn the_version_is_read_from_the_agents_user_agent_only() {
        assert_eq!(
            reported_version(&agent("rdownloader-capture/1.9.0")).as_deref(),
            Some("1.9.0")
        );
        assert_eq!(
            reported_version(&agent("rdownloader-capture/1.9.0-beta.2 (Windows)")).as_deref(),
            Some("1.9.0-beta.2")
        );
        assert_eq!(reported_version(&HeaderMap::new()), None);
        assert_eq!(reported_version(&agent("Mozilla/5.0")), None);
        assert_eq!(reported_version(&agent("rdownloader-capture")), None);
        assert_eq!(reported_version(&agent("rdownloader-capture/")), None);
        assert_eq!(reported_version(&agent("rdownloader-capturex/1.9.0")), None);
        assert_eq!(
            reported_version(&agent("rdownloader-capture/1.9.0<b>")),
            None
        );
        let long = format!("rdownloader-capture/{}", "9".repeat(65));
        let mut headers = HeaderMap::new();
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_str(&long).expect("header"),
        );
        assert_eq!(reported_version(&headers), None);
    }

    #[test]
    fn an_agent_without_a_version_or_with_an_older_one_is_outdated() {
        assert!(outdated(None, "1.9.0"), "a pre-1.9 agent reports nothing");
        assert!(outdated(Some("1.9.0"), "1.9.1"));
        assert!(outdated(Some("1.9.0-beta.2"), "1.9.0"));
        assert!(!outdated(Some("1.9.0"), "1.9.0"));
        // After a roll-back the agent may be the newer one; it restarts itself (relaunch).
        assert!(!outdated(Some("1.10.0"), "1.9.0"));
        assert!(!outdated(Some("not-a-version"), "1.9.0"));
    }

    #[test]
    fn an_agent_counts_while_any_of_its_streams_is_open() {
        let agents = CaptureAgents::default();
        let id = rd_core::CaptureTokenId::new();
        let first = agents.connect(id, None, "digest".to_owned());
        // The reconnect after a relaunch overlaps the stream it replaces.
        let second = agents.connect(id, Some("1.9.0".to_owned()), "digest".to_owned());
        let versions = agents.versions("1.9.0");
        assert_eq!(versions.len(), 1, "one agent, two streams");
        assert_eq!(versions[0].version.as_deref(), Some("1.9.0"));
        assert!(!versions[0].outdated);
        drop(first);
        assert_eq!(agents.versions("1.9.0").len(), 1);
        drop(second);
        assert!(
            agents.versions("1.9.0").is_empty(),
            "no running agent, no entry and no hint"
        );
    }

    #[test]
    fn two_agents_are_listed_apart_and_in_a_stable_order() {
        let agents = CaptureAgents::default();
        let _new = agents.connect(
            rd_core::CaptureTokenId::new(),
            Some("1.9.0".to_owned()),
            "new".to_owned(),
        );
        let _old = agents.connect(rd_core::CaptureTokenId::new(), None, "old".to_owned());
        let versions = agents.versions("1.9.0");
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].version, None);
        assert!(versions[0].outdated);
        assert_eq!(versions[1].version.as_deref(), Some("1.9.0"));
        assert!(!versions[1].outdated);
    }

    /// RD-1210-03: the settings poll's report reaches the agent whose token it bears, and only
    /// that one; an agent that sends none shows none.
    #[test]
    fn an_agents_update_report_is_kept_beside_its_version() {
        let agents = CaptureAgents::default();
        let token = "a-capture-token-of-forty-characters-0001";
        let _reporting = agents.connect(
            rd_core::CaptureTokenId::new(),
            Some("1.20.0".to_owned()),
            crate::auth::digest_of(token),
        );
        let _quiet = agents.connect(
            rd_core::CaptureTokenId::new(),
            Some("1.20.0".to_owned()),
            crate::auth::digest_of("another-token-of-forty-characters-00002"),
        );
        let mut poll = HeaderMap::new();
        poll.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).expect("header"),
        );
        agents.note_report(&poll);
        assert!(
            agents
                .versions("1.20.0")
                .iter()
                .all(|agent| agent.self_update.is_none()),
            "a poll without a report changes nothing"
        );
        poll.insert(
            rd_update::agent::report::REPORT_HEADER,
            HeaderValue::from_static("state=offered; version=1.21.0; remote=0"),
        );
        agents.note_report(&poll);
        let versions = agents.versions("1.20.0");
        let reporting: Vec<_> = versions
            .iter()
            .filter(|agent| agent.self_update.is_some())
            .collect();
        assert_eq!(reporting.len(), 1, "only the agent behind the token");
        assert_eq!(reporting[0].self_update.as_deref(), Some("offered"));
        assert_eq!(reporting[0].offered_version.as_deref(), Some("1.21.0"));
        assert_eq!(reporting[0].remote_update_allowed, Some(false));
    }
}
