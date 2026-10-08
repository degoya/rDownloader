//! End-to-end tests of the FTP runner against the in-process fixture server.

mod fixture_server;
#[path = "transfer/harness.rs"]
mod harness;
#[path = "transfer/resume.rs"]
mod resume;

use std::sync::Arc;

use fixture_server::{Behaviour, RemoteFile};
use harness::{Harness, payload, run_limits};
use rd_core::{RemoteAuthMode, RemoteProtocol, RemoteTarget};
use rd_db::NewRemoteCredential;
use rd_ftp::FtpRunner;
use rd_scheduler::{ExternalRunner, RunOutcome};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn a_whole_file_is_downloaded_and_promoted() {
    let harness = Harness::start().await;
    harness
        .fixture
        .put("/pub/movie.bin", RemoteFile::new(payload()));
    harness.fixture.put_directory("/pub");
    let file = harness.queue("/pub/movie.bin", "movie.bin").await;

    let outcome = harness
        .runner()
        .run(
            &file,
            &harness.package_of(&file).await,
            CancellationToken::new(),
            run_limits(),
        )
        .await
        .expect("run");

    assert!(matches!(outcome, RunOutcome::Completed { .. }));
    let written = tokio::fs::read(harness.destination.join("movie.bin"))
        .await
        .expect("final file");
    assert_eq!(written, payload());
    // The staging file must not survive a completed transfer.
    assert!(!harness.part_path(&file).exists());
}

#[tokio::test]
async fn an_interrupted_transfer_resumes_from_the_partial_file() {
    let harness = Harness::start().await;
    harness
        .fixture
        .put("/pub/movie.bin", RemoteFile::new(payload()));
    let file = harness.queue("/pub/movie.bin", "movie.bin").await;

    // First attempt: the server cuts the data connection after 50 000 bytes.
    harness.fixture.set_behaviour(Behaviour {
        truncate_after: Some(50_000),
        ..Behaviour::default()
    });
    let first = harness
        .runner()
        .run(
            &file,
            &harness.package_of(&file).await,
            CancellationToken::new(),
            run_limits(),
        )
        .await
        .expect("first run");
    assert!(
        matches!(first, RunOutcome::Failed(_)),
        "short read must fail"
    );
    let partial = tokio::fs::metadata(harness.part_path(&file))
        .await
        .expect("partial kept");
    assert_eq!(partial.len(), 50_000);

    // Second attempt: the server behaves, and REST continues where the file left off.
    harness.fixture.set_behaviour(Behaviour::default());
    let reloaded = harness
        .database
        .get_download(file.id)
        .await
        .expect("reload")
        .expect("row");
    let second = harness
        .runner()
        .run(
            &reloaded,
            &harness.package_of(&file).await,
            CancellationToken::new(),
            run_limits(),
        )
        .await
        .expect("second run");
    assert!(matches!(second, RunOutcome::Completed { .. }));
    let written = tokio::fs::read(harness.destination.join("movie.bin"))
        .await
        .expect("final file");
    // The whole payload, not the tail appended to a restarted download.
    assert_eq!(written, payload());
}

#[tokio::test]
async fn a_changed_remote_file_blocks_the_resume_and_keeps_the_partial() {
    let harness = Harness::start().await;
    harness.fixture.put(
        "/pub/movie.bin",
        RemoteFile::new(payload()).modified_at("20260101120000"),
    );
    let file = harness.queue("/pub/movie.bin", "movie.bin").await;

    harness.fixture.set_behaviour(Behaviour {
        truncate_after: Some(50_000),
        ..Behaviour::default()
    });
    let _ = harness
        .runner()
        .run(
            &file,
            &harness.package_of(&file).await,
            CancellationToken::new(),
            run_limits(),
        )
        .await
        .expect("first run");

    // The file is replaced on the server: same size, different timestamp.
    harness.fixture.set_behaviour(Behaviour::default());
    harness.fixture.put(
        "/pub/movie.bin",
        RemoteFile::new(payload()).modified_at("20260202080000"),
    );
    let reloaded = harness
        .database
        .get_download(file.id)
        .await
        .expect("reload")
        .expect("row");
    let outcome = harness
        .runner()
        .run(
            &reloaded,
            &harness.package_of(&file).await,
            CancellationToken::new(),
            run_limits(),
        )
        .await
        .expect("second run");

    match outcome {
        RunOutcome::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_ftp::FILE_CHANGED));
        }
        other => panic!("expected a refused resume, got {other:?}"),
    }
    // Refusing must not delete what was already downloaded.
    assert!(harness.part_path(&file).exists());
    assert!(!harness.destination.join("movie.bin").exists());
}

