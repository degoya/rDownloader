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
}

/// Held by an open capture event stream; dropping it ends the agent's entry once its last
/// stream has closed.
pub struct Connection {
    agents: CaptureAgents,
    id: rd_core::CaptureTokenId,
}

impl CaptureAgents {
    /// Notes an agent's open stream and the version it reported (`None`: it reported none).
    #[must_use]
    pub fn connect(&self, id: rd_core::CaptureTokenId, version: Option<String>) -> Connection {
        if let Ok(mut agents) = self.0.lock() {
            let entry = agents.entry(id).or_insert(Connected {
                streams: 0,
                version: None,
            });
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
            })
            .collect();
        versions.sort();
        versions
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
    match state
        .database
        .capture_token_identity(&crate::auth::digest_of(token))
        .await
    {
        Ok(Some((id, _, _))) => Some(state.capture_agents.connect(id, reported_version(headers))),
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
        let first = agents.connect(id, None);
        // The reconnect after a relaunch overlaps the stream it replaces.
        let second = agents.connect(id, Some("1.9.0".to_owned()));
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
        let _new = agents.connect(rd_core::CaptureTokenId::new(), Some("1.9.0".to_owned()));
        let _old = agents.connect(rd_core::CaptureTokenId::new(), None);
        let versions = agents.versions("1.9.0");
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].version, None);
        assert!(versions[0].outdated);
        assert_eq!(versions[1].version.as_deref(), Some("1.9.0"));
        assert!(!versions[1].outdated);
    }
}
