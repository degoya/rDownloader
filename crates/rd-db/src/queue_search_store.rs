//! The queue's packages and files by name, for the search palette (RD-1240-14).
//!
//! Two bounded reads in queue order, so the palette never pulls the whole queue to find three
//! rows. SQLite's `LIKE` ignores case for ASCII letters only; an umlaut is matched as typed.

use anyhow::Result;
use rd_core::{DownloadId, DownloadState, PackageId, PackageState};

use crate::{Database, escape_like, models::PACKAGE_ORDER, parse_id};

/// What the queue holds under a name.
#[derive(Clone, Debug, Default)]
pub struct QueueSearch {
    pub packages: Vec<QueueSearchPackage>,
    pub downloads: Vec<QueueSearchDownload>,
}

#[derive(Clone, Debug)]
pub struct QueueSearchPackage {
    pub id: PackageId,
    pub name: String,
    pub state: PackageState,
}

#[derive(Clone, Debug)]
pub struct QueueSearchDownload {
    pub id: DownloadId,
    pub package_id: PackageId,
    pub package_name: String,
    pub file_name: String,
    pub state: DownloadState,
}

impl Database {
    /// The packages and the files whose name contains `term`, at most `limit` of each, in queue
    /// order. A blank term finds nothing.
    pub async fn search_queue(&self, term: &str, limit: u32) -> Result<QueueSearch> {
        let term = term.trim();
        if term.is_empty() || limit == 0 {
            return Ok(QueueSearch::default());
        }
        let pattern = format!("%{}%", escape_like(term));
        let packages: Vec<(String, String, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT packages.id, packages.name, packages.state FROM packages \
                 WHERE packages.name LIKE ? ESCAPE '\\' {PACKAGE_ORDER}, packages.id ASC LIMIT ?"
        )))
        .bind(&pattern)
        .bind(i64::from(limit))
        .fetch_all(&self.readers)
        .await?;
        let downloads: Vec<(String, String, String, String, String)> =
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT downloads.id, downloads.package_id, packages.name, downloads.file_name, \
                 downloads.state FROM downloads JOIN packages ON packages.id = downloads.package_id \
                 WHERE downloads.file_name LIKE ? ESCAPE '\\' {PACKAGE_ORDER}, \
                 downloads.position ASC, downloads.created_at ASC, downloads.id ASC LIMIT ?"
            )))
            .bind(&pattern)
            .bind(i64::from(limit))
            .fetch_all(&self.readers)
            .await?;
        Ok(QueueSearch {
            packages: packages
                .into_iter()
                .map(|(id, name, state)| {
                    Ok(QueueSearchPackage {
                        id: parse_id(&id)?,
                        name,
                        state: state.parse()?,
                    })
                })
                .collect::<Result<_>>()?,
            downloads: downloads
                .into_iter()
                .map(|(id, package_id, package_name, file_name, state)| {
                    Ok(QueueSearchDownload {
                        id: parse_id(&id)?,
                        package_id: parse_id(&package_id)?,
                        package_name,
                        file_name,
                        state: state.parse()?,
                    })
                })
                .collect::<Result<_>>()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use rd_core::{
        AuthProfileSelection, DownloadId, DownloadKind, DownloadPriority, DownloadState, PackageId,
    };

    use crate::{Database, NewDownload, NewPackage};

    async fn package(database: &Database, directory: &std::path::Path, name: &str) -> PackageId {
        let package = NewPackage {
            id: PackageId::new(),
            name: name.to_owned(),
            destination: directory.join(name).to_string_lossy().into_owned(),
            category_id: None,
            priority: DownloadPriority::default(),
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        };
        let id = package.id;
        database.create_package(package).await.expect("package");
        id
    }

    #[tokio::test]
    async fn names_are_found_in_queue_order_without_case_and_bounded() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = Database::open(directory.path().join("search.sqlite3"))
            .await
            .expect("database");
        let first = package(&database, directory.path(), "Ubuntu 26.04").await;
        package(&database, directory.path(), "Debian 14").await;
        package(&database, directory.path(), "ubuntu-server").await;
        let created = database
            .create_download(NewDownload {
                id: DownloadId::new(),
                package_id: first,
                source: "https://example.invalid/ubuntu.iso".parse().expect("url"),
                file_name: "ubuntu-26.04-desktop.iso".to_owned(),
                total_bytes: None,
                expected_checksum: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: AuthProfileSelection::Auto,
                initial_state: DownloadState::Queued,
                kind: DownloadKind::Http,
                media: None,
                remote_credential_id: None,
                replay: None,
                mirror_group: None,
                enrichment: Vec::new(),
                secret_fragment: None,
            })
            .await
            .expect("download");

        let found = database.search_queue("UBUNTU", 10).await.expect("search");
        assert_eq!(
            found
                .packages
                .iter()
                .map(|package| package.name.as_str())
                .collect::<Vec<_>>(),
            ["Ubuntu 26.04", "ubuntu-server"]
        );
        assert_eq!(found.downloads.len(), 1);
        assert_eq!(found.downloads[0].id, created.id);
        assert_eq!(found.downloads[0].package_name, "Ubuntu 26.04");

        let bounded = database.search_queue("ubuntu", 1).await.expect("search");
        assert_eq!(bounded.packages.len(), 1);
        // `%` and `_` are text, not wildcards.
        assert!(
            database
                .search_queue("%", 10)
                .await
                .expect("search")
                .packages
                .is_empty()
        );
        assert!(
            database
                .search_queue("   ", 10)
                .await
                .expect("search")
                .packages
                .is_empty()
        );
    }
}
