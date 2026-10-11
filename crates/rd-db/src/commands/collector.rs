//! The commands of `writer/collector.rs`.

use super::Reply;

/// The commands `Writer::handle_collector` applies.
pub(crate) enum CollectorCommand {
    /// Appends probed playlist entries to an existing LinkGrabber package.
    AddMediaCandidates {
        package_id: rd_core::CollectorPackageId,
        entries: Vec<rd_core::MediaCandidate>,
        reply: Reply<Vec<rd_core::LinkCandidate>>,
    },
    SetCandidateMediaVariant {
        id: rd_core::CandidateId,
        variant_id: String,
        reply: Reply<rd_core::LinkCandidate>,
    },
    /// Stores the fields an enricher plugin contributed to a candidate (RD-090-14).
    SetCandidateEnrichment {
        id: rd_core::CandidateId,
        fields: Vec<rd_core::EnrichmentField>,
        reply: Reply<()>,
    },
    SetCandidateMediaInventory {
        id: rd_core::CandidateId,
        state: Box<rd_core::MediaCandidateState>,
        reply: Reply<()>,
    },
    /// Stores a resolved format selection (RD-080-01). Boxed because the update carries a
    /// whole variant and its criteria, which would otherwise widen every command.
    SetCandidateMediaSelection {
        id: rd_core::CandidateId,
        update: Box<rd_core::MediaSelectionUpdate>,
        reply: Reply<rd_core::LinkCandidate>,
    },
    /// Re-routes a candidate to another provider after the check learned what it is
    /// (RD-080-06).
    SetCandidateProvider {
        id: rd_core::CandidateId,
        provider: String,
        reply: Reply<rd_core::LinkCandidate>,
    },
    /// Sets the cookie/authentication profile a candidate is queued with (RD-080-04).
    SetCandidateAuthProfile {
        id: rd_core::CandidateId,
        selection: rd_core::AuthProfileSelection,
        reply: Reply<rd_core::LinkCandidate>,
    },
    AddCollectorBatch {
        intake: crate::collector_store::NewCollectorBatch,
        /// `vault://` reference per link, parallel to `intake.urls` (RD-110-38). Filled by
        /// `Database::add_collector_batch`, which is the only place that holds the vault.
        secret_fragment_refs: Vec<Option<String>>,
        reply: Reply<(
            rd_core::CollectorBatch,
            Vec<rd_core::CollectorPackage>,
            Vec<rd_core::LinkCandidate>,
            crate::collector_store::BatchPasswords,
        )>,
    },
    UpdateCollectorPackages {
        ids: Vec<rd_core::CollectorPackageId>,
        change: crate::collector_packages::CollectorPackageChange,
        reply: Reply<Vec<rd_core::CollectorPackage>>,
    },
    ReorderCollectorPackages {
        ids: Vec<rd_core::CollectorPackageId>,
        reply: Reply<()>,
    },
    ReorderGrabberEntries {
        entries: Vec<rd_core::GrabberEntryRef>,
        after: Option<rd_core::GrabberEntryRef>,
        reply: Reply<()>,
    },
    ReorderCandidates {
        package_id: rd_core::CollectorPackageId,
        ids: Vec<rd_core::CandidateId>,
        reply: Reply<()>,
    },
    MoveCandidates {
        ids: Vec<rd_core::CandidateId>,
        target: crate::collector_packages::MoveTarget,
        reply: Reply<rd_core::CollectorPackage>,
    },
    DeleteCollectorPackage {
        id: rd_core::CollectorPackageId,
        reply: Reply<()>,
    },
    RegroupBatches {
        batch_ids: Vec<rd_core::BatchId>,
        reply: Reply<()>,
    },
    /// Stores the standing mirror preference and re-chooses every group under it (RD-110-19).
    SetMirrorPreference {
        preference: rd_core::MirrorPreference,
        reply: Reply<()>,
    },
    /// Pins one candidate as its group's chosen mirror, or releases that pin.
    ///
    /// `false` means the link is in no mirror group, which the REST layer refuses.
    SetMirrorPin {
        id: rd_core::CandidateId,
        pinned: bool,
        reply: Reply<bool>,
    },
    /// Takes a proposed mirror group apart and records that its links differ (RD-110-34).
    DissolveMirrorGroup {
        id: rd_core::CandidateId,
        reply: Reply<crate::MirrorDissolve>,
    },
    ClaimCandidatesForCheck {
        ids: Vec<rd_core::CandidateId>,
        reply: Reply<Vec<rd_core::LinkCandidate>>,
    },
    RecordCandidateCheck {
        id: rd_core::CandidateId,
        result: Option<rd_core::LinkCheckResult>,
        error: Option<rd_core::CandidateMessage>,
        /// Restores the duplicate state after the check so the warning survives.
        was_duplicate: bool,
        /// The provider whose cache answered; kept only with a `cached` result (RD-130-11).
        cached_by: Option<String>,
        reply: Reply<()>,
    },
    MarkCandidateUnsupported {
        id: rd_core::CandidateId,
        message: rd_core::CandidateMessage,
        /// The provider whose cache holds the file although no check source exists.
        cached_by: Option<String>,
        reply: Reply<()>,
    },
    SetCandidateFileName {
        id: rd_core::CandidateId,
        file_name: String,
        reply: Reply<rd_core::LinkCandidate>,
    },
    ClaimPackageForEnqueue {
        id: rd_core::CollectorPackageId,
        only: Option<Vec<rd_core::CandidateId>>,
        reply: Reply<Vec<(rd_core::LinkCandidate, rd_core::LinkCandidateState)>>,
    },
    FinishPackageEnqueue {
        id: rd_core::CollectorPackageId,
        success: bool,
        restore: Vec<(rd_core::CandidateId, rd_core::LinkCandidateState)>,
        reply: Reply<()>,
    },
    DeleteCandidate {
        id: rd_core::CandidateId,
        reply: Reply<()>,
    },
    DeleteCandidates {
        reply: Reply<u64>,
    },
    /// Decides every open link anew by the LinkFilter rules (RD-1240-09).
    ApplyLinkFilters {
        reply: Reply<crate::LinkFilterOutcome>,
    },
    /// Shows links a LinkFilter rule hid; answers how many were hidden.
    ShowFilteredCandidates {
        ids: Vec<rd_core::CandidateId>,
        reply: Reply<u64>,
    },
}
