//! The rule pack as this installation sees it (RD-110-04).
//!
//! The commands that sign and verify the rule file, `rdownloader site-rules sign|verify`, live in
//! `rd_pack::site_rules` since RD-150-20, shared with the `rd-pack` binary the release runs.

/// Every rule this installation consults, for the crawler selection (RD-110-06) and for the
/// self-test (RD-110-09).
///
/// The assembly itself -- the stored rules and the switches RD-110-08 records about them --
/// lives in `rd_api::site_rules_service`, because the settings
/// page has to produce the same answer after every write and two copies of that rule would
/// drift. This stays as the name `serve` and `doctor` already call.
pub async fn load_catalogue(database: &rd_db::Database) -> rd_siterules::Catalogue {
    rd_api::site_rule_catalogue(database).await
}
