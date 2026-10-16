//! RD-1320-03 — a gallery is fetched with its authentication profile, by the rules the media
//! download keeps: a matching cookie profile reaches gallery-dl as `--cookies`, scoped to the
//! gallery's site, and the file does not outlive the run; without a matching one there is none.

#![cfg(unix)]

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use rd_core::{AuthProfileSelection, DownloadKind, DownloadState, GallerySettings, PackageId};
use rd_db::{Database, NewAuthProfile, NewDownload, NewPackage};
use rd_gallery::GalleryRunner;
use rd_scheduler::{ExternalRunner, RunOutcome};
use secrecy::SecretString;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

struct Harness {
    temp: tempfile::TempDir,
    database: Database,
    secrets: rd_secrets::SecretStore,
    runner: GalleryRunner,
    /// The argument line of the last run.
    log: PathBuf,
    /// The cookie file handed over, copied while the process still held it.
    jar: PathBuf,
}

/// A gallery-dl that records its arguments and the cookie file it was given, then stores one
/// picture into its `-D` folder and names it, as gallery-dl does.
fn fake_gallery_dl(directory: &Path, log: &Path, jar: &Path) -> PathBuf {
    let script = directory.join("gallery-dl");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = '--version' ]; then echo 'gallery-dl 1.30.0'; exit 0; fi\n\
             prev=''; cookies=''; folder=''\n\
             for arg in \"$@\"; do\n\
               if [ \"$prev\" = '--cookies' ]; then cookies=\"$arg\"; fi\n\
               if [ \"$prev\" = '-D' ]; then folder=\"$arg\"; fi\n\
               prev=\"$arg\"\n\
             done\n\
             printf '%s\\n' \"$*\" > '{log}'\n\
             if [ -n \"$cookies\" ] && [ -f \"$cookies\" ]; then cat \"$cookies\" > '{jar}'; fi\n\
             mkdir -p \"$folder\" && printf 'png' > \"$folder/one.png\" && echo \"$folder/one.png\"\n",
            log = log.display(),
            jar = jar.display(),
        ),
    )
    .expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    script
}

async fn harness() -> Harness {
    let temp = tempfile::tempdir().expect("tempdir");
    let log = temp.path().join("gallery-dl.log");
    let jar = temp.path().join("jar.txt");
    let tool = fake_gallery_dl(temp.path(), &log, &jar);
    let database = Database::open(temp.path().join("gallery.sqlite"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(temp.path().join("secrets"))
        .await
        .expect("secret store");
    let settings = Arc::new(RwLock::new(GallerySettings {
        gallery_executable: Some(tool.to_string_lossy().into_owned()),
        ..GallerySettings::default()
    }));
    let runner = GalleryRunner::new(database.clone(), settings).with_auth_profiles(secrets.clone());
    Harness {
        temp,
        database,
        secrets,
        runner,
        log,
        jar,
    }
}

impl Harness {
    /// A cookie profile for `host` beside a tracker's cookie that must never be handed over.
    async fn cookie_profile(&self, host: &str) {
        let blob = format!(
            "# Netscape HTTP Cookie File\n\
             .{host}\tTRUE\t/\tTRUE\t0\tsession\tgallery-secret\n\
             .tracker.invalid\tTRUE\t/\tTRUE\t0\ttrack\tfollow-me\n"
        );
        let secret_ref = self
            .secrets
            .put(SecretString::from(blob))
            .await
            .expect("store cookies");
        self.database
            .create_auth_profile(NewAuthProfile {
                name: host.to_owned(),
                scope: rd_core::AuthScope::parse(host, true).expect("scope"),
                method: rd_core::AuthMethod::Cookies,
                origin: rd_core::AuthOrigin::Manual,
                enabled: true,
                expires_at: None,
                username: None,
                secret_ref: Some(secret_ref),
                certificate_ref: None,
            })
            .await
            .expect("profile");
    }

    /// Runs one gallery download of `source` and returns gallery-dl's argument line.
    async fn run(&self, source: &str, auth_profile: AuthProfileSelection) -> String {
        let package = self
            .database
            .create_package(NewPackage {
                id: PackageId::new(),
                name: "set".to_owned(),
                destination: self.temp.path().join("pkg").to_string_lossy().into_owned(),
                category_id: None,
                priority: rd_core::DownloadPriority::Normal,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("package");
        let file = self
            .database
            .create_download(NewDownload {
                id: rd_core::DownloadId::new(),
                package_id: package.id,
                source: source.parse().expect("url"),
                file_name: "set".to_owned(),
                total_bytes: None,
                expected_checksum: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile,
                initial_state: DownloadState::Queued,
                kind: DownloadKind::Gallery,
                media: None,
                replay: None,
                remote_credential_id: None,
                mirror_group: None,
                enrichment: Vec::new(),
                secret_fragment: None,
            })
            .await
            .expect("download");
        let limits = rd_scheduler::RunLimits {
            max_parallel_requests: 1,
            bandwidth: rd_limits::ScopedLimiter::unlimited(),
            address_policy: None,
        };
        let outcome = self
            .runner
            .run(&file, &package, CancellationToken::new(), limits)
            .await
            .expect("run");
        assert!(
            matches!(outcome, RunOutcome::Completed { .. }),
            "unexpected outcome {outcome:?}"
        );
        std::fs::read_to_string(&self.log).expect("argument line")
    }
}

#[tokio::test]
async fn a_matching_cookie_profile_reaches_gallery_dl_as_a_scoped_file() {
    let harness = harness().await;
    harness.cookie_profile("gallery.example").await;
    let args = harness
        .run(
            "https://www.gallery.example/set/1",
            AuthProfileSelection::Auto,
        )
        .await;
    let path = args
        .split_whitespace()
        .skip_while(|arg| *arg != "--cookies")
        .nth(1)
        .expect("--cookies was passed");
    assert!(
        !Path::new(path).exists(),
        "the cookie file outlived the run"
    );
    assert!(
        !args.contains("gallery-secret"),
        "a cookie value reached argv"
    );
    let jar = std::fs::read_to_string(&harness.jar).expect("cookies handed over");
    assert!(jar.contains("gallery-secret"), "{jar}");
    assert!(
        !jar.contains("follow-me"),
        "a foreign cookie was handed over"
    );
}

#[tokio::test]
async fn without_a_matching_profile_or_with_none_there_are_no_cookies() {
    let harness = harness().await;
    harness.cookie_profile("elsewhere.example").await;
    let args = harness
        .run(
            "https://www.gallery.example/set/1",
            AuthProfileSelection::Auto,
        )
        .await;
    assert!(!args.contains("--cookies"), "{args}");

    harness.cookie_profile("gallery.example").await;
    let args = harness
        .run(
            "https://www.gallery.example/set/2",
            AuthProfileSelection::None,
        )
        .await;
    assert!(!args.contains("--cookies"), "{args}");
    assert!(!harness.jar.exists());
}