#[tokio::test]
async fn a_server_without_rest_reports_that_resume_is_impossible() {
    let harness = Harness::start().await;
    harness
        .fixture
        .put("/pub/movie.bin", RemoteFile::new(payload()));
    let file = harness.queue("/pub/movie.bin", "movie.bin").await;

    harness.fixture.set_behaviour(Behaviour {
        truncate_after: Some(50_000),
        ..Behaviour::default()
    });
    let _ = harness
        .runner()
        .run(
            &file,
            &harness.package_of(&file).await,
            CancellationToken::new(),
            run_limits(),
        )
        .await
        .expect("first run");

    harness.fixture.set_behaviour(Behaviour {
        refuse_rest: true,
        hide_rest_feature: true,
        ..Behaviour::default()
    });
    let reloaded = harness
        .database
        .get_download(file.id)
        .await
        .expect("reload")
        .expect("row");
    let outcome = harness
        .runner()
        .run(
            &reloaded,
            &harness.package_of(&file).await,
            CancellationToken::new(),
            run_limits(),
        )
        .await
        .expect("second run");

    match outcome {
        RunOutcome::Failed(failure) => {
            // The bug this guards: silently restarting would append the whole file to the
            // 50 000 bytes already on disk and produce a corrupt result.
            assert_eq!(failure.code.as_deref(), Some(rd_ftp::RESUME_UNSUPPORTED));
        }
        other => panic!("expected a refused resume, got {other:?}"),
    }
}

#[tokio::test]
async fn a_directory_is_walked_through_mlsd_and_through_list() {
    for refuse_mlsd in [false, true] {
        let harness = Harness::start().await;
        harness.fixture.put_directory("/pub");
        harness.fixture.put_directory("/pub/extras");
        harness
            .fixture
            .put("/pub/movie.bin", RemoteFile::new(vec![0u8; 10]));
        harness
            .fixture
            .put("/pub/extras/notes.txt", RemoteFile::new(vec![1u8; 20]));
        harness.fixture.set_behaviour(Behaviour {
            refuse_mlsd,
            ..Behaviour::default()
        });
        let credential = harness.credential().await;
        let target = RemoteTarget::parse(
            &format!("ftp://127.0.0.1:{}/pub", harness.fixture.port)
                .parse()
                .expect("url"),
        )
        .expect("target");

        let (probed, used) = harness
            .service
            .probe(&target, Some(credential), None)
            .await
            .expect("probe");
        assert_eq!(used, Some(credential));
        let listing = match probed {
            rd_ftp::Probed::Resolved(listing) => listing,
            rd_ftp::Probed::Failed(failure) => panic!("probe failed: {failure:?}"),
        };
        assert!(!listing.single_file, "a directory is not a single file");
        let paths: Vec<&str> = listing
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(
            paths,
            ["extras", "extras/notes.txt", "movie.bin"],
            "mlsd={refuse_mlsd}"
        );
        let sizes: Vec<Option<u64>> = listing
            .entries
            .iter()
            .map(|entry| entry.size.map(rd_core::ByteCount::get))
            .collect();
        assert_eq!(sizes, [None, Some(20), Some(10)]);
    }
}

