//! Parameters for the tools RD-120-29 added: remote jobs, and what the installation can tell
//! you about itself.
//!
//! Mirrors of the REST query strings and bodies rather than reuses of them, for the reason
//! `super::params_config` gives at length: the REST types derive `utoipa::ToSchema` and a
//! tool needs `schemars::JsonSchema`. The conversion is mechanical and the REST handler still
//! does the validating, so there is no second set of error codes.

use rmcp::schemars;
use serde::Deserialize;

/// How far back `GET /api/v1/stats/transfers` reaches.
#[derive(Clone, Copy, Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StatsRangeParam {
    #[default]
    Day,
    Week,
    Month,
    Year,
}

impl From<StatsRangeParam> for crate::stats_handlers::StatsRange {
    fn from(value: StatsRangeParam) -> Self {
        match value {
            StatsRangeParam::Day => Self::Day,
            StatsRangeParam::Week => Self::Week,
            StatsRangeParam::Month => Self::Month,
            StatsRangeParam::Year => Self::Year,
        }
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct TransferStatsParams {
    /// `day` (hourly buckets), `week`, `month` or `year` (daily buckets); `day` by default.
    pub range: Option<StatsRangeParam>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct LogQueryToolParams {
    /// This level and the more severe ones: `trace`, `debug`, `info`, `warn` or `error`.
    pub level: Option<String>,
    /// A component prefix, such as `rd_http`.
    pub component: Option<String>,
    /// A stable code, exactly.
    pub code: Option<String>,
    /// A correlation id, exactly; the thread tying one piece of work together.
    pub correlation_id: Option<String>,
    /// A case-insensitive substring of the message.
    pub search: Option<String>,
    /// RFC 3339; records at or after this moment.
    pub since: Option<String>,
    /// RFC 3339; records at or before this moment.
    pub until: Option<String>,
    /// Records older than this id, for paging backwards through a full page.
    pub before_id: Option<i64>,
    /// Newest rows to return (1-500).
    pub limit: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct AuditQueryToolParams {
    /// One action word, such as `login_failed`. The answer lists every action it knows.
    pub action: Option<String>,
    /// `success` or `failure`.
    pub outcome: Option<String>,
    /// `session`, `token`, `anonymous` or `system`.
    pub actor_kind: Option<String>,
    /// An actor id, exactly.
    pub actor_id: Option<String>,
    /// A target family, such as `download`.
    pub target_kind: Option<String>,
    /// A target id, exactly.
    pub target_id: Option<String>,
    /// A trace id, exactly; the same value list_log_records filters on.
    pub trace_id: Option<String>,
    /// RFC 3339; records at or after this moment.
    pub since: Option<String>,
    /// RFC 3339; records at or before this moment.
    pub until: Option<String>,
    /// Records older than this id, for paging backwards through a full page.
    pub before_id: Option<i64>,
    /// Newest rows to return (1-500).
    pub limit: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct SiteRuleSwitchParams {
    /// The rule's own identifier, as list_site_rules reports it.
    pub id: String,
    pub enabled: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct SiteRuleGroupSwitchParams {
    /// The group name, as the rules carry it.
    pub group: String,
    pub enabled: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct SubmitRemoteJobParams {
    /// The account whose provider is to run the job.
    pub account_id: String,
    /// The `magnet:` address to hand over. Give exactly one of `magnet`, `address` and
    /// `container`.
    #[serde(default)]
    pub magnet: Option<String>,
    /// A plain `http(s)` address the provider fetches for itself. The route refuses a call
    /// that names none or more than one source, so the tool does not have to decide which one
    /// wins -- it forwards what it was given (RD-120-20).
    #[serde(default)]
    pub address: Option<String>,
    /// A `.torrent` or `.nzb` file as base64 (standard alphabet), at most 16 MiB decoded. The
    /// provider's plugin reads the format from the bytes (RD-120-31).
    #[serde(default)]
    pub container: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct RemoteJobChoiceParams {
    /// The job that is waiting in `awaiting_choice`.
    pub id: String,
    /// Entry ids the job offered, as its `choice` field lists them. Anything else is dropped
    /// rather than forwarded.
    pub entries: Vec<u32>,
}

/// The confirmation a clear carries (RD-120-34).
///
/// Spelled out as an argument rather than implied by calling the tool: the toolbox leaves out
/// capabilities that destroy something without a confirmation, and an argument the caller had
/// to set is what makes this one different from the ones it leaves out. The REST handler
/// refuses the request without it, so a tool that forgot it deletes nothing.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct DataClearToolParams {
    /// Must be `true`. Anything else is refused with `data_reset.not_confirmed`.
    pub confirmed: bool,
}
