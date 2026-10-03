//! Database facade for the LinkGrabber's mirror groups (RD-110-18, RD-110-19): the standing
//! preference, a pinned choice and a refused group. Split out of `facade_collector.rs` to keep
//! it inside the size budget.

use anyhow::Result;
use rd_core::{CandidateId, MirrorPreference};

use crate::{Database, commands::WriterCommand, writer};

impl Database {
    /// The standing mirror preference, or its defaults when none was ever stored (RD-110-19).
    pub async fn mirror_preference(&self) -> Result<MirrorPreference> {
        Ok(self
            .get_setting(crate::MIRROR_PREFERENCE_KEY)
            .await?
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default())
    }

    /// Stores the standing mirror preference and re-chooses every group under it.
    pub async fn set_mirror_preference(&self, preference: MirrorPreference) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::SetMirrorPreference {
            preference,
            reply,
        })
        .await
    }

    /// Makes one candidate its group's chosen mirror, or releases that choice.
    ///
    /// `false` means the link belongs to no mirror group, so there was nothing to choose
    /// between.
    pub async fn set_mirror_pin(&self, id: CandidateId, pinned: bool) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::SetMirrorPin {
            id,
            pinned,
            reply,
        })
        .await
    }

    /// Takes a proposed mirror group apart, so its links are single candidates again.
    ///
    /// The refusal is stored as pairs of links, not as an absent group, so it survives the
    /// recompute at intake, after the online check and on a move between packages.
    pub async fn dissolve_mirror_group(&self, id: CandidateId) -> Result<crate::MirrorDissolve> {
        writer::request(&self.writer, |reply| WriterCommand::DissolveMirrorGroup {
            id,
            reply,
        })
        .await
    }
}
