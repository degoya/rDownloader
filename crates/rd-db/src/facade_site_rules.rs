//! Database facade methods for user-written site rules (RD-110-04).

use anyhow::Result;

use crate::{
    Database, commands::ConfigCommand, site_rules_store, site_rules_store::NewUserSiteRule,
    site_rules_store::UserSiteRule, writer,
};

impl Database {
    /// Every user rule, by id.
    pub async fn list_site_rules(&self) -> Result<Vec<UserSiteRule>> {
        site_rules_store::list_site_rules(&self.readers).await
    }

    /// Writes a user rule, replacing an earlier one of the same id.
    ///
    /// The body is stored as given: the caller has parsed and validated it through
    /// `rd_siterules::Rule` before it arrives here.
    pub async fn upsert_site_rule(&self, input: NewUserSiteRule) -> Result<UserSiteRule> {
        writer::request(&self.writer, |reply| ConfigCommand::UpsertSiteRule {
            input,
            reply,
        })
        .await
    }

    /// Removes a user rule; returns whether one was there.
    pub async fn delete_site_rule(&self, id: &str) -> Result<bool> {
        writer::request(&self.writer, |reply| ConfigCommand::DeleteSiteRule {
            id: id.to_owned(),
            reply,
        })
        .await
    }

    /// Removes every user rule and every self-test result; returns how many rules went. The
    /// group switches stay (RD-1230-03).
    pub async fn delete_all_site_rules(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| ConfigCommand::DeleteAllSiteRules {
            reply,
        })
        .await
    }
}
