//! Which door an audited action came through (RD-1200-04).
//!
//! The actor says *who* acted — a session, a token — and a token is the same token whether it
//! called a REST route or an MCP tool. The channel says *how*: an agent's tool call and a
//! dashboard's request with the one token are told apart only by it (`docs/security/mcp.md`,
//! finding 6).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// How an action reached the service. A closed set with a stable word each, so a filter can be
/// written against it and the store can keep it as text.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AuditChannel {
    /// The REST API: the web interface, a script, a launcher on this machine. What a record
    /// written before the channel existed reads as.
    #[default]
    Rest,
    /// A tool call on the MCP endpoint `/mcp`.
    Mcp,
    /// The capture door `/api/v1/capture/*`: the browser extension and the desktop agent.
    Capture,
    /// The SABnzbd and qBittorrent adapters.
    Compat,
    /// No request at all: the service's own work, or a command that wrote a stopped service's
    /// database itself.
    Internal,
}

impl AuditChannel {
    pub const ALL: [Self; 5] = [
        Self::Rest,
        Self::Mcp,
        Self::Capture,
        Self::Compat,
        Self::Internal,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rest => "rest",
            Self::Mcp => "mcp",
            Self::Capture => "capture",
            Self::Compat => "compat",
            Self::Internal => "internal",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL
            .into_iter()
            .find(|channel| channel.as_str() == value)
    }
}

#[cfg(test)]
mod tests {
    use super::AuditChannel;

    #[test]
    fn every_channel_parses_its_own_word_and_nothing_else() {
        let mut seen = std::collections::BTreeSet::new();
        for channel in AuditChannel::ALL {
            assert!(seen.insert(channel.as_str()), "duplicate {channel:?}");
            assert_eq!(AuditChannel::parse(channel.as_str()), Some(channel));
            let json = serde_json::to_value(channel).expect("json");
            assert_eq!(json, serde_json::json!(channel.as_str()));
        }
        assert_eq!(AuditChannel::parse("smtp"), None);
        assert_eq!(AuditChannel::default(), AuditChannel::Rest);
    }
}
