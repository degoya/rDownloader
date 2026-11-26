//! What a notification is about, for the program that receives it (RD-1480-11).
//!
//! A webhook's body had nothing but words: a receiver could neither tell when it happened nor
//! ask the API about the package. The fields are the same on every body, `null` where an event
//! has none, and are only ever added to.

use rd_core::{DownloadId, PackageId, Timestamp};
use serde::{Deserialize, Serialize};

/// The version of a webhook's body, its `version` field. Raised when a field goes or means
/// something else; a field that is added leaves it.
pub const PAYLOAD_VERSION: u32 = 1;

/// When it happened and to what. Kept with the delivery, so a retry says the same.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Subject {
    /// Unknown only for a delivery queued before 1.48, which says when it was queued instead.
    pub occurred_at: Option<Timestamp>,
    pub package_id: Option<PackageId>,
    pub download_id: Option<DownloadId>,
}

impl Subject {
    /// Something that happens now and is about no package.
    #[must_use]
    pub fn now() -> Self {
        Self {
            occurred_at: Some(Timestamp::now()),
            ..Self::default()
        }
    }

    /// Writes `version`, `occurred_at`, `package_id` and `download_id` into a webhook's body.
    pub fn describe(&self, body: &mut serde_json::Value) {
        body["version"] = PAYLOAD_VERSION.into();
        body["occurred_at"] = serde_json::json!(self.occurred_at);
        body["package_id"] = serde_json::json!(self.package_id);
        body["download_id"] = serde_json::json!(self.download_id);
    }
}
