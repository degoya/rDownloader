//! `rdownloader doctor site-rules`: ask every rule about its own page (RD-110-09).
//!
//! The run is triggered, never scheduled. A download manager that went out and knocked on
//! board pages by itself would be doing something nobody asked it for, so this is a command
//! somebody types -- before a release, or when a paste came back empty.
//!
//! **No captcha broker.** The one-shot command has no scheduler behind it, so a rule whose
//! page demands a captcha reports `blocked` rather than spending a solver's credit. That is
//! the honest reading: a page that cannot be reached without an answer nobody gave is
//! blocked.

use anyhow::{Context, Result};
use clap::Args;
use rd_siterules::{Catalogue, Executor, RuleReport, SystemClock, Verdict, selftest};

/// What the command prints when a rule answered.
const NO_REASON: &str = "-";

#[derive(Args)]
pub struct SiteRulesCheckArgs {
    /// Checks only this rule, by id; repeatable. Without it every rule is checked.
    #[arg(long = "rule")]
    pub rules: Vec<String>,
}

/// Runs the self-test over `catalogue`, one rule at a time.
///
/// Sequential on purpose: a handful of boards asked one after another is polite, and a run
/// that opened twenty connections at once would look to every one of them exactly like what
/// they guard against.
pub async fn check_rules(
    network: &rd_plugin_host::RuleNetwork,
    catalogue: &Catalogue,
    wanted: &[String],
) -> Vec<RuleReport> {
    let mut reports = Vec::new();
    for rule in catalogue.rules() {
        if !wanted.is_empty() && !wanted.iter().any(|id| id == &rule.id) {
            continue;
        }
        let fetcher = network.fetcher();
        let resolver = rd_plugin_host::RuleResolver;
        let clock = SystemClock::new();
        let executor = Executor::new(&fetcher, &resolver, &clock);
        reports.push(selftest::check(&executor, rule).await);
    }
    reports
}

/// The table the command prints: one line per rule, widths from the content.
#[must_use]
pub fn render(reports: &[RuleReport]) -> String {
    if reports.is_empty() {
        return "no rules to check\n".to_owned();
    }
    let rows: Vec<[String; 4]> = reports
        .iter()
        .map(|report| {
            [
                report.rule_id.clone(),
                report.verdict.as_str().to_owned(),
                report.links.to_string(),
                report.reason.unwrap_or(NO_REASON).to_owned(),
            ]
        })
        .collect();
    let header = ["rule", "state", "links", "reason"];
    let mut widths = header.map(str::len);
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let line = |cells: [&str; 4]| {
        let mut text = String::new();
        for (index, (cell, width)) in cells.iter().zip(widths).enumerate() {
            if index + 1 == cells.len() {
                text.push_str(cell);
            } else {
                text.push_str(&format!("{cell:<width$}  "));
            }
        }
        text.push('\n');
        text
    };
    let mut table = line(header);
    for row in &rows {
        table.push_str(&line([&row[0], &row[1], &row[2], &row[3]]));
    }
    table.push_str(&format!("\n{}\n", summary(reports)));
    table
}

/// The one line a person reads when they do not read the table.
fn summary(reports: &[RuleReport]) -> String {
    let not_ok = reports
        .iter()
        .filter(|report| !report.verdict.is_ok())
        .count();
    let checked = match reports.len() {
        1 => "1 rule checked".to_owned(),
        many => format!("{many} rules checked"),
    };
    match not_ok {
        0 => format!("{checked}, all ok"),
        1 => format!("{checked}, 1 not ok"),
        many => format!("{checked}, {many} not ok"),
    }
}

/// What the process exits with: anything but `ok` is a finding, so a release preparation
/// that runs this can stop on it.
#[must_use]
pub fn exit_code(reports: &[RuleReport]) -> i32 {
    i32::from(reports.iter().any(|report| !report.verdict.is_ok()))
}

/// The rows one run writes: every rule it checked, with the refusal's own code beside the
/// four-way verdict so the reason survives the sort.
#[must_use]
pub fn rows(reports: &[RuleReport]) -> Vec<rd_db::NewSiteRuleCheck> {
    reports
        .iter()
        .map(|report| rd_db::NewSiteRuleCheck {
            rule_id: report.rule_id.clone(),
            verdict: report.verdict.as_str().to_owned(),
            code: report.reason.map(str::to_owned),
            links: i64::try_from(report.links).unwrap_or(i64::MAX),
            pages: i64::from(report.pages),
        })
        .collect()
}

