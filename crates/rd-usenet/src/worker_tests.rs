use std::{sync::Arc, time::Duration};

use rd_core::{DownloadPriority, DownloadState, ImportMode, PostprocessKind, PostprocessState};
use rd_db::{
    Database, NewCategory, NewNzbFile, NewNzbImport, NewNzbSegment, NewStorageRoot, NewUsenetServer,
};
use rd_scheduler::{ExternalRunner, SchedulerConfig, SchedulerHandle};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{UsenetRunner, UsenetRunnerConfig};

async fn start_scheduler(directory: &std::path::Path, database: &Database) -> SchedulerHandle {
    start_scheduler_with(
        directory,
        database,
        UsenetRunnerConfig::default().parallel_files,
    )
    .await
}

/// The same, with the number of NZB files the runner may work on at once fixed.
///
/// One at a time is what makes the order of a package's files a fact rather than a race, and
/// the verdict on a missing segment (RD-108-24) is precisely about which file finishes first.
async fn start_scheduler_with(
    directory: &std::path::Path,
    database: &Database,
    parallel_files: usize,
) -> SchedulerHandle {
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secret store");
    let runner: Arc<dyn ExternalRunner> = Arc::new(UsenetRunner::new(
        database.clone(),
        secrets.clone(),
        UsenetRunnerConfig {
            max_file_bytes: 1024 * 1024,
            parallel_files,
        },
    ));
    SchedulerHandle::start(
        database.clone(),
        SchedulerConfig::for_directory(directory.join("fallback")),
        secrets,
        None,
        vec![runner],
    )
    .await
    .expect("scheduler")
}

/// Waits until every file of the package reached one of `states`.
async fn wait_for_files(
    database: &Database,
    package_id: rd_core::PackageId,
    states: &[DownloadState],
) -> Vec<rd_core::DownloadFile> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let files: Vec<_> = database
                .list_downloads()
                .await
                .expect("downloads")
                .into_iter()
                .filter(|file| file.package_id == package_id)
                .collect();
            if !files.is_empty() && files.iter().all(|file| states.contains(&file.state)) {
                return files;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("package reached the expected state")
}

fn server(name: &str, address: std::net::SocketAddr, priority: i32) -> NewUsenetServer {
    NewUsenetServer {
        name: name.to_owned(),
        host: address.ip().to_string(),
        port: address.port(),
        tls: false,
        username: None,
        password_ref: None,
        proxy_profile_id: None,
        priority,
        max_connections: 1,
        enabled: true,
    }
}

