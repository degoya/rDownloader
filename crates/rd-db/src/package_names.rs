//! The package-name rules in force for a new package (RD-1140-05).
//!
//! Global switches live in the `service.settings` blob under [`PACKAGE_NAME_RULES_FIELD`] and the
//! global regex pairs under [`PACKAGE_NAME_REGEX_FIELD`]; a category's overrides in
//! `categories.package_name_rules_json` and `package_name_regex_json`. Every path that creates a
//! package asks here, so the LinkGrabber's preview, the queue and the NZB import agree on what a
//! package is called — and on which category's rules decide it: the one the package gets.

use std::collections::HashMap;

use anyhow::Result;
use rd_core::{
    CategoryId, CollectorPackage, PackageNameRegex, PackageNameRulesOverride, PackageNaming,
};
use sqlx::SqliteConnection;

use crate::{Database, json_column::lenient, service_settings::SERVICE_SETTINGS_KEY};

/// The global naming and every category's overrides, read once for a whole listing.
struct ListingRules {
    global: PackageNaming,
    /// By category id as stored.
    overrides: HashMap<String, CategoryNaming>,
}

/// A category's stored overrides, both columns read leniently.
struct CategoryNaming {
    rules: PackageNameRulesOverride,
    regex: Option<Vec<PackageNameRegex>>,
}

impl CategoryNaming {
    fn read(rules: Option<&str>, regex: Option<&str>, category: &str) -> Self {
        Self {
            rules: rules
                .and_then(|value| {
                    lenient(
                        serde_json::from_str(value),
                        "categories",
                        "package_name_rules_json",
                        category,
                    )
                })
                .unwrap_or_default(),
            regex: regex.and_then(|value| {
                lenient(
                    serde_json::from_str(value),
                    "categories",
                    "package_name_regex_json",
                    category,
                )
            }),
        }
    }

    fn over(&self, global: &PackageNaming) -> PackageNaming {
        global.for_category(self.rules, self.regex.as_deref())
    }
}

/// The settings field holding the global switches.
pub const PACKAGE_NAME_RULES_FIELD: &str = "package_name_rules";

/// The settings field holding the global regex pairs.
pub const PACKAGE_NAME_REGEX_FIELD: &str = "package_name_regex";

/// The global naming; a missing blob or field is no rule, an unreadable field too (reported).
async fn global_naming(connection: &mut SqliteConnection) -> Result<PackageNaming> {
    let raw = sqlx::query_scalar::<_, String>("SELECT value_json FROM settings WHERE key = ?")
        .bind(SERVICE_SETTINGS_KEY)
        .fetch_optional(&mut *connection)
        .await?;
    let Some(raw) = raw else {
        return Ok(PackageNaming::default());
    };
    let blob: serde_json::Value = serde_json::from_str(&raw)?;
    use crate::service_settings::service_setting_field_of;
    Ok(PackageNaming {
        rules: service_setting_field_of(&blob, PACKAGE_NAME_RULES_FIELD).unwrap_or_default(),
        regex: service_setting_field_of(&blob, PACKAGE_NAME_REGEX_FIELD).unwrap_or_default(),
    })
}

/// The naming a new package of `category` (its id as stored) gets, read on `connection` — the
/// writer's transaction for the NZB import, a reader otherwise.
pub(crate) async fn naming_for(
    connection: &mut SqliteConnection,
    category: Option<&str>,
) -> Result<PackageNaming> {
    let global = global_naming(connection).await?;
    let Some(category) = category else {
        return Ok(global);
    };
    let stored: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT package_name_rules_json, package_name_regex_json FROM categories WHERE id = ?",
    )
    .bind(category)
    .fetch_optional(&mut *connection)
    .await?;
    let Some((rules, regex)) = stored else {
        return Ok(global);
    };
    Ok(CategoryNaming::read(rules.as_deref(), regex.as_deref(), category).over(&global))
}

impl Database {
    /// The naming a new package of `category` gets: the category's overrides over the global
    /// switches and regex pairs.
    pub async fn package_naming(&self, category: Option<CategoryId>) -> Result<PackageNaming> {
        let mut connection = self.readers.acquire().await?;
        naming_for(
            &mut connection,
            category.map(|id| id.to_string()).as_deref(),
        )
        .await
    }

    /// `name` as a new package of `category` is called: tidied by the rules in force.
    ///
    /// Only for a name the application derived — from a file name, a torrent, the release name
    /// a resolver learned. A name somebody stated or renamed is passed on as it is.
    pub async fn tidy_package_name(
        &self,
        name: &str,
        category: Option<CategoryId>,
    ) -> Result<String> {
        Ok(rd_files::tidy_package_name(
            name,
            &self.package_naming(category).await?,
        ))
    }

    /// Sets `queue_name` on every LinkGrabber package whose derived name the rules change.
    ///
    /// A failed read costs the preview, never the listing: the packages then show the name
    /// they have, and the enqueue reads the rules again.
    pub(crate) async fn fill_queue_names(&self, packages: &mut [CollectorPackage]) {
        if !packages.iter().any(|package| package.auto_named) {
            return;
        }
        let rules = match self.queue_name_rules().await {
            Ok(rules) => rules,
            Err(error) => {
                tracing::warn!(%error, "the package-name rules could not be read");
                return;
            }
        };
        for package in packages {
            package.queue_name = None;
            if !package.auto_named {
                continue;
            }
            let in_force = package
                .category_id
                .and_then(|id| rules.overrides.get(&id.to_string()))
                .map_or_else(
                    || rules.global.clone(),
                    |category| category.over(&rules.global),
                );
            let tidied = rd_files::tidy_package_name(&package.name, &in_force);
            if tidied != package.name {
                package.queue_name = Some(tidied);
            }
        }
    }

    async fn queue_name_rules(&self) -> Result<ListingRules> {
        let mut connection = self.readers.acquire().await?;
        let global = global_naming(&mut connection).await?;
        let rows: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT id, package_name_rules_json, package_name_regex_json FROM categories",
        )
        .fetch_all(&mut *connection)
        .await?;
        let overrides = rows
            .into_iter()
            .map(|(id, rules, regex)| {
                let naming = CategoryNaming::read(rules.as_deref(), regex.as_deref(), &id);
                (id, naming)
            })
            .collect();
        Ok(ListingRules { global, overrides })
    }
}
