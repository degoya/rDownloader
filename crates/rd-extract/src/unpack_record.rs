//! What an unpack created in its folder (RD-1390-07), recorded so a full package reset deletes
//! exactly that and never an entry that was in a shared folder before.
//!
//! The record is the difference between the folder's entries before and after the set: an
//! unpack merges into an existing folder and replaces same-named entries, so only what is new
//! is provably the archive's. A replaced file stays out of the record - left behind by a reset
//! rather than deleted on a guess.

use std::{
    collections::HashSet,
    ffi::OsString,
    path::{Path, PathBuf},
};

/// A folder's entries before an unpack.
pub(crate) struct Before {
    /// Whether the folder existed at all; one created for the set is the archive's whole.
    existed: bool,
    names: HashSet<OsString>,
}

/// The entries of `destination` before an unpack writes into it.
pub(crate) async fn before(destination: &Path) -> Before {
    let mut names = HashSet::new();
    let Ok(mut entries) = tokio::fs::read_dir(rd_files::long_path(destination)).await else {
        return Before {
            existed: false,
            names,
        };
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        names.insert(entry.file_name());
    }
    Before {
        existed: true,
        names,
    }
}

/// The entries the unpack created: the folder itself when it did not exist before (a folder of
/// its own), else every entry that is new - never an unpack's staging directory.
pub(crate) async fn created(destination: &Path, before: &Before) -> Vec<PathBuf> {
    if !before.existed {
        return if tokio::fs::try_exists(rd_files::long_path(destination))
            .await
            .unwrap_or(false)
        {
            vec![destination.to_owned()]
        } else {
            Vec::new()
        };
    }
    let mut created = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(rd_files::long_path(destination)).await else {
        return created;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        if is_new(&name, before) {
            created.push(destination.join(name));
        }
    }
    created.sort();
    created
}

/// Whether an entry named `name` came out of the unpack.
fn is_new(name: &OsString, before: &Before) -> bool {
    !before.names.contains(name)
        && !name
            .to_string_lossy()
            .starts_with(rd_postprocess::STAGING_PREFIX)
}

/// Adds `created` to the package's record; a failure is logged, never the unpack's.
pub(crate) async fn store(inner: &crate::Inner, owner: &str, created: &[PathBuf]) {
    if created.is_empty() {
        return;
    }
    let Ok(package_id) = owner.parse::<rd_core::PackageId>() else {
        tracing::warn!(%owner, "unpacked entries were not recorded: the owner is no package");
        return;
    };
    let paths = created
        .iter()
        .filter_map(|path| crate::steps::path_string(path).ok())
        .collect();
    if let Err(error) = inner
        .database
        .record_package_outputs(package_id, paths)
        .await
    {
        tracing::warn!(%owner, error = %format!("{error:#}"), "unpacked entries were not recorded");
    }
}

#[cfg(test)]
mod tests {
    use super::{before, created};

    #[tokio::test]
    async fn only_what_the_unpack_added_is_recorded() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let folder = directory.path();
        tokio::fs::write(folder.join("set.part1.rar"), b"volume")
            .await
            .expect("volume");
        tokio::fs::write(folder.join("theirs.txt"), b"someone else's")
            .await
            .expect("foreign file");
        let snapshot = before(folder).await;

        tokio::fs::write(folder.join("film.mkv"), b"film")
            .await
            .expect("unpacked file");
        tokio::fs::create_dir(folder.join("Extras"))
            .await
            .expect("unpacked folder");
        tokio::fs::write(folder.join("theirs.txt"), b"replaced")
            .await
            .expect("replaced file");
        let staging = folder.join(format!("{}abc", rd_postprocess::STAGING_PREFIX));
        tokio::fs::create_dir(&staging).await.expect("staging");

        assert_eq!(
            created(folder, &snapshot).await,
            vec![folder.join("Extras"), folder.join("film.mkv")]
        );
    }

    #[tokio::test]
    async fn a_folder_made_for_the_set_is_recorded_whole() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let own = directory.path().join("Film");
        let snapshot = before(&own).await;

        tokio::fs::create_dir(&own).await.expect("own folder");
        tokio::fs::write(own.join("film.mkv"), b"film")
            .await
            .expect("unpacked file");

        assert_eq!(created(&own, &snapshot).await, vec![own.clone()]);
    }

    /// An unpack into a shared folder records what it put there, never the file somebody else
    /// left beside the archive.
    #[tokio::test]
    async fn an_unpack_records_its_entries_on_the_package() {
        let temp = tempfile::tempdir().expect("tempdir");
        let database = rd_db::Database::open(temp.path().join("extract.sqlite"))
            .await
            .expect("database");
        let destination = temp.path().join("dl");
        std::fs::create_dir_all(&destination).expect("destination");
        std::fs::write(destination.join("theirs.txt"), b"someone else's").expect("foreign");
        let package = database
            .create_package(rd_db::NewPackage {
                id: rd_core::PackageId::new(),
                name: "archives".to_owned(),
                destination: destination.to_string_lossy().into_owned(),
                category_id: None,
                priority: rd_core::DownloadPriority::Normal,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("package");
        std::fs::write(
            destination.join("Film.zip"),
            crate::tests::zip_bytes("film.mkv", b"film"),
        )
        .expect("archive");
        crate::tests::seed_completed_file(&database, package.id, "Film.zip").await;

        crate::tests::run_extraction(&database, temp.path(), package.id).await;

        let plan = database.package_reset_plan(package.id).await.expect("plan");
        assert_eq!(
            plan.unpack_outputs,
            vec![destination.join("film.mkv").to_string_lossy().into_owned()]
        );
        assert_eq!(
            plan.unpack_folders,
            vec![destination.to_string_lossy().into_owned()]
        );
    }
}
