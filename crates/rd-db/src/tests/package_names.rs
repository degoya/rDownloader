//! The package-name rules (RD-1140-05): global switches, a category's override, and the two
//! places in this crate that apply them — the NZB import and the LinkGrabber's queue name.

use rd_core::{IngressSource, PackageNameRulesOverride};

use super::{dropped_nzb, mirrors::mirror_batch, routing_category, routing_root};
use crate::{CollectorPackageChange, Database, SERVICE_SETTINGS_KEY};

async fn rules_on(database: &Database, rules: serde_json::Value) {
    database
        .set_setting(
            SERVICE_SETTINGS_KEY.to_owned(),
            serde_json::json!({ "package_name_rules": rules }),
        )
        .await
        .expect("settings");
}

async fn override_rules(
    database: &Database,
    category: rd_core::CategoryId,
    rules: PackageNameRulesOverride,
) {
    database
        .update_category_postprocess(
            category,
            crate::CategoryPostprocess {
                package_name_rules: Some(rules),
                ..crate::CategoryPostprocess::default()
            },
        )
        .await
        .expect("category rules");
}

#[tokio::test]
async fn without_a_setting_every_name_stays_as_it_is() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("names-off.sqlite"))
        .await
        .expect("database");
    assert_eq!(
        database
            .tidy_package_name("Big Buck Bunny [1080p]", None)
            .await
            .expect("tidy"),
        "Big Buck Bunny [1080p]"
    );
}

#[tokio::test]
async fn a_category_overrides_the_global_switches_it_sets() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("names-override.sqlite"))
        .await
        .expect("database");
    let root = routing_root(&database, directory.path()).await;
    let plain = routing_category(&database, root, "NamesPlain", true).await;
    let lower = routing_category(&database, root, "NamesLower", false).await;
    rules_on(
        &database,
        serde_json::json!({ "spaces_to_dots": true, "strip_bracket_tags": true }),
    )
    .await;
    override_rules(
        &database,
        lower.id,
        PackageNameRulesOverride {
            lowercase: Some(true),
            strip_bracket_tags: Some(false),
            ..PackageNameRulesOverride::default()
        },
    )
    .await;
    let tidy = |category| database.tidy_package_name("Big Buck Bunny [1080p]", category);
    assert_eq!(tidy(None).await.expect("global"), "Big.Buck.Bunny.");
    assert_eq!(
        tidy(Some(plain.id)).await.expect("plain"),
        "Big.Buck.Bunny."
    );
    assert_eq!(
        tidy(Some(lower.id)).await.expect("override"),
        "big.buck.bunny.[1080p]"
    );
    // An override that sets nothing is stored as "inherit".
    override_rules(&database, lower.id, PackageNameRulesOverride::default()).await;
    let stored = database.list_categories().await.expect("categories");
    let lower = stored
        .iter()
        .find(|category| category.id == lower.id)
        .expect("category");
    assert_eq!(lower.package_name_rules, None);
}

/// The NZB path names its package from the file — or from what an *arr adapter sent — and
/// the rules of the category the package lands in decide the name and with it the folder.
#[tokio::test]
async fn an_nzb_package_takes_the_tidied_name_and_folder_of_its_category() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("names-nzb.sqlite"))
        .await
        .expect("database");
    let root = routing_root(&database, directory.path()).await;
    routing_category(&database, root, "NzbNamesDefault", true).await;
    let lower = routing_category(&database, root, "NzbNamesLower", false).await;
    rules_on(&database, serde_json::json!({ "spaces_to_dots": true })).await;
    override_rules(
        &database,
        lower.id,
        PackageNameRulesOverride {
            lowercase: Some(true),
            ..PackageNameRulesOverride::default()
        },
    )
    .await;

    let mut packages = Vec::new();
    for (sha, category) in [("b1", None), ("b2", Some(lower.id))] {
        let import = database
            .add_nzb_import(dropped_nzb(
                "Big Buck Bunny.nzb",
                &sha.repeat(32),
                category,
                IngressSource::Manual,
                None,
            ))
            .await
            .expect("import");
        packages.push(
            database
                .enqueue_nzb_import(
                    import.id,
                    directory.path().join(sha),
                    rd_core::DownloadPriority::Normal,
                    false,
                )
                .await
                .expect("enqueue"),
        );
    }
    assert_eq!(packages[0].name, "Big.Buck.Bunny");
    assert!(packages[0].destination.ends_with("Big.Buck.Bunny"));
    assert_eq!(packages[1].name, "big.buck.bunny");
    assert!(packages[1].destination.ends_with("big.buck.bunny"));
}