#[tokio::test]
async fn a_single_file_link_resolves_without_a_tree() {
    let harness = Harness::start().await;
    harness.fixture.put_directory("/pub");
    harness
        .fixture
        .put("/pub/movie.bin", RemoteFile::new(payload()));
    let credential = harness.credential().await;
    let target = RemoteTarget::parse(
        &format!("ftp://127.0.0.1:{}/pub/movie.bin", harness.fixture.port)
            .parse()
            .expect("url"),
    )
    .expect("target");

    let (probed, _) = harness
        .service
        .probe(&target, Some(credential), None)
        .await
        .expect("probe");
    let listing = match probed {
        rd_ftp::Probed::Resolved(listing) => listing,
        rd_ftp::Probed::Failed(failure) => panic!("probe failed: {failure:?}"),
    };
    assert!(listing.single_file);
    assert_eq!(listing.root, "/pub");
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].path, "movie.bin");
    assert!(
        listing.supports_resume,
        "the fixture advertises REST STREAM"
    );
}

#[tokio::test]
async fn a_link_without_a_stored_login_says_so_instead_of_failing_obscurely() {
    let harness = Harness::start().await;
    let target = RemoteTarget::parse(&"ftp://unconfigured.example/pub/a.bin".parse().expect("url"))
        .expect("target");

    let (probed, used) = harness
        .service
        .probe(&target, None, None)
        .await
        .expect("probe");
    assert!(used.is_none());
    match probed {
        rd_ftp::Probed::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_ftp::NO_CREDENTIAL));
            assert_eq!(
                failure.params.get("host").map(String::as_str),
                Some("unconfigured.example")
            );
        }
        rd_ftp::Probed::Resolved(_) => panic!("there is no server to resolve against"),
    }
}

/// RD-150-03: a link a stranger's document or page proposed is held to the address rule when
/// the socket is opened. This machine is refused by its literal address and by a name that
/// resolves to it, before a single connection is made, and with the stable code.
#[tokio::test]
async fn a_guarded_probe_never_connects_to_this_machine() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let harness = Harness::start().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("address").port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&accepted);
    tokio::spawn(async move {
        while listener.accept().await.is_ok() {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });
    // The person's own network is allowed here; this machine never is.
    let policy = rd_http::AddressPolicy::new(true);
    for host in ["127.0.0.1", "localhost"] {
        let credential = harness
            .database
            .create_remote_credential(NewRemoteCredential {
                name: host.to_owned(),
                protocol: RemoteProtocol::Ftp,
                host: host.to_owned(),
                port,
                username: None,
                auth_mode: RemoteAuthMode::Anonymous,
                passive: true,
                enabled: true,
                secret_ref: None,
                key_ref: None,
                passphrase_ref: None,
            })
            .await
            .expect("credential")
            .id;
        let target = RemoteTarget::parse(
            &format!("ftp://{host}:{port}/pub/a.bin")
                .parse()
                .expect("url"),
        )
        .expect("target");
        let (probed, _) = harness
            .service
            .probe(&target, Some(credential), Some(&policy))
            .await
            .expect("probe");
        match probed {
            rd_ftp::Probed::Failed(failure) => assert_eq!(
                failure.code.as_deref(),
                Some(rd_core::CODE_INTERNAL_ADDRESS),
                "{host}: {failure:?}"
            ),
            rd_ftp::Probed::Resolved(_) => panic!("{host} was probed"),
        }
    }
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        0,
        "a refused server was connected to"
    );
}

/// RD-150-03: an FTP mirror of a multi-source download reports its size and serves a chunk
/// from the chunk's offset, over the runner's range source.
#[tokio::test]
async fn an_ftp_mirror_serves_a_chunk_from_its_offset() {
    use tokio::io::AsyncReadExt as _;

    let harness = Harness::start().await;
    harness
        .fixture
        .put("/pub/mirror.bin", RemoteFile::new(payload()));
    harness.credential().await;
    let source = FtpRunner::new(harness.service.clone())
        .range_source()
        .expect("FTP serves mirrors");
    let target = RemoteTarget::parse(
        &format!("ftp://127.0.0.1:{}/pub/mirror.bin", harness.fixture.port)
            .parse()
            .expect("url"),
    )
    .expect("target");

    assert_eq!(
        source.size(&target, None).await.expect("size"),
        payload().len() as u64
    );
    let mut reader = source.open_at(&target, 1000, None).await.expect("open");
    let mut read = vec![0_u8; 500];
    reader.read_exact(&mut read).await.expect("read");
    assert_eq!(read, payload()[1000..1500]);
}
