//! The rules in force, and the one place that assembles them (RD-110-08).
//!
//! Three consumers need the same answer and must not each derive it: `serve` at start, this
//! area's handlers after every write, and `rdownloader doctor site-rules`. The answer is
//! "which rules does this installation consult", and it is the person's own rules minus what
//! somebody switched off, with a switched-off group removing every rule that carries it.
//!
//! **No rule is compiled into the catalogue** (RD-130-07). Until 1.2 a signed pack was compiled
//! in and verified here once per process; since RD-1230-03 there is no signature at all, rules
//! travel as exchange files, and the app brings only a few examples for free sites, which
//! [`install_examples_once`] stores switched off at the first start. So every rule this
//! installation knows has a row in `site_rules`, and a rule's switch is `site_rules.enabled`,
//! where RD-110-04 put it. A group's switch sits in `site_rule_switches` (migration `0082`),
//! because a group is not a rule; the `rule` scope of that table belonged to the rules of the
//! compiled-in pack alone and migration `0095` emptied it.

use std::collections::{BTreeMap, BTreeSet};

use rand::Rng;
use rd_db::{Database, NewUserSiteRule, SiteRuleOriginKind};
use rd_siterules::{Catalogue, Rule};

/// The settings key this installation's value for the rule variable `device_id` is kept under.
const DEVICE_ID_KEY: &str = "site_rules.device_id";

/// The settings key that records when the example list was installed (RD-1230-03).
const EXAMPLES_KEY: &str = "site_rules.examples_installed";

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

/// This installation's value for the rule variable `device_id` (RD-1170-03): 32 hexadecimal
/// digits, drawn once and kept, the same for every run from then on.
///
/// A page whose script sends a browser fingerprint along with a request -- serienjunkies.org
/// posts a fingerprintjs2 hash beside the captcha token -- is sent this instead. It is not a
/// fingerprint of anything: random, stable, and naming nothing about the machine. `None` when
/// the settings cannot be read or written, which leaves a rule reading it to refuse.
pub async fn device_id(database: &Database) -> Option<String> {
    let mut bytes = [0_u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    let drawn: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    // Written only when absent, so two starts never disagree about the value.
    if let Err(error) = database
        .insert_setting_if_absent(DEVICE_ID_KEY.to_owned(), drawn.into())
        .await
    {
        tracing::warn!(%error, "the site-rule device value could not be stored");
        return None;
    }
    match database.get_setting(DEVICE_ID_KEY).await {
        Ok(Some(serde_json::Value::String(value)))
            if value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
        {
            Some(value)
        }
        Ok(_) => {
            tracing::warn!("the stored site-rule device value is not 32 hexadecimal digits");
            None
        }
        Err(error) => {
            tracing::warn!(%error, "the site-rule device value could not be read");
            None
        }
    }
}

/// Writes every example the app brings whose id no stored rule carries, switched off and with
/// the origin `example`, and answers how many it wrote (RD-1230-03). An example somebody kept,
/// switched on or changed is left as it is.
pub async fn add_missing_examples(database: &Database) -> anyhow::Result<usize> {
    let stored: BTreeSet<String> = database
        .list_site_rules()
        .await?
        .into_iter()
        .map(|row| row.id)
        .collect();
    let mut added = 0usize;
    for rule in rd_siterules::examples() {
        if stored.contains(&rule.id) {
            continue;
        }
        database
            .upsert_site_rule(NewUserSiteRule {
                id: rule.id.clone(),
                name: rule.name.clone(),
                group: rule.group.clone(),
                enabled: false,
                rule: serde_json::to_value(&rule)?,
                origin: SiteRuleOriginKind::Example,
            })
            .await?;
        added += 1;
    }
    Ok(added)
}

/// Installs the example list once per installation, at its first start with this build
/// (RD-1230-03): switched off, so nothing is fetched that nobody asked for.
///
/// The mark is set before the rules are written, on purpose: an installation whose examples
/// somebody deleted must not find them back after the next restart, and a write that fails
/// halfway costs examples the settings page restores with one button, never a decision the
/// person made. `serve` calls this before it builds the catalogue.
pub async fn install_examples_once(database: &Database) {
    let first = database
        .insert_setting_if_absent(
            EXAMPLES_KEY.to_owned(),
            serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
        )
        .await;
    match first {
        Ok(true) => match add_missing_examples(database).await {
            Ok(added) => tracing::info!(rules = added, "example site rules installed"),
            Err(error) => tracing::warn!(%error, "the example site rules could not be installed"),
        },
        Ok(false) => {}
        Err(error) => tracing::warn!(%error, "the example site rules' mark could not be read"),
    }
}
