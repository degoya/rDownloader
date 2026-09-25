use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

macro_rules! domain_id {
    ($name:ident) => {
        #[doc = concat!("Stable UUIDv7 identifier for ", stringify!($name), ".")]
        #[derive(
            Clone,
            Copy,
            Debug,
            Deserialize,
            Eq,
            Hash,
            Ord,
            PartialEq,
            PartialOrd,
            Serialize,
            ToSchema,
        )]
        #[serde(transparent)]
        #[schema(value_type = String, format = Uuid)]
        pub struct $name(Uuid);

        impl $name {
            /// Creates a time-ordered UUIDv7 identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Wraps an existing UUID.
            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            /// Returns the underlying UUID.
            #[must_use]
            pub const fn into_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value).map(Self)
            }
        }
    };
}

domain_id!(AccountId);
domain_id!(AutomationId);
domain_id!(AutomationRunId);
domain_id!(AutomationVersionId);
domain_id!(BandwidthProfileId);
domain_id!(AuthProfileId);
domain_id!(BandwidthWindowId);
domain_id!(BatchId);
domain_id!(CandidateId);
domain_id!(CaptchaId);
domain_id!(CaptureAgentId);
domain_id!(CaptureTokenId);
domain_id!(CategoryId);
domain_id!(CategoryRuleId);
domain_id!(ChunkId);
domain_id!(CollectorPackageId);
domain_id!(DownloadId);
domain_id!(EventId);
domain_id!(HotFolderId);
domain_id!(NotificationDeliveryId);
domain_id!(NotificationRuleId);
domain_id!(NotificationTargetId);
domain_id!(MfaCredentialId);
domain_id!(NzbFileId);
domain_id!(NzbImportId);
domain_id!(NzbSegmentId);
domain_id!(PackageId);
domain_id!(PluginId);
domain_id!(ProxyProfileId);
domain_id!(RemoteCredentialId);
domain_id!(RemoteJobId);
domain_id!(SessionId);
domain_id!(StorageRootId);
domain_id!(StreamChannelId);
domain_id!(StreamScheduleId);
domain_id!(StreamScheduledRunId);
domain_id!(SubscriptionId);
domain_id!(SubscriptionItemId);
domain_id!(SubscriptionRunId);
domain_id!(UsenetServerId);

#[cfg(test)]
mod tests {
    use super::DownloadId;

    #[test]
    fn uuid_v7_round_trip() {
        let id = DownloadId::new();
        let parsed = id.to_string().parse::<DownloadId>();
        assert_eq!(parsed.ok(), Some(id));
        assert_eq!(id.into_uuid().get_version_num(), 7);
    }
}
