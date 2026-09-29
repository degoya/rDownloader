//! The rules in force, and the one place that assembles them (RD-110-08).
//!
//! Three consumers need the same answer and must not each derive it: `serve` at start, this
//! area's handlers after every write, and `rdownloader doctor site-rules`. The answer is
//! "which rules does this installation consult", and it is the person's own rules minus what
//! somebody switched off, with a switched-off group removing every rule that carries it.
//!
//! **No rule arrives with the binary** (RD-130-07). Until 1.2 a signed pack was compiled in
//! and verified here once per process; the project's rules are now a signed release file that
//! the import verifies and stores as the person's own, switched off. So every rule this
//! installation knows has a row in `site_rules`, and a rule's switch is `site_rules.enabled`,
//! where RD-110-04 put it. A group's switch sits in `site_rule_switches` (migration `0082`),
//! because a group is not a rule; the `rule` scope of that table belonged to the rules of the
//! compiled-in pack alone and migration `0095` emptied it.

use std::collections::{BTreeMap, BTreeSet};

use rd_db::Database;
use rd_siterules::{Catalogue, Rule};

/// What somebody decided about the groups.
#[derive(Clone, Debug, Default)]
pub struct Switches {
    /// Groups that are switched off.
    groups_off: BTreeSet<String>,
}

impl Switches {
    /// Reads the decisions. A failure is a warning and an empty set: a database that cannot
    /// be read is no reason to stop recognising pages.
    pub async fn load(database: &Database) -> Self {
        let rows = match database.list_site_rule_switches().await {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(%error, "the site-rule switches could not be read");
                return Self::default();
            }
        };
        let groups_off = rows
            .into_iter()
            // A scope a later build wrote is ignored rather than guessed at, and so is a
            // `rule` row an older build left behind.
            .filter(|row| !row.enabled && row.scope == rd_db::SCOPE_GROUP)
            .map(|row| row.key)
            .collect();
        Self { groups_off }
    }

    /// Whether a group's switch is on.
    #[must_use]
    pub fn group_on(&self, group: &str) -> bool {
        !self.groups_off.contains(group)
    }
}

/// Every rule this installation consults.
///
/// A rule that does not parse or that the catalogue refuses costs itself and nothing else:
/// the database stores a body as opaque JSON on purpose (RD-110-04), so a row written by an
/// older build is a warning rather than a reason to recognise no page at all.
pub async fn catalogue(database: &Database) -> Catalogue {
    let switches = Switches::load(database).await;
    let mut catalogue = Catalogue::default();
    let stored = match database.list_site_rules().await {
        Ok(stored) => stored,
        Err(error) => {
            tracing::warn!(%error, "site rules could not be read");
            return catalogue;
        }
    };
    let mut admitted = 0usize;
    for row in stored
        .into_iter()
        .filter(|row| row.enabled && switches.group_on(&row.group))
    {
        match serde_json::from_value::<Rule>(row.rule) {
            Ok(rule) => match catalogue.add_user_rule(rule) {
                Ok(()) => admitted += 1,
                Err(error) => tracing::warn!(
                    id = %row.id,
                    code = error.code(),
                    "a site rule was not admitted"
                ),
            },
            Err(error) => tracing::warn!(id = %row.id, %error, "a site rule does not parse"),
        }
    }
    if admitted > 0 {
        tracing::info!(rules = admitted, "site rules loaded");
    }
    catalogue
}

/// What the last self-test said about each rule, by rule id (RD-110-09).
pub(crate) async fn checks(database: &Database) -> BTreeMap<String, rd_db::SiteRuleCheck> {
    match database.list_site_rule_checks().await {
        Ok(rows) => rows
            .into_iter()
            .map(|row| (row.rule_id.clone(), row))
            .collect(),
        Err(error) => {
            tracing::warn!(%error, "the rule self-test results could not be read");
            BTreeMap::new()
        }
    }
}