/// The rules a later run must not ask again: what the last self-test found dead.
///
/// Read at start by `serve` and handed to the crawler selection, which skips them. A word
/// this build does not know is ignored rather than guessed at, so a row written by a later
/// version costs nothing.
pub async fn dead_rules(database: &rd_db::Database) -> std::collections::BTreeSet<String> {
    match database.list_site_rule_checks().await {
        Ok(checks) => checks
            .into_iter()
            .filter(|check| Verdict::parse(&check.verdict) == Some(Verdict::Dead))
            .map(|check| check.rule_id)
            .collect(),
        Err(error) => {
            tracing::warn!(%error, "the rule self-test results could not be read");
            std::collections::BTreeSet::new()
        }
    }
}

/// Runs the command: checks the rules, stores what it found, prints the table.
pub async fn run(
    database: &rd_db::Database,
    network: &rd_plugin_host::RuleNetwork,
    args: &SiteRulesCheckArgs,
) -> Result<i32> {
    let catalogue = crate::site_rules_cli::load_catalogue(database).await;
    if !args.rules.is_empty() {
        for id in &args.rules {
            anyhow::ensure!(catalogue.get(id).is_some(), "no rule with id {id:?}");
        }
    }
    let reports = check_rules(network, &catalogue, &args.rules).await;
    database
        .record_site_rule_checks(rows(&reports))
        .await
        .context("store the rule self-test results")?;
    print!("{}", render(&reports));
    Ok(exit_code(&reports))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(
        id: &str,
        verdict: Verdict,
        reason: Option<&'static str>,
        links: usize,
    ) -> RuleReport {
        RuleReport {
            rule_id: id.to_owned(),
            rule_name: format!("{id}.test"),
            probe: format!("https://{id}.test/a/b"),
            verdict,
            reason,
            links,
            pages: 1,
        }
    }

    #[test]
    fn the_table_names_every_rule_its_state_and_why() {
        let reports = [
            report("scnlog", Verdict::Ok, None, 12),
            report(
                "downmagaz",
                Verdict::Structural,
                Some("site_rules.structure"),
                0,
            ),
            report("gpaste", Verdict::Dead, Some("site_rules.page_dead"), 0),
        ];
        let table = render(&reports);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines[0], "rule       state       links  reason");
        assert_eq!(lines[1], "scnlog     ok          12     -");
        assert_eq!(
            lines[2],
            "downmagaz  structural  0      site_rules.structure"
        );
        assert_eq!(
            lines[3],
            "gpaste     dead        0      site_rules.page_dead"
        );
        assert_eq!(lines[5], "3 rules checked, 2 not ok");
    }

    #[test]
    fn a_run_in_which_every_rule_answered_exits_zero() {
        let reports = [
            report("scnlog", Verdict::Ok, None, 12),
            report("downmagaz", Verdict::Ok, None, 3),
        ];
        assert_eq!(exit_code(&reports), 0);
        assert!(render(&reports).ends_with("2 rules checked, all ok\n"));
        assert_eq!(exit_code(&[]), 0);
        assert_eq!(render(&[]), "no rules to check\n");
        let one = [report("scnlog", Verdict::Ok, None, 12)];
        assert!(render(&one).ends_with("1 rule checked, all ok\n"));
    }

    /// Any state but `ok` stops a release preparation, blocked and structural included: a
    /// rule that cannot reach its page is no more use than one whose page is gone.
    #[test]
    fn any_state_but_ok_exits_non_zero() {
        for verdict in [Verdict::Structural, Verdict::Blocked, Verdict::Dead] {
            let reports = [
                report("fine", Verdict::Ok, None, 4),
                report("other", verdict, Some("site_rules.structure"), 0),
            ];
            assert_eq!(exit_code(&reports), 1, "{}", verdict.as_str());
        }
    }

    /// The row keeps the refusal's own code beside the four-way verdict, so the reason is
    /// not lost to the sort, and a rule that answered stores no code at all.
    #[test]
    fn a_stored_row_carries_the_verdict_and_the_code() {
        let rows = rows(&[
            report("scnlog", Verdict::Ok, None, 12),
            report("gpaste", Verdict::Dead, Some("site_rules.page_dead"), 0),
        ]);
        assert_eq!(rows[0].verdict, "ok");
        assert_eq!(rows[0].code, None);
        assert_eq!(rows[0].links, 12);
        assert_eq!(rows[1].verdict, "dead");
        assert_eq!(rows[1].code.as_deref(), Some("site_rules.page_dead"));
    }
}
