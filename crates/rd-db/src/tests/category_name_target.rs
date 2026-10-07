//! Category rules whose name pattern targets the package name (RD-1140-02).

use rd_core::{CategoryRuleNameTarget, IngressSource};

use super::{dropped_nzb, routing_category, routing_root};
use crate::{Database, NewCategoryRule};

/// The owner's example: an update release whose hoster files carry obfuscated names.
const RELEASE: &str = "Game.Update.v1.2.0.NSW-GROUP";
const PATTERN: &str = "(?i)update.*nsw-";

fn updates_rule(
    category_id: rd_core::CategoryId,
    name_target: CategoryRuleNameTarget,
) -> NewCategoryRule {
    NewCategoryRule {
        name: "Switch updates".to_owned(),
        priority: 10,
        source: None,
        domain: None,
        protocol: None,
        extension: None,
        mime_type: None,
        name_regex: Some(PATTERN.to_owned()),
        name_target,
        category_id,
        enabled: true,
    }
}

/// Two hoster links of one release, grouped into a package named `RELEASE`, whose file names
/// say nothing about it.
fn obfuscated_release(category_id: Option<rd_core::CategoryId>) -> crate::NewCollectorBatch {
    crate::NewCollectorBatch {
        package_name: Some(RELEASE.to_owned()),
        category_id,
        ..super::mirror_batch(
            &[
                "https://ddownload.com/x1y2z3/a8f3c91d0e.part1.rar",
                "https://ddownload.com/x4y5z6/a8f3c91d0e.part2.rar",
            ],
            vec![
                Some("a8f3c91d0e.part1.rar".to_owned()),
                Some("a8f3c91d0e.part2.rar".to_owned()),
            ],
            Vec::new(),
        )
    }
}

async fn routing_database(
    directory: &tempfile::TempDir,
    file: &str,
) -> (Database, rd_core::Category, rd_core::Category) {
    let database = Database::open(directory.path().join(file))
        .await
        .expect("database");
    let root = routing_root(&database, directory.path()).await;
    let updates = routing_category(&database, root, "Updates", false).await;
    let fallback = routing_category(&database, root, "Fallback", true).await;
    (database, updates, fallback)
}

/// The acceptance case: a LinkGrabber package with obfuscated file names reaches its category
/// through the package name, and every link of it carries that category.
#[tokio::test]
async fn a_package_rule_routes_an_obfuscated_release_by_its_package_name() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, updates, _) = routing_database(&directory, "name-target-package.sqlite").await;
    database
        .create_category_rule(updates_rule(updates.id, CategoryRuleNameTarget::Package))
        .await
        .expect("rule");

    let (_, packages, candidates) = database
        .add_collector_batch(obfuscated_release(None))
        .await
        .expect("batch");

    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].name, RELEASE);
    assert_eq!(packages[0].category_id, Some(updates.id));
    assert_eq!(candidates.len(), 2);
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.category_id == Some(updates.id)),
        "{candidates:?}"
    );
}

/// The same pattern on a rule that targets the file name -- what every rule stored before
/// RD-1140-02 does -- does not see the package name, so the release keeps the default.
#[tokio::test]
async fn a_file_rule_does_not_see_the_package_name() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, updates, fallback) =
        routing_database(&directory, "name-target-file.sqlite").await;
    database
        .create_category_rule(updates_rule(updates.id, CategoryRuleNameTarget::File))
        .await
        .expect("rule");

    let (_, packages, _) = database
        .add_collector_batch(obfuscated_release(None))
        .await
        .expect("batch");

    assert_eq!(packages[0].category_id, Some(fallback.id));
}

/// A category chosen for the intake is a decision about these links; no rule re-evaluates it.
#[tokio::test]
async fn a_chosen_category_is_not_overruled_by_a_package_rule() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, updates, fallback) =
        routing_database(&directory, "name-target-chosen.sqlite").await;
    database
        .create_category_rule(updates_rule(updates.id, CategoryRuleNameTarget::Either))
        .await
        .expect("rule");

    let (_, packages, _) = database
        .add_collector_batch(obfuscated_release(Some(fallback.id)))
        .await
        .expect("batch");

    assert_eq!(packages[0].category_id, Some(fallback.id));
}

/// An NZB is its own package: a package rule matches the NZB's name.
#[tokio::test]
async fn a_package_rule_matches_the_name_of_an_nzb() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, updates, _) = routing_database(&directory, "name-target-nzb.sqlite").await;
    database
        .create_category_rule(updates_rule(updates.id, CategoryRuleNameTarget::Package))
        .await
        .expect("rule");

    let import = database
        .add_nzb_import(dropped_nzb(
            &format!("{RELEASE}.nzb"),
            &"b1".repeat(32),
            None,
            IngressSource::HotFolder,
            Some("/watch/update.nzb"),
        ))
        .await
        .expect("import");

    assert_eq!(import.category_id, Some(updates.id));
}

/// Both write paths bind the new column, and a read gives back what was written.
#[tokio::test]
async fn the_name_target_survives_both_write_paths() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, updates, _) = routing_database(&directory, "name-target-write.sqlite").await;
    let created = database
        .create_category_rule(updates_rule(updates.id, CategoryRuleNameTarget::Package))
        .await
        .expect("create");
    assert_eq!(
        database.list_category_rules().await.expect("rules")[0].name_target,
        CategoryRuleNameTarget::Package
    );

    database
        .update_category_rule(
            created.id,
            updates_rule(updates.id, CategoryRuleNameTarget::Either),
        )
        .await
        .expect("update");
    assert_eq!(
        database.list_category_rules().await.expect("rules")[0].name_target,
        CategoryRuleNameTarget::Either
    );
}
