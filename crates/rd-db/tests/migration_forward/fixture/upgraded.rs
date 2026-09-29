//! What the release fixture must look like after the upgrade.

use chrono::SecondsFormat;
use rd_core::PackageState;
use sqlx::Connection;

use super::{
    ACCOUNT, ACCOUNT_SECRET_REF, CHUNK, DOWNLOADS, FINISHED, GRABBER_FIRST, GRABBER_LAST,
    GRABBER_NZB, PACKAGE_COMPLETED, PACKAGES, ROOT_DOWNLOADS, SITE_RULE, SITE_RULES, source_url,
};

/// Asserts the fixture came through the upgrade from migration `version` intact.
pub(crate) async fn assert_upgraded(
    database: &rd_db::Database,
    path: &std::path::Path,
    release: &str,
    version: i64,
) {
    let packages = database.list_packages().await.expect("packages");
    assert_eq!(
        packages.len(),
        PACKAGES.len(),
        "{release}: a package is gone"
    );
    for expected in PACKAGES {
        let package = packages
            .iter()
            .find(|package| package.id.to_string() == expected.id)
            .unwrap_or_else(|| panic!("{release}: package {} is gone", expected.name));
        let name = expected.name;
        assert_eq!(package.name, name, "{release}");
        assert_eq!(package.state, expected.state, "{release}: {name}");
        assert_eq!(package.kind, expected.kind, "{release}: {name}");
        assert_eq!(
            package.category_id.map(|id| id.to_string()).as_deref(),
            expected.category,
            "{release}: {name}"
        );
        // `0048` takes the last write; the releases after it wrote the time themselves.
        let finished = (expected.state == PackageState::Completed).then_some(FINISHED);
        assert_eq!(
            package
                .completed_at
                .map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true))
                .as_deref(),
            finished,
            "{release}: {name} finish time"
        );
    }

    let downloads = database.list_downloads().await.expect("downloads");
    assert_eq!(
        downloads.len(),
        DOWNLOADS.len(),
        "{release}: a download is gone"
    );
    for expected in DOWNLOADS {
        let name = expected.file_name;
        let download = downloads
            .iter()
            .find(|download| download.id.to_string() == expected.id)
            .unwrap_or_else(|| panic!("{release}: download {name} is gone"));
        assert_eq!(
            download.package_id.to_string(),
            expected.package,
            "{release}: {name}"
        );
        assert_eq!(download.file_name, name, "{release}");
        assert_eq!(
            download.source.as_str(),
            source_url(name),
            "{release}: {name}"
        );
        assert_eq!(download.state, expected.state, "{release}: {name}");
        assert_eq!(download.kind, expected.kind, "{release}: {name}");
        // The number that matters: bytes already on disk must still be accounted for, or the
        // upgrade silently re-downloads them.
        assert_eq!(
            download.committed_bytes.get(),
            expected.committed,
            "{release}: {name}: committed bytes were not carried forward"
        );
        assert_eq!(
            download.total_bytes.map(rd_core::ByteCount::get),
            expected.total,
            "{release}: {name}"
        );
        assert_eq!(
            download.recovery, expected.recovery,
            "{release}: {name} recovery flag"
        );
    }

    let roots = database.list_storage_roots().await.expect("storage roots");
    assert_eq!(roots.len(), 2, "{release}: a storage root is gone");
    let defaults: Vec<&str> = roots
        .iter()
        .filter(|root| root.is_default)
        .map(|root| root.name.as_str())
        .collect();
    assert_eq!(
        defaults,
        ["Archive"],
        "{release}: one default root, the first by name"
    );

    let categories = database.list_categories().await.expect("categories");
    assert_eq!(categories.len(), 2, "{release}: a category is gone");
    let defaults: Vec<&str> = categories
        .iter()
        .filter(|category| category.is_default)
        .map(|category| category.name.as_str())
        .collect();
    assert_eq!(
        defaults,
        ["Books"],
        "{release}: one default category, the first by name"
    );
    for category in &categories {
        assert_eq!(
            category.storage_root_id.to_string(),
            ROOT_DOWNLOADS,
            "{release}"
        );
        assert_eq!(
            category.relative_path,
            category.name.to_lowercase(),
            "{release}"
        );
    }

    let accounts = database.list_accounts().await.expect("accounts");
    assert_eq!(accounts.len(), 1, "{release}: the account is gone");
    let account = &accounts[0];
    assert_eq!(account.id.to_string(), ACCOUNT, "{release}");
    assert_eq!(account.label, "Main account", "{release}");
    assert!(
        account.enabled && account.has_secret && !account.has_cookies,
        "{release}"
    );
    assert!(account.credential_mode.is_none(), "{release}");
    // The reference is what finds the credential in the vault; a changed one loses it.
    assert_eq!(
        database
            .account_secret_refs(account.id)
            .await
            .expect("secret refs"),
        Some((Some(ACCOUNT_SECRET_REF.to_owned()), None)),
        "{release}: the secret reference"
    );

    let steps = database
        .list_postprocess_steps(PACKAGE_COMPLETED)
        .await
        .expect("post-processing steps");
    assert_eq!(steps.len(), 1, "{release}: the extraction step is gone");
    assert_eq!(
        steps[0].code.as_deref(),
        Some("extract.data_damaged"),
        "{release}"
    );
    assert_eq!(
        steps[0].params.get("detail").map(String::as_str),
        Some("Release.part01.rar"),
        "{release}"
    );
    assert_eq!(
        steps[0].message.as_deref(),
        Some("Release.part01.rar"),
        "{release}"
    );

    if version >= SITE_RULES {
        let rules = database.list_site_rules().await.expect("site rules");
        let rule = rules
            .iter()
            .find(|rule| rule.id == SITE_RULE)
            .unwrap_or_else(|| panic!("{release}: the site rule is gone"));
        assert_eq!(rule.group, "ebooks", "{release}: the column");
        assert_eq!(rule.rule["group"], "ebooks", "{release}: the body");
    }

    let repositories = database
        .list_plugin_repositories()
        .await
        .expect("plugin repositories");
    assert!(
        repositories
            .iter()
            .any(|repository| repository.is_official()),
        "{release}: the official plugin repository"
    );

    // Neither the chunk table nor the LinkGrabber's order has a facade read of its own.
    let mut connection = crate::connect(path).await.expect("connect");
    let committed: i64 = sqlx::query_scalar("SELECT committed_offset FROM chunks WHERE id = ?1")
        .bind(CHUNK)
        .fetch_one(&mut connection)
        .await
        .unwrap_or_else(|error| panic!("{release}: the chunk checkpoint: {error}"));
    assert_eq!(committed, 1024, "{release}: the chunk checkpoint");
    let order: Vec<(String, i64)> = sqlx::query_as(
        "SELECT id, position FROM collector_packages
         UNION ALL SELECT id, position FROM nzb_imports
         ORDER BY position",
    )
    .fetch_all(&mut connection)
    .await
    .expect("LinkGrabber order");
    assert_eq!(
        order,
        [
            (GRABBER_FIRST.to_owned(), 1),
            (GRABBER_NZB.to_owned(), 2),
            (GRABBER_LAST.to_owned(), 3),
        ],
        "{release}: the LinkGrabber order"
    );
    connection.close().await.expect("close");
}
