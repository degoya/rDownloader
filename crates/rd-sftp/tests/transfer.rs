//! End-to-end tests of the SFTP runner against the in-process SSH fixture.

mod fixture_server;
mod support;

use std::sync::Arc;
use std::time::Duration;

use fixture_server::{Behaviour, Fixture, RemoteFile};
use rd_core::{RemoteAuthMode, RemoteProtocol, RemoteTarget};
use rd_db::NewRemoteCredential;
use rd_scheduler::{ExternalRunner, RunOutcome};
use support::{HOST_KEY_A, Harness, other_host_key, payload, run_limits};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_host_key_blocks_the_transfer_and_reports_its_fingerprint() {
    // Default settings: nothing is trusted until a person says so.
    let harness = Harness::start().await;
    harness
        .fixture
        .put("/srv/movie.bin", RemoteFile::new(payload()));
    let file = harness.queue("/srv/movie.bin", "movie.bin").await;

    match harness.run(&file).await {
        RunOutcome::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_sftp::HOST_KEY_UNKNOWN));
            let fingerprint = failure.params.get("fingerprint").expect("fingerprint");
            assert!(fingerprint.starts_with("SHA256:"), "{fingerprint}");
            assert_eq!(
                failure.params.get("algorithm").map(String::as_str),
                Some("ssh-ed25519")
            );
        }
        other => panic!("expected the host key to block the transfer, got {other:?}"),
    }
    assert!(!harness.destination.join("movie.bin").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confirmed_host_key_lets_the_transfer_through() {
    let harness = Harness::start().await;
    harness
        .fixture
        .put("/srv/movie.bin", RemoteFile::new(payload()));
    let file = harness.queue("/srv/movie.bin", "movie.bin").await;

    // First attempt records the fingerprint the user has to confirm.
    let RunOutcome::Failed(blocked) = harness.run(&file).await else {
        panic!("an unknown key must block first");
    };
    let fingerprint = blocked.params.get("fingerprint").expect("fingerprint");
    harness
        .database
        .trust_ssh_host_key(rd_core::SshHostKey {
            host: "127.0.0.1".to_owned(),
            port: harness.fixture.port,
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: fingerprint.clone(),
            first_seen: rd_core::Timestamp::now(),
        })
        .await
        .expect("trust");

    let outcome = harness.run(&harness.reload(&file).await).await;
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    let written = tokio::fs::read(harness.destination.join("movie.bin"))
        .await
        .expect("final file");
    assert_eq!(written, payload());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_host_key_blocks_even_though_first_use_trust_is_on() {
    // The endpoint is trusted, but for a *different* key than the one the server now
    // offers. That is what a rebuilt server and an active interception both look like from
    // the client, so it must block regardless of the first-use setting.
    let harness = Harness::start_with(Fixture::start(other_host_key()).await).await;
    harness.allow_first_use_trust().await;
    harness
        .fixture
        .put("/srv/movie.bin", RemoteFile::new(payload()));

    // Fingerprint of the *other* identity, recorded as the trusted one for this endpoint.
    let impostor = russh::keys::decode_secret_key(HOST_KEY_A, None).expect("key");
    let stored_fingerprint = impostor
        .public_key()
        .fingerprint(russh::keys::HashAlg::Sha256)
        .to_string();
    harness
        .database
        .trust_ssh_host_key(rd_core::SshHostKey {
            host: "127.0.0.1".to_owned(),
            port: harness.fixture.port,
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: stored_fingerprint.clone(),
            first_seen: rd_core::Timestamp::now(),
        })
        .await
        .expect("trust");

    let file = harness.queue("/srv/movie.bin", "movie.bin").await;
    match harness.run(&file).await {
        RunOutcome::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_sftp::HOST_KEY_CHANGED));
            assert_eq!(
                failure.params.get("stored_fingerprint"),
                Some(&stored_fingerprint)
            );
            assert_ne!(
                failure.params.get("fingerprint"),
                Some(&stored_fingerprint),
                "the offered key must differ from the stored one"
            );
        }
        other => panic!("a changed host key must block, got {other:?}"),
    }
    assert!(!harness.destination.join("movie.bin").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_transfer_resumes_from_the_partial_file() {
    let harness = Harness::start().await;
    harness.allow_first_use_trust().await;
    harness
        .fixture
        .put("/srv/movie.bin", RemoteFile::new(payload()));
    let file = harness.queue("/srv/movie.bin", "movie.bin").await;

    harness.fixture.set_behaviour(Behaviour {
        truncate_after: Some(50_000),
        ..Behaviour::default()
    });
    let first = harness.run(&file).await;
    assert!(matches!(first, RunOutcome::Failed(_)), "{first:?}");
    let partial = tokio::fs::metadata(harness.part_path(&file))
        .await
        .expect("partial kept");
    assert_eq!(partial.len(), 50_000);

    harness.fixture.set_behaviour(Behaviour::default());
    let second = harness.run(&harness.reload(&file).await).await;
    assert!(matches!(second, RunOutcome::Completed { .. }), "{second:?}");
    let written = tokio::fs::read(harness.destination.join("movie.bin"))
        .await
        .expect("final file");
    // The whole payload, not the tail appended to a restarted download.
    assert_eq!(written, payload());
}

/// RD-1490-07: a transfer the queue stops while a read is outstanding ends as stopped, not
/// failed, keeps its partial file with the row at its length, and the next run continues it.
#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_transfer_keeps_its_partial_and_resumes() {
    let harness = Harness::start().await;
    harness.allow_first_use_trust().await;
    harness
        .fixture
        .put("/srv/movie.bin", RemoteFile::new(payload()));
    let file = harness.queue("/srv/movie.bin", "movie.bin").await;
    harness.fixture.set_behaviour(Behaviour {
        stall_after: Some(50_000),
        ..Behaviour::default()
    });

    let cancellation = CancellationToken::new();
    let package = harness.package_of(&file).await;
    let runner = harness.runner();
    let part = harness.part_path(&file);
    let stop = async {
        // Stop once everything before the stall is on disk and the next read hangs.
        tokio::time::timeout(Duration::from_secs(20), async {
            while !tokio::fs::metadata(&part)
                .await
                .is_ok_and(|metadata| metadata.len() == 50_000)
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the transfer never reached the stall");
        cancellation.cancel();
    };
    let (stopped, ()) = tokio::join!(
        runner.run(&file, &package, cancellation.clone(), run_limits()),
        stop
    );
    let stopped = stopped.expect("run");
    assert!(matches!(stopped, RunOutcome::Stopped), "{stopped:?}");
    let row = harness.reload(&file).await;
    assert_eq!(row.committed_bytes.get(), 50_000);
    assert!(!harness.destination.join("movie.bin").exists());

    harness.fixture.set_behaviour(Behaviour::default());
    let resumed = harness.run(&row).await;
    assert!(
        matches!(resumed, RunOutcome::Completed { .. }),
        "{resumed:?}"
    );
    let written = tokio::fs::read(harness.destination.join("movie.bin"))
        .await
        .expect("final file");
    assert_eq!(written, payload());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_remote_file_blocks_the_resume_and_keeps_the_partial() {
    let harness = Harness::start().await;
    harness.allow_first_use_trust().await;
    harness.fixture.put(
        "/srv/movie.bin",
        RemoteFile::new(payload()).modified_at(1_767_268_800),
    );
    let file = harness.queue("/srv/movie.bin", "movie.bin").await;

    harness.fixture.set_behaviour(Behaviour {
        truncate_after: Some(50_000),
        ..Behaviour::default()
    });
    let _ = harness.run(&file).await;

    // Same size, different modification time: the file was replaced.
    harness.fixture.set_behaviour(Behaviour::default());
    harness.fixture.put(
        "/srv/movie.bin",
        RemoteFile::new(payload()).modified_at(1_800_000_000),
    );

    match harness.run(&harness.reload(&file).await).await {
        RunOutcome::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_sftp::FILE_CHANGED));
        }
        other => panic!("expected a refused resume, got {other:?}"),
    }
    assert!(harness.part_path(&file).exists(), "the partial is kept");
    assert!(!harness.destination.join("movie.bin").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rejected_password_reports_an_auth_failure() {
    let harness = Harness::start().await;
    harness.allow_first_use_trust().await;
    harness
        .fixture
        .put("/srv/movie.bin", RemoteFile::new(payload()));
    harness.fixture.set_behaviour(Behaviour {
        reject_password: true,
        ..Behaviour::default()
    });
    let file = harness.queue("/srv/movie.bin", "movie.bin").await;

    match harness.run(&file).await {
        RunOutcome::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_sftp::AUTH_FAILED));
            // Retrying a password the server keeps refusing is pointless.
            assert!(!failure.category.is_retryable());
        }
        other => panic!("expected an auth failure, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_directory_is_walked_and_a_single_file_is_not() {
    let harness = Harness::start().await;
    harness.allow_first_use_trust().await;
    harness.fixture.put_directory("/srv");
    harness.fixture.put_directory("/srv/extras");
    harness
        .fixture
        .put("/srv/movie.bin", RemoteFile::new(vec![0u8; 10]));
    harness
        .fixture
        .put("/srv/extras/notes.txt", RemoteFile::new(vec![1u8; 20]));
    let credential = harness.credential().await;

    let directory = RemoteTarget::parse(
        &format!("sftp://127.0.0.1:{}/srv", harness.fixture.port)
            .parse()
            .expect("url"),
    )
    .expect("target");
    let (probed, used) = harness
        .service
        .probe(&directory, Some(credential), None)
        .await
        .expect("probe");
    assert_eq!(used, Some(credential));
    let listing = match probed {
        rd_sftp::Probed::Resolved(listing) => listing,
        rd_sftp::Probed::Failed(failure) => panic!("probe failed: {failure:?}"),
    };
    assert!(!listing.single_file);
    let paths: Vec<&str> = listing
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    assert_eq!(paths, ["extras", "extras/notes.txt", "movie.bin"]);
    // SFTP reads from an explicit offset, so resume never depends on a server capability.
    assert!(listing.supports_resume);

    let single = RemoteTarget::parse(
        &format!("sftp://127.0.0.1:{}/srv/movie.bin", harness.fixture.port)
            .parse()
            .expect("url"),
    )
    .expect("target");
    let (probed, _) = harness
        .service
        .probe(&single, Some(credential), None)
        .await
        .expect("probe");
    let listing = match probed {
        rd_sftp::Probed::Resolved(listing) => listing,
        rd_sftp::Probed::Failed(failure) => panic!("probe failed: {failure:?}"),
    };
    assert!(listing.single_file);
    assert_eq!(listing.root, "/srv");
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].path, "movie.bin");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_without_a_stored_login_says_so() {
    let harness = Harness::start().await;
    let target = RemoteTarget::parse(
        &"sftp://unconfigured.example/srv/a.bin"
            .parse()
            .expect("url"),
    )
    .expect("target");

    let (probed, used) = harness
        .service
        .probe(&target, None, None)
        .await
        .expect("probe");
    assert!(used.is_none());
    match probed {
        rd_sftp::Probed::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_sftp::NO_CREDENTIAL));
            assert_eq!(
                failure.params.get("host").map(String::as_str),
                Some("unconfigured.example")
            );
        }
        rd_sftp::Probed::Resolved(_) => panic!("there is no server to resolve against"),
    }
}

