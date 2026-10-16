//! RD-1320-02 — the LinkGrabber check and the format query of a download use the link's
//! authentication profile as the download does: a matching cookie profile reaches yt-dlp as
//! `--cookies`, a pinned one wins, "no profile" sends none, and a disabled or expired one is
//! never taken. RD-1320-01 — what yt-dlp says about a Facebook reel without a session.

#![cfg(unix)]

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use rd_core::{
    AuthProfileId, AuthProfileSelection, DownloadKind, DownloadState, FailureKind,
    MediaFormatCriteria, MediaKind, MediaSettings, MediaStrictness, PackageId,
};
use rd_db::{Database, NewAuthProfile, NewDownload, NewPackage};
use rd_media::{MediaProbe, MediaRunner, YtDlpProbe};
use rd_scheduler::{ExternalRunner, RunOutcome};
use secrecy::SecretString;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const REEL: &str = "https://www.facebook.com/reel/1428647186119665";

/// yt-dlp's answer for a reel without a session, as the managed 2026.08.19 prints it.
const CANNOT_PARSE: &str = "ERROR: [facebook] 1428647186119665: Cannot parse data; please \
     report this issue on  https://github.com/yt-dlp/yt-dlp/issues?q= , filling out the \
     appropriate issue template. Confirm you are on the latest version using  yt-dlp -U";

struct Harness {
    temp: tempfile::TempDir,
    database: Database,
    secrets: rd_secrets::SecretStore,
    settings: Arc<RwLock<MediaSettings>>,
    /// One `ARGS` line per invocation.
    log: PathBuf,
    /// The cookie files handed over, copied while the process still held them.
    jar: PathBuf,
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"))
        .join(format!("tests/fixtures/{name}.sh"))
}

/// A yt-dlp that records its arguments and the cookie file it was given, refuses `-J`
/// without cookies the way Facebook makes it, and otherwise answers as the shared fixture.
fn signin_ytdlp(directory: &Path, log: &Path, jar: &Path) -> PathBuf {
    let script = directory.join("yt-dlp");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             prev=''; cookies=''\n\
             for arg in \"$@\"; do\n\
               if [ \"$prev\" = '--cookies' ]; then cookies=\"$arg\"; fi; prev=\"$arg\"\n\
             done\n\
             printf 'ARGS %s\\n' \"$*\" >> '{log}'\n\
             if [ -n \"$cookies\" ] && [ -f \"$cookies\" ]; then cat \"$cookies\" >> '{jar}'; fi\n\
             case \" $* \" in *' -J '*)\n\
               if [ -z \"$cookies\" ]; then echo '{CANNOT_PARSE}' >&2; exit 1; fi ;;\n\
             esac\n\
             exec '{fake}' \"$@\"\n",
            log = log.display(),
            jar = jar.display(),
            fake = fixture("fake-yt-dlp").display(),
        ),
    )
    .expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    script
}

