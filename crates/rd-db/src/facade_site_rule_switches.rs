//! Database facade methods for the rule and group switches (RD-110-08).

use anyhow::Result;

use crate::{
    Database,
    commands::WriterCommand,
    site_rule_switches_store::{self, SiteRuleSwitch},
    writer,
};

impl Database {
    /// Every decision somebody made about a shipped rule or a group.
    pub async fn list_site_rule_switches(&self) -> Result<Vec<SiteRuleSwitch>> {
        site_rule_switches_store::list_site_rule_switches(&self.readers).await
    }

    /// Switches one shipped rule (`scope` = `rule`) or one group (`scope` = `group`).
    pub async fn set_site_rule_switch(&self, scope: &str, key: &str, enabled: bool) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::SetSiteRuleSwitch {
            scope: scope.to_owned(),
            key: key.to_owned(),
            enabled,
            reply,
        })
        .await
    }
}