#[tokio::test]
async fn queued_import_is_downloaded_into_its_category_and_extracted() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("worker.sqlite"))
        .await
        .expect("database");
    let output_root = directory.path().join("output");
    let root = database
        .create_storage_root(
            rd_core::StorageRootId::new(),
            NewStorageRoot {
                name: "Test".to_owned(),
                path: output_root.to_string_lossy().into_owned(),
                is_default: true,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("storage root");
    let category = database
        .create_category(NewCategory {
            name: "Usenet".to_owned(),
            color: "#38bdf8".to_owned(),
            storage_root_id: root.id,
            relative_path: "usenet".to_owned(),
            is_default: true,
            postprocess_level: None,
            script: None,
            cleanup_extensions: None,
            recursive_unpack: None,
            sfv_verify: None,
            safe_postproc: None,
            delete_par2: None,
            upload_enabled: None,
            upload_remote: None,
        })
        .await
        .expect("category");
    database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({ "auto_extract": true }),
        )
        .await
        .expect("enable auto extraction for the fixture");
    let payload = zip_fixture();
    let corrupt_address = spawn_nntp_fixture(false, payload.clone()).await;
    let backup_address = spawn_nntp_fixture(true, payload.clone()).await;
    database
        .create_usenet_server(server("Corrupt primary", corrupt_address, 0))
        .await
        .expect("NNTP server");
    database
        .create_usenet_server(server("Valid backup", backup_address, 1))
        .await
        .expect("backup NNTP server");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "fixture.nzb".to_owned(),
            sha256: "ef".repeat(32),
            category_id: Some(category.id),
            source: rd_core::IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![NewNzbFile {
                subject: "fixture.bin".to_owned(),
                poster: "fixture".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![NewNzbSegment {
                    number: 1,
                    bytes: u64::try_from(payload.len()).expect("fixture length"),
                    message_id: "part-1@example.test".to_owned(),
                }],
            }],
        })
        .await
        .expect("NZB import");
    let package = database
        .enqueue_nzb_import(
            import.id,
            output_root.join("usenet"),
            DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue as package");
    assert_eq!(package.kind, rd_core::DownloadKind::Usenet);
    assert_eq!(package.nzb_import_id, Some(import.id));

    let scheduler = start_scheduler(directory.path(), &database).await;
    let extraction = rd_extract::ExtractionService::start(
        database.clone(),
        rd_extract::ExtractionConfig {
            default_passwords_file: directory.path().join("passwords.txt"),
            rar_timeout: Duration::from_secs(5),
            default_scripts_directory: std::env::temp_dir().join("rd-scripts-test"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
        },
    );
    let files = wait_for_files(&database, package.id, &[DownloadState::Completed]).await;
    assert_eq!(files[0].file_name, "archive.zip");
    assert!(output_root.join("usenet/fixture/archive.zip").exists());
    let segment = &database.list_nzb_files(import.id).await.expect("files")[0].segments[0];
    assert_eq!(segment.state, rd_core::NzbSegmentState::Completed);
    assert_eq!(segment.server_attempts, 2, "backup server was used");

    // Auto extraction follows the completed package.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let steps = database
                .list_postprocess_steps(&package.id.to_string())
                .await
                .expect("steps");
            if steps.iter().any(|step| {
                step.kind == PostprocessKind::ExtractZip
                    && step.state == PostprocessState::Completed
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("extraction step");
    assert_eq!(
        tokio::fs::read(output_root.join("usenet/fixture/payload.txt"))
            .await
            .expect("extracted file"),
        b"postprocess-ok"
    );
    assert!(output_root.join("usenet/fixture/archive.zip").exists());

    // Crash recovery keeps finished files finished and does not re-download them.
    database
        .recover_interrupted()
        .await
        .expect("crash recovery");
    let files = wait_for_files(&database, package.id, &[DownloadState::Completed]).await;
    assert_eq!(files[0].state, DownloadState::Completed);
    let segment = &database.list_nzb_files(import.id).await.expect("files")[0].segments[0];
    assert_eq!(
        segment.server_attempts, 2,
        "completed file was not downloaded again"
    );
    extraction.shutdown().await;
    scheduler.shutdown().await.expect("scheduler shutdown");
}

#[tokio::test]
async fn missing_segment_without_par2_fails_the_file_after_downloading_the_rest() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("worker.sqlite"))
        .await
        .expect("database");
    let output = directory.path().join("output");
    database
        .create_storage_root(
            rd_core::StorageRootId::new(),
            NewStorageRoot {
                name: "Test".to_owned(),
                path: output.to_string_lossy().into_owned(),
                is_default: true,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("storage root");
    let part = |number: u64, payload: &[u8], total: usize| {
        let begin = (number - 1) * 2 + 1;
        let end = begin + payload.len() as u64 - 1;
        let mut article = format!(
            "222 body follows\r\n=ybegin part={number} line=128 size={total} name=multi.bin\r\n=ypart begin={begin} end={end}\r\n"
        )
        .into_bytes();
        article.extend(yenc_encode(payload));
        article.extend(
            format!(
                "\r\n=yend size={} part={number} pcrc32={:08x}\r\n.\r\n",
                payload.len(),
                crc32fast::hash(payload)
            )
            .as_bytes(),
        );
        article
    };
    let address = spawn_scripted_fixture(vec![
        ("part-1@example.test".to_owned(), None),
        ("part-2@example.test".to_owned(), Some(part(2, b"cd", 4))),
    ])
    .await;
    database
        .create_usenet_server(server("Only server", address, 0))
        .await
        .expect("NNTP server");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "holes.nzb".to_owned(),
            sha256: "ab".repeat(32),
            category_id: None,
            source: rd_core::IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![NewNzbFile {
                subject: "multi.bin".to_owned(),
                poster: "fixture".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![
                    NewNzbSegment {
                        number: 1,
                        bytes: 2,
                        message_id: "part-1@example.test".to_owned(),
                    },
                    NewNzbSegment {
                        number: 2,
                        bytes: 2,
                        message_id: "part-2@example.test".to_owned(),
                    },
                ],
            }],
        })
        .await
        .expect("NZB import");
    let package = database
        .enqueue_nzb_import(import.id, output.clone(), DownloadPriority::Normal, false)
        .await
        .expect("enqueue");
    let scheduler = start_scheduler(directory.path(), &database).await;
    let files = wait_for_files(&database, package.id, &[DownloadState::Failed]).await;
    let error = files[0].last_error.clone().expect("failure recorded");
    assert!(
        error.message.contains("PAR2"),
        "unexpected error: {}",
        error.message
    );
    let nzb_files = database.list_nzb_files(import.id).await.expect("files");
    let states = nzb_files[0]
        .segments
        .iter()
        .map(|segment| (segment.number, segment.state))
        .collect::<Vec<_>>();
    assert_eq!(
        states,
        [
            (1, rd_core::NzbSegmentState::Failed),
            (2, rd_core::NzbSegmentState::Completed)
        ]
    );
    let assembled = tokio::fs::read(nzb_files[0].output_path.as_deref().expect("output path"))
        .await
        .expect("assembled file with zero-filled hole");
    assert_eq!(assembled, [0, 0, b'c', b'd']);
    scheduler.shutdown().await.expect("scheduler shutdown");
}

/// A single-part yEnc article announcing `name` for `payload`.
fn single_part_article(name: &str, payload: &[u8]) -> Vec<u8> {
    let mut article = format!(
        "222 body follows\r\n=ybegin line=128 size={} name={name}\r\n",
        payload.len()
    )
    .into_bytes();
    article.extend(yenc_encode(payload));
    article.extend(
        format!(
            "\r\n=yend size={} crc32={:08x}\r\n.\r\n",
            payload.len(),
            crc32fast::hash(payload)
        )
        .as_bytes(),
    );
    article
}

fn storage_root_at(output: &std::path::Path) -> NewStorageRoot {
    NewStorageRoot {
        name: "Test".to_owned(),
        path: output.to_string_lossy().into_owned(),
        is_default: true,
        minimum_free_bytes: None,
    }
}

/// RD-108-23: a finished file whose content is PAR2 is recovery data whatever its name says.
///
/// SABnzbd's `handle_par2` recognises the file the same way (`is_par2_file`), and the check
/// itself already existed in `rd-postprocess`; it is applied here in addition to the name,
/// the moment the assembled file is on disk.
#[tokio::test]
async fn a_finished_file_with_a_par2_header_is_marked_as_recovery_data() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("worker.sqlite"))
        .await
        .expect("database");
    let output = directory.path().join("output");
    database
        .create_storage_root(rd_core::StorageRootId::new(), storage_root_at(&output))
        .await
        .expect("storage root");
    let mut payload = b"PAR2\0PKT".to_vec();
    payload.extend(std::iter::repeat_n(0_u8, 56));
    let address = spawn_nntp_fixture(true, payload.clone()).await;
    database
        .create_usenet_server(server("Only server", address, 0))
        .await
        .expect("NNTP server");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "obfuscated.nzb".to_owned(),
            sha256: "cd".repeat(32),
            category_id: None,
            source: rd_core::IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![NewNzbFile {
                subject: "archive.zip".to_owned(),
                poster: "fixture".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![NewNzbSegment {
                    number: 1,
                    bytes: u64::try_from(payload.len()).expect("fixture length"),
                    message_id: "part-1@example.test".to_owned(),
                }],
            }],
        })
        .await
        .expect("NZB import");
    let package = database
        .enqueue_nzb_import(import.id, output.clone(), DownloadPriority::Normal, false)
        .await
        .expect("enqueue");
    let queued = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .find(|file| file.package_id == package.id)
        .expect("queued row");
    assert!(!queued.recovery, "the name alone says nothing about PAR2");

    let scheduler = start_scheduler(directory.path(), &database).await;
    let files = wait_for_files(&database, package.id, &[DownloadState::Completed]).await;
    assert_eq!(files[0].file_name, "archive.zip");
    assert!(
        files[0].recovery,
        "a file that starts with the PAR2 packet magic is recovery data"
    );
    scheduler.shutdown().await.expect("scheduler shutdown");
}

