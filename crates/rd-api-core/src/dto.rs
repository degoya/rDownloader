use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

mod access;
mod accounts;
mod categories;
mod collector;
mod credentials;
mod media;
mod package_export;
mod plugins;
mod queue;
mod restart;
mod settings;
mod settings_validation;
mod storage;
mod update;

pub use access::*;
pub use accounts::*;
pub use categories::*;
pub use collector::*;
pub use credentials::*;
pub use media::*;
pub use package_export::*;
pub use plugins::*;
pub use queue::*;
pub use restart::*;
pub use settings::*;
pub use settings_validation::*;
pub use storage::*;
pub use update::*;

/// Action result: English text plus a stable code (and parameters) clients translate.
#[derive(Serialize, ToSchema)]
pub struct MessageResponse {
    pub message: String,
    pub code: String,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub params: crate::error::MessageParams,
}

impl MessageResponse {
    /// Creates a coded action result.
    #[must_use]
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: code.to_owned(),
            params: Default::default(),
        }
    }

    /// Attaches a translation parameter.
    #[must_use]
    pub fn with_param(mut self, key: &str, value: impl ToString) -> Self {
        self.params.insert(key.to_owned(), value.to_string());
        self
    }

    /// Attaches the `count` parameter used by pluralised messages.
    #[must_use]
    pub fn with_count(self, count: impl ToString) -> Self {
        self.with_param("count", count)
    }
}

fn default_channel_enabled() -> bool {
    true
}

const fn default_true() -> bool {
    true
}
