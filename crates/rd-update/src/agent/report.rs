//! What the agent tells the service about its own update, and what the service tells the agent
//! (RD-1210-03).
//!
//! One header each way on the agent's settings poll (`GET /api/v1/capture/agent-settings`), which
//! the agent sends every five seconds anyway: [`REPORT_HEADER`] on the request says where the
//! agent's update stands, [`CHANNEL_HEADER`] on the answer names the service's update channel.
//! Additive in both directions — a service from before ignores the request header, an agent from
//! before neither sends it nor reads the answer's.
//!
//! [`REQUEST_HEADER`] is the service asking the agent to install a version. Nothing sends it yet;
//! the agent refuses it unless its own configuration allows it (`allow_remote`, off by default),
//! so no service — and no MCP tool behind one — installs software on another machine without
//! that machine's consent.
//!
//! The report is plain `key=value` pairs separated by `;`, at most [`MAX_REPORT_BYTES`]: a service
//! keeps it in memory for as long as the agent is connected, and shows it with `api:read`.

use crate::{install::is_plain_version, offer::parse_version};

/// The request header the agent reports in.
pub const REPORT_HEADER: &str = "x-rdownloader-agent-update";
/// The answer header the service names its update channel in: `stable` or `beta`.
pub const CHANNEL_HEADER: &str = "x-rdownloader-update-channel";
/// The answer header the service asks the agent to install a version in.
pub const REQUEST_HEADER: &str = "x-rdownloader-agent-update-request";
/// The longest report read; anything longer is not one this project's agent sends.
pub const MAX_REPORT_BYTES: usize = 160;

/// Where the agent's own update stands.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SelfUpdate {
    /// The service sits in the agent's folder and updates both.
    WithService,
    /// The agent's own check is switched off in its configuration.
    Disabled,
    /// No check has answered yet, or this build carries no update key.
    Unchecked,
    /// The last check found nothing newer.
    Current,
    /// The last check found a newer version.
    Offered,
    /// The last check failed or refused the manifest.
    Failed,
    /// The agent is installing the offered version.
    Installing,
}

impl SelfUpdate {
    const ALL: [Self; 7] = [
        Self::WithService,
        Self::Disabled,
        Self::Unchecked,
        Self::Current,
        Self::Offered,
        Self::Failed,
        Self::Installing,
    ];

    /// The stable name in the header and the API.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WithService => "with_service",
            Self::Disabled => "disabled",
            Self::Unchecked => "unchecked",
            Self::Current => "current",
            Self::Offered => "offered",
            Self::Failed => "failed",
            Self::Installing => "installing",
        }
    }

    /// The state `value` names; anything else is `None`.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == value)
    }
}

/// One report, as the agent sends it and the service keeps it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentReport {
    pub state: SelfUpdate,
    /// The version the agent offers itself, with [`SelfUpdate::Offered`] and
    /// [`SelfUpdate::Installing`].
    pub offered: Option<String>,
    /// Whether the agent lets the service ask it to install an update.
    pub remote_allowed: bool,
}

impl AgentReport {
    /// The header value: `state=offered; version=1.21.0; remote=0`.
    #[must_use]
    pub fn to_header(&self) -> String {
        let mut text = format!("state={}", self.state.as_str());
        if let Some(version) = &self.offered {
            text.push_str("; version=");
            text.push_str(version);
        }
        text.push_str(if self.remote_allowed {
            "; remote=1"
        } else {
            "; remote=0"
        });
        text
    }

    /// The report a header carries; `None` for anything this build does not read as one. Keys
    /// it does not know are skipped, so a later agent may add one.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() > MAX_REPORT_BYTES {
            return None;
        }
        let (mut state, mut offered, mut remote_allowed) = (None, None, false);
        for pair in text.split(';') {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            match (key.trim(), value.trim()) {
                ("state", value) => state = SelfUpdate::parse(value),
                ("version", value) if is_plain_version(value) && parse_version(value).is_some() => {
                    offered = Some(value.to_owned());
                }
                ("version", _) => return None,
                ("remote", value) => remote_allowed = value == "1",
                _ => {}
            }
        }
        Some(Self {
            state: state?,
            offered,
            remote_allowed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentReport, MAX_REPORT_BYTES, SelfUpdate};

    #[test]
    fn a_report_reads_back_as_it_was_sent() {
        let report = AgentReport {
            state: SelfUpdate::Offered,
            offered: Some("1.21.0".to_owned()),
            remote_allowed: false,
        };
        assert_eq!(
            report.to_header(),
            "state=offered; version=1.21.0; remote=0"
        );
        assert_eq!(AgentReport::parse(&report.to_header()), Some(report));
        let beside = AgentReport {
            state: SelfUpdate::WithService,
            offered: None,
            remote_allowed: true,
        };
        assert_eq!(AgentReport::parse(&beside.to_header()), Some(beside));
    }

    /// The service shows what it keeps; a report it cannot read is none rather than a guess.
    #[test]
    fn a_report_the_service_cannot_trust_is_dropped() {
        assert_eq!(AgentReport::parse("state=sideways"), None);
        assert_eq!(AgentReport::parse("version=1.21.0"), None, "no state");
        assert_eq!(
            AgentReport::parse("state=offered; version=1.21.0<script>"),
            None
        );
        assert_eq!(AgentReport::parse("state=offered; version=latest"), None);
        let long = format!("state=current; note={}", "x".repeat(MAX_REPORT_BYTES));
        assert_eq!(AgentReport::parse(&long), None);
        // A key a later agent adds is skipped.
        assert_eq!(
            AgentReport::parse("state=current; colour=blue").map(|report| report.state),
            Some(SelfUpdate::Current)
        );
    }
}