/// RD-108-23: a hole in the payload is not `usenet.segments_missing_no_par2` when the NZB's
/// PAR2 file announces itself the way the finding's poster does - release name quoted first,
/// file name second. That question used to be answered on the first quoted group alone.
#[tokio::test]
async fn a_missing_segment_relies_on_par2_that_only_the_second_quoted_group_names() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("worker.sqlite"))
        .await
        .expect("database");
    let output = directory.path().join("output");
    database
        .create_storage_root(rd_core::StorageRootId::new(), storage_root_at(&output))
        .await
        .expect("storage root");
    let part = |number: u64, payload: &[u8], total: usize| {
        let begin = (number - 1) * 2 + 1;
        let end = begin + payload.len() as u64 - 1;
        let mut article = format!(
            "222 body follows\r\n=ybegin part={number} line=128 size={total} name=multi.bin\r\n=ypart begin={begin} end={end}\r\n"
        )
        .into_bytes();
        article.extend(yenc_encode(payload));
        article.extend(
            format!(
                "\r\n=yend size={} part={number} pcrc32={:08x}\r\n.\r\n",
                payload.len(),
                crc32fast::hash(payload)
            )
            .as_bytes(),
        );
        article
    };
    let mut index = b"PAR2\0PKT".to_vec();
    index.extend(std::iter::repeat_n(0_u8, 56));
    let address = spawn_scripted_fixture(vec![
        ("part-1@example.test".to_owned(), None),
        ("part-2@example.test".to_owned(), Some(part(2, b"cd", 4))),
        (
            "index@example.test".to_owned(),
            Some(single_part_article("abc.par2", &index)),
        ),
    ])
    .await;
    database
        .create_usenet_server(server("Only server", address, 0))
        .await
        .expect("NNTP server");
    let segment = |number: u32, bytes: u64, message_id: &str| NewNzbSegment {
        number,
        bytes,
        message_id: message_id.to_owned(),
    };
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "holes.nzb".to_owned(),
            sha256: "ac".repeat(32),
            category_id: None,
            source: rd_core::IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![
                NewNzbFile {
                    subject: "multi.bin".to_owned(),
                    poster: "fixture".to_owned(),
                    groups: vec!["alt.binaries.test".to_owned()],
                    segments: vec![
                        segment(1, 2, "part-1@example.test"),
                        segment(2, 2, "part-2@example.test"),
                    ],
                },
                NewNzbFile {
                    subject: r#""Release.1984.German.x265-FuN" - [2/2] - "abc.par2" yEnc (1/1)"#
                        .to_owned(),
                    poster: "fixture".to_owned(),
                    groups: vec!["alt.binaries.test".to_owned()],
                    segments: vec![segment(1, 64, "index@example.test")],
                },
            ],
        })
        .await
        .expect("NZB import");
    let package = database
        .enqueue_nzb_import(import.id, output.clone(), DownloadPriority::Normal, false)
        .await
        .expect("enqueue");
    let scheduler = start_scheduler(directory.path(), &database).await;
    let files = wait_for_files(
        &database,
        package.id,
        &[DownloadState::Completed, DownloadState::Failed],
    )
    .await;
    let payload = files
        .iter()
        .find(|file| file.file_name == "multi.bin")
        .expect("payload row");
    assert_eq!(
        payload.state,
        DownloadState::Completed,
        "the hole is left to PAR2: {:?}",
        payload.last_error
    );
    scheduler.shutdown().await.expect("scheduler shutdown");
}

