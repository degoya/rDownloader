// The macro and the three identifiers a plugin names (`AccountId`, `PluginId`,
// `ProxyProfileId`) live in `rd-plugin-types` (RD-1190-08).
use rd_plugin_types::domain_id;

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
domain_id!(IndexerId);
domain_id!(NotificationDeliveryId);
domain_id!(NotificationRuleId);
domain_id!(NotificationTargetId);
domain_id!(MfaCredentialId);
domain_id!(NzbFileId);
domain_id!(NzbImportId);
domain_id!(NzbSegmentId);
domain_id!(ObjectStorageProfileId);
domain_id!(PackageId);
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
