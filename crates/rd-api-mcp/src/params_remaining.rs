//! Parameters for the tools RD-120-55 added: the thirteen capabilities RD-120-32 left
//! unclassified, checked against the owner's line and taken in where none of its four marks
//! applies.
//!
//! Same arrangement as [`super::params_handling`]: string-typed mirrors of the REST query
//! strings and bodies, turned into the REST type by [`super::params_handling::body`] so that the
//! REST type itself deserialises them and the handler answers with its own codes. [`object`]
//! leaves an absent argument out rather than sending `null`, because several of the query types
//! default a missing field and would refuse a `null` one.

use rmcp::schemars;
use serde::Deserialize;

/// A JSON object from the arguments that were given; an absent one is left out, not `null`.
pub(crate) fn object(pairs: &[(&str, serde_json::Value)]) -> serde_json::Value {
    serde_json::Value::Object(
        pairs
            .iter()
            .filter(|(_, value)| !value.is_null())
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect(),
    )
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct PluginMessagesParams {
    /// A two-letter language tag: `de`, `en`, `es` or `fr`.
    pub locale: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct AutomationRunsParams {
    /// Only the runs of this automation (id from list_automations).
    #[serde(default)]
    pub automation_id: Option<String>,
    /// Newest runs to return (1-500).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct DryRunParams {
    /// The trigger to simulate, as get_automation_vocabulary lists it, e.g. `package_completed`.
    pub trigger: String,
    /// A package (id from list_packages) to judge the conditions against. Absent evaluates
    /// against nothing, which only a condition-free automation matches.
    #[serde(default)]
    pub package_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct DeliveriesParams {
    /// Newest deliveries to return (1-500).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct SubscriptionItemsParams {
    /// The subscription, as list_subscriptions reports it.
    pub id: String,
    /// `pending` (default), `queued`, `dismissed`, `skipped` or `all`.
    #[serde(default)]
    pub state: Option<String>,
    /// Rows per page.
    #[serde(default)]
    pub limit: Option<i64>,
    /// Rows to skip, for the next page.
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct SubscriptionRunsParams {
    /// The subscription, as list_subscriptions reports it.
    pub id: String,
    /// Newest runs to return.
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct SubscriptionSwitchParams {
    /// The subscription, as list_subscriptions reports it.
    pub id: String,
    pub enabled: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ReviewParams {
    /// For review_subscription_item the item id from list_subscription_items; for
    /// review_pending_subscription_items the subscription id from list_subscriptions.
    pub id: String,
    /// `queued` hands the hit to the LinkGrabber; `dismissed` sets it aside.
    pub state: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct StreamRunsParams {
    /// Only the occurrences of this schedule (id from list_stream_schedules).
    #[serde(default)]
    pub schedule_id: Option<String>,
    /// Newest occurrences to return (1-200).
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct RecordNowParams {
    /// The livestream's address.
    pub url: String,
    /// The package name; the address's own when absent.
    #[serde(default)]
    pub name: Option<String>,
    /// A quality selector such as `best` or `720p`.
    #[serde(default)]
    pub quality: Option<String>,
    /// The category (id from list_configuration section categories).
    #[serde(default)]
    pub category_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct TestRegexParams {
    /// The regular expression, as a routing rule would carry it.
    pub pattern: String,
    /// Up to 50 sample texts, each at most 512 bytes, to match it against.
    pub samples: Vec<String>,
}