/// NNTP fixture answering scripted BODY requests on any number of connections; `None` yields 430.
async fn spawn_scripted_fixture(articles: Vec<(String, Option<Vec<u8>>)>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let articles = std::sync::Arc::new(articles);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let articles = std::sync::Arc::clone(&articles);
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut read = BufReader::new(read);
                write
                    .write_all(b"200 fixture ready\r\n")
                    .await
                    .expect("greeting");
                loop {
                    let mut command = String::new();
                    if read.read_line(&mut command).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let requested = command
                        .trim()
                        .strip_prefix("BODY <")
                        .and_then(|rest| rest.strip_suffix('>'))
                        .expect("bracketed BODY command")
                        .to_owned();
                    let article = articles
                        .iter()
                        .find(|(id, _)| *id == requested)
                        .map(|(_, article)| article.clone())
                        .expect("scripted message id");
                    let reply = crate::test_support::named_answer(
                        article.unwrap_or_else(|| b"430 No such article\r\n".to_vec()),
                        &requested,
                    );
                    if write.write_all(&reply).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    address
}

async fn spawn_nntp_fixture(valid_crc: bool, payload: Vec<u8>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("fixture client");
        let (read, mut write) = stream.into_split();
        let mut read = BufReader::new(read);
        write
            .write_all(b"200 fixture ready\r\n")
            .await
            .expect("greeting");
        let mut command = String::new();
        read.read_line(&mut command).await.expect("BODY command");
        assert_eq!(command, "BODY <part-1@example.test>\r\n");
        let crc = if valid_crc {
            crc32fast::hash(&payload)
        } else {
            0
        };
        let mut article = format!(
            "222 0 <part-1@example.test> body follows\r\n=ybegin line=128 size={} name=archive.zip\r\n",
            payload.len()
        )
        .into_bytes();
        article.extend(yenc_encode(&payload));
        article.extend(
            format!("\r\n=yend size={} crc32={crc:08x}\r\n.\r\n", payload.len()).as_bytes(),
        );
        write.write_all(&article).await.expect("article");
    });
    address
}