/// RD-150-03: an SFTP link a stranger's document or page proposed is held to the address rule
/// when the socket is opened. This machine is refused by its literal address and by a name
/// that resolves to it, before a single connection is made, and with the stable code.
#[tokio::test(flavor = "multi_thread")]
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
                protocol: RemoteProtocol::Sftp,
                host: host.to_owned(),
                port,
                username: Some("tester".to_owned()),
                auth_mode: RemoteAuthMode::Password,
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
            &format!("sftp://{host}:{port}/srv/a.bin")
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
            rd_sftp::Probed::Failed(failure) => assert_eq!(
                failure.code.as_deref(),
                Some(rd_core::CODE_INTERNAL_ADDRESS),
                "{host}: {failure:?}"
            ),
            rd_sftp::Probed::Resolved(_) => panic!("{host} was probed"),
        }
    }
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        0,
        "a refused server was connected to"
    );
}

/// RD-150-03: an SFTP mirror of a multi-source download reports its size and serves a chunk
/// from the chunk's offset, over the runner's range source.
#[tokio::test(flavor = "multi_thread")]
async fn an_sftp_mirror_serves_a_chunk_from_its_offset() {
    use tokio::io::AsyncReadExt as _;

    let harness = Harness::start().await;
    harness.allow_first_use_trust().await;
    harness
        .fixture
        .put("/srv/mirror.bin", RemoteFile::new(payload()));
    harness.credential().await;
    let source = harness
        .runner()
        .range_source()
        .expect("SFTP serves mirrors");
    let target = RemoteTarget::parse(
        &format!("sftp://127.0.0.1:{}/srv/mirror.bin", harness.fixture.port)
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
