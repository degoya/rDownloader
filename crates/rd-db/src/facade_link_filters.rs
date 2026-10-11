//! Database facade for the LinkFilter rules and their application (RD-1240-09).

use anyhow::Result;

use crate::{
    Database, LinkFilterOutcome, NewLinkFilterRule,
    commands::{CollectorCommand, ConfigCommand},
    link_filter_store, writer,
};

impl Database {
    /// The LinkFilter rules in evaluation order.
    pub async fn list_link_filter_rules(&self) -> Result<Vec<rd_core::LinkFilterRule>> {
        link_filter_store::list_link_filter_rules(&self.readers).await
    }

    /// Adds a rule at the end of the evaluation order.
    pub async fn create_link_filter_rule(
        &self,
        input: NewLinkFilterRule,
    ) -> Result<rd_core::LinkFilterRule> {
        writer::request(&self.writer, |reply| ConfigCommand::CreateLinkFilterRule {
            input,
            reply,
        })
        .await
    }

    /// Replaces every editable field of a rule; its place in the order stays.
    pub async fn update_link_filter_rule(
        &self,
        id: rd_core::LinkFilterRuleId,
        input: NewLinkFilterRule,
    ) -> Result<rd_core::LinkFilterRule> {
        writer::request(&self.writer, |reply| ConfigCommand::UpdateLinkFilterRule {
            id,
            input,
            reply,
        })
        .await
    }

    /// Removes a rule; the links it hid are shown again.
    pub async fn delete_link_filter_rule(&self, id: rd_core::LinkFilterRuleId) -> Result<()> {
        writer::request(&self.writer, |reply| ConfigCommand::DeleteLinkFilterRule {
            id,
            reply,
        })
        .await
    }

    /// Puts the listed rules first, in that order; the others follow in theirs.
    pub async fn reorder_link_filter_rules(
        &self,
        ids: Vec<rd_core::LinkFilterRuleId>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            ConfigCommand::ReorderLinkFilterRules { ids, reply }
        })
        .await
    }

    /// Decides every open LinkGrabber link anew by the rules; nothing in the downloads changes.
    pub async fn apply_link_filters(&self) -> Result<LinkFilterOutcome> {
        let outcome = writer::request(&self.writer, |reply| CollectorCommand::ApplyLinkFilters {
            reply,
        })
        .await?;
        // A route may have emptied a package, and its password goes with it (RD-190-04).
        if outcome.routed > 0 {
            self.sweep_archive_passwords().await;
        }
        Ok(outcome)
    }

    /// Shows links a rule hid, until the rules are applied again; answers how many were hidden.
    pub async fn show_filtered_candidates(&self, ids: Vec<rd_core::CandidateId>) -> Result<u64> {
        writer::request(&self.writer, |reply| {
            CollectorCommand::ShowFilteredCandidates { ids, reply }
        })
        .await
    }
}
