//! The package, LinkGrabber and NZB lists read one page in SQL with the length of the whole list
//! (RD-191-05), the way the download list does since RD-1120-17.

use rd_core::{DownloadPriority, IngressSource, PackageId};

use super::{dropped_nzb, mirror_batch};
use crate::{Database, NewPackage};

async fn open(directory: &std::path::Path) -> Database {
    Database::open(directory.join("pages.sqlite"))
        .await
        .expect("database")
}

/// A page is a slice of the whole list in its order, and every page counts all of it: `middle`
/// was read with offset 1 and limit 2, `rest` with offset 2 and no limit, `beyond` with an
/// offset of the list's length.
fn assert_slices<I: PartialEq + std::fmt::Debug>(
    whole: &[I],
    middle: (Vec<I>, u64),
    rest: (Vec<I>, u64),
    beyond: (Vec<I>, u64),
) {
    let total = u64::try_from(whole.len()).expect("length");
    assert!(
        whole.len() >= 4,
        "the fixture needs a list longer than a page"
    );
    assert_eq!(middle.1, total);
    assert_eq!(middle.0, whole[1..3], "the page is the list's own slice");
    assert_eq!(rest.1, total);
    assert_eq!(rest.0, whole[2..], "no limit is the rest of the list");
    assert_eq!(beyond.1, total);
    assert!(beyond.0.is_empty(), "a page past the end is empty");
}

#[tokio::test]
async fn the_package_list_pages_in_its_queue_order() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    // Priorities out of creation order, so the queue order is not the order of the rows.
    for (index, priority) in [
        DownloadPriority::Low,
        DownloadPriority::High,
        DownloadPriority::Normal,
        DownloadPriority::High,
        DownloadPriority::Normal,
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("paged-{index}");
        database
            .create_package(NewPackage {
                id: PackageId::new(),
                destination: directory.path().join(&name).to_string_lossy().into_owned(),
                name,
                category_id: None,
                priority,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("package");
    }
    let ids = |page: (Vec<rd_core::DownloadPackage>, u64)| -> (Vec<PackageId>, u64) {
        (page.0.into_iter().map(|row| row.id).collect(), page.1)
    };
    let whole: Vec<PackageId> = database
        .list_packages_with_passwords()
        .await
        .expect("list")
        .into_iter()
        .map(|row| row.id)
        .collect();
    assert_slices(
        &whole,
        ids(database.packages_page(1, Some(2)).await.expect("page")),
        ids(database.packages_page(2, None).await.expect("rest")),
        ids(database.packages_page(5, Some(2)).await.expect("beyond")),
    );
}

#[tokio::test]
async fn the_linkgrabber_lists_page_in_their_own_order() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    for index in 0..4 {
        let urls = [
            format!("https://one.example/paged-{index}.bin"),
            format!("https://two.example/paged-{index}.part2.bin"),
        ];
        let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
        database
            .add_collector_batch(crate::NewCollectorBatch {
                package_name: Some(format!("Paged release {index}")),
                ..mirror_batch(&refs, Vec::new(), Vec::new())
            })
            .await
            .expect("batch");
    }

    let batches = database.list_collector_batches().await.expect("batches");
    let whole: Vec<_> = batches.iter().map(|batch| batch.id).collect();
    let batch_ids = |page: (Vec<rd_core::CollectorBatch>, u64)| -> (Vec<rd_core::BatchId>, u64) {
        (page.0.into_iter().map(|batch| batch.id).collect(), page.1)
    };
    assert_slices(
        &whole,
        batch_ids(
            database
                .collector_batches_page(1, Some(2))
                .await
                .expect("page"),
        ),
        batch_ids(
            database
                .collector_batches_page(2, None)
                .await
                .expect("rest"),
        ),
        batch_ids(
            database
                .collector_batches_page(4, Some(2))
                .await
                .expect("beyond"),
        ),
    );

    let packages = database.list_collector_packages().await.expect("packages");
    let whole: Vec<_> = packages.iter().map(|package| package.id).collect();
    let package_ids =
        |page: (Vec<rd_core::CollectorPackage>, u64)| -> (Vec<rd_core::CollectorPackageId>, u64) {
            (
                page.0.into_iter().map(|package| package.id).collect(),
                page.1,
            )
        };
    let length = u64::try_from(whole.len()).expect("length");
    assert_slices(
        &whole,
        package_ids(
            database
                .collector_packages_page(1, Some(2))
                .await
                .expect("page"),
        ),
        package_ids(
            database
                .collector_packages_page(2, None)
                .await
                .expect("rest"),
        ),
        package_ids(
            database
                .collector_packages_page(length, Some(2))
                .await
                .expect("beyond"),
        ),
    );

    let candidates = database.list_candidates().await.expect("candidates");
    let whole: Vec<_> = candidates.iter().map(|candidate| candidate.id).collect();
    let length = u64::try_from(whole.len()).expect("length");
    let candidate_ids =
        |page: (Vec<rd_core::LinkCandidate>, u64)| -> (Vec<rd_core::CandidateId>, u64) {
            (
                page.0.into_iter().map(|candidate| candidate.id).collect(),
                page.1,
            )
        };
    assert_slices(
        &whole,
        candidate_ids(database.candidates_page(1, Some(2)).await.expect("page")),
        candidate_ids(database.candidates_page(2, None).await.expect("rest")),
        candidate_ids(
            database
                .candidates_page(length, Some(2))
                .await
                .expect("beyond"),
        ),
    );
}

#[tokio::test]
async fn the_nzb_review_list_pages_in_its_own_order() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    for (index, digest) in ["b1", "b2", "b3", "b4"].into_iter().enumerate() {
        database
            .add_nzb_import(dropped_nzb(
                &format!("paged-{index}.nzb"),
                &digest.repeat(32),
                None,
                IngressSource::Manual,
                None,
            ))
            .await
            .expect("import");
    }
    let whole: Vec<_> = database
        .list_nzb_imports()
        .await
        .expect("imports")
        .into_iter()
        .map(|import| import.id)
        .collect();
    let import_ids = |page: (Vec<rd_core::NzbImport>, u64)| -> (Vec<rd_core::NzbImportId>, u64) {
        (page.0.into_iter().map(|import| import.id).collect(), page.1)
    };
    assert_slices(
        &whole,
        import_ids(database.nzb_imports_page(1, Some(2)).await.expect("page")),
        import_ids(database.nzb_imports_page(2, None).await.expect("rest")),
        import_ids(database.nzb_imports_page(4, Some(2)).await.expect("beyond")),
    );
}
