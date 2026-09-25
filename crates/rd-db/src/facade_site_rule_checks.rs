//! Database facade methods for the rule self-test's results (RD-110-09).

use anyhow::Result;

use crate::{
    Database,
    commands::WriterCommand,
    site_rule_checks_store::{self, NewSiteRuleCheck, SiteRuleCheck},
    writer,
};

impl Database {
    /// What the last self-test said about each rule, by rule id.
    pub async fn list_site_rule_checks(&self) -> Result<Vec<SiteRuleCheck>> {
        site_rule_checks_store::list_site_rule_checks(&self.readers).await
    }

    /// Writes the results of one self-test run.
    pub async fn record_site_rule_checks(&self, checks: Vec<NewSiteRuleCheck>) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::RecordSiteRuleChecks {
            checks,
            reply,
        })
        .await
    }
}
