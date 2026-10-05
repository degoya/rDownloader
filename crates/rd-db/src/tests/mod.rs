//! The crate's unit tests against a real database file, one module per area.

use rd_core::{AuthProfileSelection, ImportMode, IngressSource};

use crate::{Database, NewCategory, NewNzbFile, NewNzbImport, NewNzbSegment, NewStorageRoot};
use mirrors::mirror_batch;

mod auth;
mod automations;
mod categories;
mod collector;
mod downloads;
mod enrichment;
mod grabber_order;
mod held_verdicts;
mod hopeless_sets;
mod mirrors;
mod network;
mod notifications;
mod nzb;
mod nzb_routing;
mod online_check;
mod packages;
mod recovery_volumes;
mod remote_jobs;
mod subscriptions;

/// Default selection for tests that only care about proxy/account precedence.
const SELECTION: AuthProfileSelection = AuthProfileSelection::Auto;

fn probe_url() -> url::Url {
    "https://example.com/file.bin".parse().expect("url")
}

/// Storage root for the category-routing tests; the path itself is never written to.
async fn routing_root(database: &Database, directory: &std::path::Path) -> rd_core::StorageRootId {
    database
        .create_storage_root(
            rd_core::StorageRootId::new(),
            NewStorageRoot {
                name: "Downloads".to_owned(),
                path: directory.to_string_lossy().into_owned(),
                is_default: true,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("storage root")
        .id
}

async fn routing_category(
    database: &Database,
    root_id: rd_core::StorageRootId,
    name: &str,
    is_default: bool,
) -> rd_core::Category {
    database
        .create_category(NewCategory {
            name: name.to_owned(),
            color: "#38BDF8".to_owned(),
            storage_root_id: root_id,
            relative_path: name.to_lowercase(),
            is_default,
            postprocess_level: None,
            script: None,
            cleanup_extensions: None,
            recursive_unpack: None,
            unpack_to_subfolder: None,
            direct_unpack: None,
            malware_scan: None,
            sfv_verify: None,
            safe_postproc: None,
            delete_par2: None,
            upload_enabled: None,
            upload_remote: None,
        })
        .await
        .expect("category")
}

fn dropped_nzb(
    name: &str,
    sha256: &str,
    category_id: Option<rd_core::CategoryId>,
    source: IngressSource,
    source_path: Option<&str>,
) -> NewNzbImport {
    NewNzbImport {
        name: name.to_owned(),
        sha256: sha256.to_owned(),
        category_id,
        source,
        priority: None,
        import_mode: ImportMode::Review,
        source_path: source_path.map(str::to_owned),
        password: None,
        announce_arrival: true,
        files: vec![NewNzbFile {
            subject: "payload.bin".to_owned(),
            poster: "poster".to_owned(),
            groups: vec!["alt.binaries.test".to_owned()],
            segments: vec![NewNzbSegment {
                number: 1,
                bytes: 128,
                message_id: "payload-1@example.test".to_owned(),
            }],
        }],
    }
}

/// One NZB file with a single segment, for the PAR2 postponement fixtures.
fn nzb_file(subject: &str) -> NewNzbFile {
    NewNzbFile {
        subject: subject.to_owned(),
        poster: "poster".to_owned(),
        groups: vec!["alt.binaries.test".to_owned()],
        segments: vec![NewNzbSegment {
            number: 1,
            bytes: 128,
            message_id: format!("{subject}@example.test"),
        }],
    }
}

/// A proposed group, as the third source leaves it: one shared name, no size behind it.
async fn proposed_pair(database: &Database, name: &str) -> Vec<rd_core::LinkCandidate> {
    let urls = [
        format!("https://one.example/{name}"),
        format!("https://two.example/{name}"),
    ];
    let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
    let (_, _, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            // No package name, so the package is auto-named and the regroup after the online
            // check is allowed to touch it -- which is the path this pair exists to test.
            package_name: None,
            ..mirror_batch(&refs, vec![Some(format!("{name}.mkv")); 2], Vec::new())
        })
        .await
        .expect("batch");
    candidates
}