fn yenc_encode(payload: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(payload.len());
    for byte in payload {
        let shifted = byte.wrapping_add(42);
        if matches!(shifted, 0 | 10 | 13 | 61) {
            encoded.push(b'=');
            encoded.push(shifted.wrapping_add(64));
        } else {
            encoded.push(shifted);
        }
    }
    encoded
}

fn zip_fixture() -> Vec<u8> {
    use std::io::Write;

    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    archive
        .start_file("payload.txt", zip::write::SimpleFileOptions::default())
        .expect("ZIP member");
    archive.write_all(b"postprocess-ok").expect("ZIP payload");
    archive.finish().expect("ZIP finish").into_inner()
}

/// A recovery volume that expired on the servers is not the same event as a lost payload
/// file, and the queue has to be able to tell them apart (RD-107-10).
///
/// Before this both ended as `usenet.all_segments_missing`, which is why a package whose
/// payload was complete and which unpacked cleanly still showed an error row.
#[test]
fn a_lost_recovery_volume_is_coded_apart_from_a_lost_payload_file() {
    let volume = crate::worker::missing_everything_failure(
        "[04/48] - \"Release.vol012+10.par2\" yEnc (1/42)",
    );
    assert_eq!(volume.code.as_deref(), Some("usenet.recovery_unavailable"));

    let index = crate::worker::missing_everything_failure("[01/48] - \"Release.par2\" yEnc (1/3)");
    assert_eq!(index.code.as_deref(), Some("usenet.recovery_unavailable"));
}

/// The boundary this fix must not cross: a missing payload file keeps the failure it had.
#[test]
fn a_lost_payload_file_still_reports_the_old_failure() {
    let payload =
        crate::worker::missing_everything_failure("[07/48] - \"Release.part03.rar\" yEnc (1/420)");
    assert_eq!(payload.code.as_deref(), Some("usenet.all_segments_missing"));
    assert_eq!(payload.category, rd_core::FailureKind::Permanent);
}

/// Builds a two-part payload article for a file called `multi.bin`.
fn multi_part(number: u64, payload: &[u8], total: usize) -> Vec<u8> {
    let begin = (number - 1) * 2 + 1;
    let end = begin + payload.len() as u64 - 1;
    let mut article = format!(
        "222 body follows\r\n=ybegin part={number} line=128 size={total} name=multi.bin\r\n=ypart begin={begin} end={end}\r\n"
    )
    .into_bytes();
    article.extend(yenc_encode(payload));
    article.extend(
        format!(
            "\r\n=yend size={} part={number} pcrc32={:08x}\r\n.\r\n",
            payload.len(),
            crc32fast::hash(payload)
        )
        .as_bytes(),
    );
    article
}

fn nzb_segment(number: u32, bytes: u64, message_id: &str) -> NewNzbSegment {
    NewNzbSegment {
        number,
        bytes,
        message_id: message_id.to_owned(),
    }
}

/// A payload with a hole and a second file, both under subjects that say nothing.
///
/// `index` is the body served for the second file; the scripted fixture answers article 1 of
/// the payload with a 430, so the payload always ends up with one hole.
fn obfuscated_import(digest: &str, second_file: &str) -> NewNzbImport {
    NewNzbImport {
        name: "obfuscated.nzb".to_owned(),
        sha256: digest.repeat(32),
        category_id: None,
        source: rd_core::IngressSource::Manual,
        priority: None,
        import_mode: ImportMode::Enqueue,
        source_path: None,
        password: None,
        announce_arrival: true,
        files: vec![
            NewNzbFile {
                subject: "a1b2c3d4e5f6".to_owned(),
                poster: "fixture".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![
                    nzb_segment(1, 2, "part-1@example.test"),
                    nzb_segment(2, 2, "part-2@example.test"),
                ],
            },
            NewNzbFile {
                subject: second_file.to_owned(),
                poster: "fixture".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![nzb_segment(1, 64, "second@example.test")],
            },
        ],
    }
}