/// The LinkGrabber shows the queue name before the package is added; a name somebody gave
/// the package stays.
#[tokio::test]
async fn the_linkgrabber_previews_a_derived_name_and_keeps_a_renamed_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("names-grabber.sqlite"))
        .await
        .expect("database");
    rules_on(&database, serde_json::json!({ "spaces_to_dots": true })).await;
    let urls = [
        "https://one.example/Big%20Buck%20Bunny.part1.rar",
        "https://one.example/Big%20Buck%20Bunny.part2.rar",
    ];
    let (_, packages, _) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_name: None,
            ..mirror_batch(
                &urls,
                vec![
                    Some("Big Buck Bunny.part1.rar".to_owned()),
                    Some("Big Buck Bunny.part2.rar".to_owned()),
                ],
                Vec::new(),
            )
        })
        .await
        .expect("batch");
    let package = &packages[0];
    assert!(package.auto_named);
    assert!(package.name.contains(' '), "{}", package.name);
    assert_eq!(
        package.queue_name.as_deref(),
        Some(package.name.replace(' ', ".").as_str())
    );
    let listed = database.list_collector_packages().await.expect("list");
    assert_eq!(listed[0].queue_name, package.queue_name);

    let renamed = database
        .update_collector_packages(
            vec![package.id],
            CollectorPackageChange {
                name: Some("My Own Name".to_owned()),
                category_id: None,
                priority: None,
                password: None,
                postprocess_level: None,
                script: None,
            },
        )
        .await
        .expect("rename");
    assert!(!renamed[0].auto_named);
    assert_eq!(renamed[0].queue_name, None);

    // A name the request stated is not the LinkGrabber's to tidy either.
    let (_, stated, _) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_name: Some("Stated Name".to_owned()),
            ..mirror_batch(
                &["https://two.example/x.bin"],
                vec![Some("x.bin".to_owned())],
                Vec::new(),
            )
        })
        .await
        .expect("stated batch");
    assert_eq!(stated[0].name, "Stated Name");
    assert_eq!(stated[0].queue_name, None);
}

/// The regex pairs run after the switches; a category's list replaces the global one, an empty
/// list switches it off, and no list inherits it.
#[tokio::test]
async fn a_category_regex_list_replaces_the_global_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("names-regex.sqlite"))
        .await
        .expect("database");
    let root = routing_root(&database, directory.path()).await;
    let inherits = routing_category(&database, root, "RegexInherits", true).await;
    let own = routing_category(&database, root, "RegexOwn", false).await;
    let none = routing_category(&database, root, "RegexNone", false).await;
    database
        .set_setting(
            SERVICE_SETTINGS_KEY.to_owned(),
            serde_json::json!({
                "package_name_rules": { "spaces_to_dots": true },
                "package_name_regex": [{ "pattern": "\\.\\[1080p\\]$", "replacement": "" }]
            }),
        )
        .await
        .expect("settings");
    for (category, regex) in [
        (
            own.id,
            vec![rd_core::PackageNameRegex {
                pattern: r"^(\w+)\.(\w+)".to_owned(),
                replacement: "$2.$1".to_owned(),
            }],
        ),
        (none.id, Vec::new()),
    ] {
        database
            .update_category_postprocess(
                category,
                crate::CategoryPostprocess {
                    package_name_regex: Some(regex),
                    ..crate::CategoryPostprocess::default()
                },
            )
            .await
            .expect("category regex");
    }
    let tidy = |category| database.tidy_package_name("Big Buck Bunny [1080p]", category);
    assert_eq!(
        tidy(Some(inherits.id)).await.expect("inherits"),
        "Big.Buck.Bunny"
    );
    assert_eq!(
        tidy(Some(own.id)).await.expect("own"),
        "Buck.Big.Bunny.[1080p]"
    );
    assert_eq!(
        tidy(Some(none.id)).await.expect("none"),
        "Big.Buck.Bunny.[1080p]"
    );
}