async fn harness() -> Harness {
    let temp = tempfile::tempdir().expect("tempdir");
    let log = temp.path().join("ytdlp.log");
    let jar = temp.path().join("jar.txt");
    let ytdlp = signin_ytdlp(temp.path(), &log, &jar);
    let database = Database::open(temp.path().join("media.sqlite"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(temp.path().join("secrets"))
        .await
        .expect("secret store");
    let settings = Arc::new(RwLock::new(MediaSettings {
        media_ytdlp_executable: Some(ytdlp.to_string_lossy().into_owned()),
        media_ffmpeg_executable: Some(fixture("fake-yt-dlp").to_string_lossy().into_owned()),
        media_default_variant: "720p".to_owned(),
        ..MediaSettings::default()
    }));
    Harness {
        temp,
        database,
        secrets,
        settings,
        log,
        jar,
    }
}

/// How a profile differs from an active, unexpiring one for the whole site.
struct Profile<'a> {
    host: &'a str,
    cookie: &'a str,
    enabled: bool,
    expired: bool,
}

impl Harness {
    fn probe(&self) -> YtDlpProbe {
        YtDlpProbe::new(self.settings.clone())
            .with_auth_profiles(self.database.clone(), self.secrets.clone())
    }

    /// A cookie profile for `profile.host` holding `xs=<cookie>` for that host and a tracker's
    /// cookie that must never be handed over.
    async fn profile(&self, profile: Profile<'_>) -> AuthProfileId {
        let blob = format!(
            "# Netscape HTTP Cookie File\n\
             .{host}\tTRUE\t/\tTRUE\t0\txs\t{cookie}\n\
             .tracker.invalid\tTRUE\t/\tTRUE\t0\ttrack\tfollow-me\n",
            host = profile.host,
            cookie = profile.cookie,
        );
        let secret_ref = self
            .secrets
            .put(SecretString::from(blob))
            .await
            .expect("store cookies");
        let expires_at = profile
            .expired
            .then(|| chrono::Utc::now() - chrono::Duration::hours(1));
        self.database
            .create_auth_profile(NewAuthProfile {
                name: profile.host.to_owned(),
                scope: rd_core::AuthScope::parse(profile.host, true).expect("scope"),
                method: rd_core::AuthMethod::Cookies,
                origin: rd_core::AuthOrigin::Manual,
                enabled: profile.enabled,
                expires_at,
                username: None,
                secret_ref: Some(secret_ref),
                certificate_ref: None,
            })
            .await
            .expect("profile")
            .id
    }

    fn handed_over(&self) -> String {
        std::fs::read_to_string(&self.jar).unwrap_or_default()
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

/// An enabled, unexpiring profile.
const fn active<'a>(host: &'a str, cookie: &'a str) -> Profile<'a> {
    Profile {
        host,
        cookie,
        enabled: true,
        expired: false,
    }
}

fn reel() -> url::Url {
    REEL.parse().expect("url")
}

/// The check yt-dlp could not answer without a session: blocked with the sign-in hint.
fn assert_signin_required(failure: &rd_core::Failure) {
    assert_eq!(failure.code.as_deref(), Some("media.ytdlp_signin_required"));
    assert_eq!(failure.category, FailureKind::AuthRequired);
}

#[tokio::test]
async fn a_matching_cookie_profile_reaches_the_check_and_its_file_is_gone_afterwards() {
    let harness = harness().await;
    harness.profile(active("facebook.com", "session-one")).await;
    let entries = harness
        .probe()
        .probe(&reel(), AuthProfileSelection::Auto)
        .await
        .expect("the check passes with the session");
    assert_eq!(entries.len(), 1);

    let log = harness.log();
    let probe_line = log
        .lines()
        .find(|line| line.contains(" -J "))
        .expect("a -J run");
    let path = probe_line
        .split_whitespace()
        .skip_while(|arg| *arg != "--cookies")
        .nth(1)
        .expect("--cookies was passed");
    assert!(
        !Path::new(path).exists(),
        "the cookie file outlived the check"
    );
    assert!(!log.contains("session-one"), "a cookie value reached argv");
    let jar = harness.handed_over();
    assert!(jar.contains("session-one"), "{jar}");
    assert!(
        !jar.contains("follow-me"),
        "a foreign cookie was handed over"
    );
}

#[tokio::test]
async fn without_a_matching_profile_the_check_runs_without_cookies() {
    let harness = harness().await;
    harness.profile(active("youtube.com", "elsewhere")).await;
    let failure = harness
        .probe()
        .probe(&reel(), AuthProfileSelection::Auto)
        .await
        .expect_err("no session for Facebook");
    assert_signin_required(&failure);
    assert!(!harness.log().contains("--cookies"));
}

#[tokio::test]
async fn a_pinned_profile_is_taken_and_no_profile_prevents_the_match() {
    let harness = harness().await;
    // The more specific scope wins on automatic; pinning takes the other one.
    harness
        .profile(active("www.facebook.com", "specific"))
        .await;
    let pinned = harness.profile(active("facebook.com", "pinned")).await;
    harness
        .probe()
        .probe(&reel(), AuthProfileSelection::Pinned(pinned))
        .await
        .expect("pinned check");
    let jar = harness.handed_over();
    assert!(jar.contains("pinned") && !jar.contains("specific"), "{jar}");

    let failure = harness
        .probe()
        .probe(&reel(), AuthProfileSelection::None)
        .await
        .expect_err("no profile, no session");
    assert_signin_required(&failure);
    assert_eq!(harness.log().matches("--cookies").count(), 1);
}

#[tokio::test]
async fn a_disabled_or_expired_profile_is_not_taken() {
    let harness = harness().await;
    // Two scopes that both match the reel: one profile per scope is all the schema allows.
    harness
        .profile(Profile {
            enabled: false,
            ..active("facebook.com", "disabled")
        })
        .await;
    let expired = harness
        .profile(Profile {
            expired: true,
            ..active("www.facebook.com", "expired")
        })
        .await;
    let failure = harness
        .probe()
        .probe(&reel(), AuthProfileSelection::Auto)
        .await
        .expect_err("neither profile may be used");
    assert_signin_required(&failure);
    // Pinned, an expired profile is the user's mistake to learn about, not a silent fallback.
    let failure = harness
        .probe()
        .probe(&reel(), AuthProfileSelection::Pinned(expired))
        .await
        .expect_err("pinned and expired");
    assert_eq!(
        failure.code.as_deref(),
        Some("media.cookie_profile_unusable")
    );
    assert!(!harness.log().contains("--cookies"));
    assert!(harness.handed_over().is_empty());
}

/// RD-1320-02: a `Required` selection asks yt-dlp for the formats again before the download,
/// and that question carries the run's cookies — without them a login page fails there.
#[tokio::test]
async fn the_format_query_of_a_download_carries_the_runs_cookies() {
    let harness = harness().await;
    let profile = harness.profile(active("facebook.com", "for-the-run")).await;
    let destination = harness.temp.path().join("pkg");
    let package = harness
        .database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "reel".to_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let selection = rd_core::MediaSelection {
        page_url: reel(),
        variant_id: "best".to_owned(),
        format: "b".to_owned(),
        kind: MediaKind::Video,
        ext: "mp4".to_owned(),
        title: "reel".to_owned(),
        contract_version: rd_core::MEDIA_CONTRACT_VERSION,
        criteria: Some(Box::new(MediaFormatCriteria {
            strictness: MediaStrictness::Required,
            ..MediaFormatCriteria::default()
        })),
        resolved: None,
    };
    let file = harness
        .database
        .create_download(NewDownload {
            id: rd_core::DownloadId::new(),
            package_id: package.id,
            source: selection.page_url.clone(),
            file_name: "reel.mp4".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Pinned(profile),
            initial_state: DownloadState::Queued,
            kind: DownloadKind::Media,
            media: Some(selection),
            replay: None,
            remote_credential_id: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    let runner = MediaRunner::new(
        harness.database.clone(),
        harness.secrets.clone(),
        harness.settings.clone(),
    );
    let limits = rd_scheduler::RunLimits {
        max_parallel_requests: 1,
        bandwidth: rd_limits::ScopedLimiter::unlimited(),
        address_policy: None,
    };
    let outcome = runner
        .run(&file, &package, CancellationToken::new(), limits)
        .await
        .expect("run");
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "unexpected outcome {outcome:?}"
    );
    let log = harness.log();
    let format_query = log
        .lines()
        .find(|line| line.contains(" -J "))
        .expect("the format query ran");
    assert!(format_query.contains("--cookies"), "{format_query}");
}

/// RD-1320-01, end to end: the reel without a session is blocked with the sign-in hint, and
/// yt-dlp's request to report the error to it does not reach the interface.
#[tokio::test]
async fn a_reel_without_a_session_asks_for_one() {
    let harness = harness().await;
    let failure = harness
        .probe()
        .probe(&reel(), AuthProfileSelection::Auto)
        .await
        .expect_err("no session");
    assert_signin_required(&failure);
    assert_eq!(
        failure.params.get("site").map(String::as_str),
        Some("facebook")
    );
    let detail = failure.params.get("detail").expect("detail");
    assert!(detail.ends_with("Cannot parse data"), "{detail}");
    assert!(!failure.message.contains("report this issue"));
}