/// RD-108-24: a fully obfuscated set with PAR2 repairs the file with a hole.
///
/// This is the limit RD-108-23 wrote down and left open. No subject names a PAR2 file, so
/// `package_has_par2` could only ever have answered from a row that had already declared
/// itself — and with the payload running first, none had. The verdict was
/// `usenet.segments_missing_no_par2` at a set that carries PAR2. It is taken when the package
/// settles now, and by then the second file has been recognised by its header.
#[tokio::test]
async fn a_fully_obfuscated_set_with_par2_repairs_the_file_with_a_hole() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("worker.sqlite"))
        .await
        .expect("database");
    let output = directory.path().join("output");
    database
        .create_storage_root(rd_core::StorageRootId::new(), storage_root_at(&output))
        .await
        .expect("storage root");
    let mut index = b"PAR2\0PKT".to_vec();
    index.extend(std::iter::repeat_n(0_u8, 56));
    let address = spawn_scripted_fixture(vec![
        ("part-1@example.test".to_owned(), None),
        (
            "part-2@example.test".to_owned(),
            Some(multi_part(2, b"cd", 4)),
        ),
        (
            "second@example.test".to_owned(),
            Some(single_part_article("f7a8b9c0.bin", &index)),
        ),
    ])
    .await;
    database
        .create_usenet_server(server("Only server", address, 0))
        .await
        .expect("NNTP server");
    let import = database
        .add_nzb_import(obfuscated_import("ad", "f7a8b9c0"))
        .await
        .expect("NZB import");
    let package = database
        .enqueue_nzb_import(import.id, output.clone(), DownloadPriority::Normal, false)
        .await
        .expect("enqueue");

    // One file at a time, so the payload with the hole really is the first to finish.
    let scheduler = start_scheduler_with(directory.path(), &database, 1).await;
    let files = wait_for_files(
        &database,
        package.id,
        &[DownloadState::Completed, DownloadState::Failed],
    )
    .await;
    let payload = files
        .iter()
        .find(|file| file.file_name == "multi.bin")
        .expect("payload row");
    assert_eq!(
        payload.state,
        DownloadState::Completed,
        "the set carries PAR2, so the hole goes to repair: {:?}",
        payload.last_error
    );
    assert!(payload.last_error.is_none());
    let recovery = files
        .iter()
        .find(|file| file.file_name == "f7a8b9c0.bin")
        .expect("second row");
    assert!(
        recovery.recovery,
        "the second file declared itself by its header, which is what settled the verdict"
    );
    scheduler.shutdown().await.expect("scheduler shutdown");
}

/// RD-108-24: the limit this job must not move.
///
/// The same package without PAR2 anywhere. The payload with the hole is still the first to
/// finish, so the verdict waits — and once the second file has arrived and nothing of the set
/// is on its way any more, it is the same verdict, with the same code and the same count.
#[tokio::test]
async fn a_fully_obfuscated_set_without_par2_still_fails_the_file_with_a_hole() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("worker.sqlite"))
        .await
        .expect("database");
    let output = directory.path().join("output");
    database
        .create_storage_root(rd_core::StorageRootId::new(), storage_root_at(&output))
        .await
        .expect("storage root");
    let address = spawn_scripted_fixture(vec![
        ("part-1@example.test".to_owned(), None),
        (
            "part-2@example.test".to_owned(),
            Some(multi_part(2, b"cd", 4)),
        ),
        (
            "second@example.test".to_owned(),
            Some(single_part_article(
                "f7a8b9c0.bin",
                b"no recovery data here",
            )),
        ),
    ])
    .await;
    database
        .create_usenet_server(server("Only server", address, 0))
        .await
        .expect("NNTP server");
    let import = database
        .add_nzb_import(obfuscated_import("ae", "f7a8b9c0"))
        .await
        .expect("NZB import");
    let package = database
        .enqueue_nzb_import(import.id, output.clone(), DownloadPriority::Normal, false)
        .await
        .expect("enqueue");

    let scheduler = start_scheduler_with(directory.path(), &database, 1).await;
    let files = wait_for_files(
        &database,
        package.id,
        &[DownloadState::Completed, DownloadState::Failed],
    )
    .await;
    let payload = files
        .iter()
        .find(|file| file.file_name == "multi.bin")
        .expect("payload row");
    assert_eq!(payload.state, DownloadState::Failed);
    let failure = payload.last_error.clone().expect("failure recorded");
    assert_eq!(
        failure.code.as_deref(),
        Some("usenet.segments_missing_no_par2"),
        "the set really has no PAR2, and the message says so"
    );
    assert_eq!(failure.params.get("missing").map(String::as_str), Some("1"));
    scheduler.shutdown().await.expect("scheduler shutdown");
}
