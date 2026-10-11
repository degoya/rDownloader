//! The agent's own update (RD-1210-03): only a manifest signed with the update key counts, never
//! a version older than the running one, the service's request only where the agent allows it.

use chrono::{DateTime, Duration, Utc};
use rd_update::agent::AgentSetup;
use rd_update::agent::report::SelfUpdate;
use rd_update::{
    Artifact, Channel, InstallKind, MemoryFetcher, SigningKey, Sources, TrustStore, UpdateManifest,
    manifest::{self, UPDATE_MANIFEST_SCHEMA_VERSION},
};

use super::check::check_with;
use super::*;

const KEY_ID: &str = "rdownloader-update-v1";
const PORTABLE: AgentSetup = AgentSetup::Alone(InstallKind::Portable);

fn key() -> SigningKey {
    SigningKey::from_bytes(&[11; 32])
}

fn trust() -> TrustStore {
    let trust = TrustStore::new();
    trust
        .trust(KEY_ID.to_owned(), key().verifying_key())
        .expect("trust");
    trust
}

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(1_790_000_000, 0).expect("timestamp")
}

/// A release whose manifest carries the agent's archive for the platform the test runs on.
fn release(version: &str) -> UpdateManifest {
    let target = rd_update::Target::current(InstallKind::Portable);
    UpdateManifest {
        schema_version: UPDATE_MANIFEST_SCHEMA_VERSION,
        sequence: 10,
        issued_at: now() - Duration::days(1),
        not_after: now() + Duration::days(90),
        channel: Channel::Stable,
        version: version.to_owned(),
        released_at: now() - Duration::days(1),
        notes: "- A thing".to_owned(),
        changelog_anchor: None,
        artifacts: Vec::new(),
        schema_change: None,
        agent_artifacts: vec![Artifact {
            platform: target.platform.to_owned(),
            arch: target.arch.to_owned(),
            kind: target.kind.to_owned(),
            url: format!(
                "https://github.com/degoya/rDownloader/releases/download/v{version}/rdownloader-capture-{}-{}.tar.gz",
                target.platform, target.arch
            ),
            sha256: "ab".repeat(32),
            size: 2048,
        }],
    }
}

async fn checked(signed_by: &SigningKey, version: &str, running: &str) -> State {
    let sources = Sources::official();
    let fetcher = MemoryFetcher::new();
    fetcher.serve(
        sources.stable.as_str(),
        manifest::sign(KEY_ID, signed_by, &release(version)).expect("sign"),
    );
    let mut state = State::default();
    check_with(
        &fetcher,
        (&sources, &trust()),
        Channel::Stable,
        &mut state,
        (running, now()),
    )
    .await;
    state
}

#[tokio::test]
async fn a_newer_signed_release_is_offered_with_the_agents_archive() {
    let state = checked(&key(), "1.21.0", "1.20.0").await;
    assert_eq!(state.last_error, None);
    let offer = state.current_offer("1.20.0").expect("offered");
    assert_eq!(offer.version, "1.21.0");
    assert!(
        offer
            .artifact
            .as_ref()
            .is_some_and(|artifact| artifact.url.contains("rdownloader-capture-"))
    );
    assert_eq!(
        entry_for(PORTABLE, offer).map(|entry| (entry.label, entry.enabled)),
        Some(("Install update to 1.21.0".to_owned(), true))
    );
}

/// A manifest signed by any other key is refused: nothing is offered, the report says it failed.
#[tokio::test]
async fn a_manifest_with_a_wrong_signature_offers_nothing() {
    let state = checked(&SigningKey::from_bytes(&[12; 32]), "1.21.0", "1.20.0").await;
    assert_eq!(state.offer, None);
    assert_eq!(state.last_error.as_deref(), Some("update.bad_signature"));
    let report = report_of(PORTABLE, &Config::default(), &state, "1.20.0", None);
    assert_eq!(report.state, SelfUpdate::Failed);
    assert_eq!(report.offered, None);
}

/// Never a version older than the running one, and not the running one itself.
#[tokio::test]
async fn an_older_or_the_same_version_is_never_offered() {
    for (version, running) in [("1.19.0", "1.20.0"), ("1.20.0", "1.20.0")] {
        let state = checked(&key(), version, running).await;
        assert_eq!(state.last_error, None, "{version}");
        assert!(
            state.current_offer(running).is_none(),
            "{version} over {running}"
        );
        assert_eq!(
            report_of(PORTABLE, &Config::default(), &state, running, None).state,
            SelfUpdate::Current
        );
    }
    // An offer kept from before an update by hand is history once that version runs.
    let state = checked(&key(), "1.21.0", "1.20.0").await;
    assert!(state.current_offer("1.21.0").is_none());
}

#[test]
fn beside_the_service_or_switched_off_the_agent_reports_so_and_offers_nothing() {
    let state = offered("1.21.0");
    let beside = report_of(
        AgentSetup::WithService,
        &Config::default(),
        &state,
        "1.20.0",
        None,
    );
    assert_eq!(beside.state, SelfUpdate::WithService);
    assert_eq!(beside.offered, None);
    let offer = state.offer.clone().expect("offer");
    assert_eq!(entry_for(AgentSetup::WithService, &offer), None);
    let off = Config {
        check: false,
        allow_remote: false,
        auto_install: false,
    };
    assert_eq!(
        report_of(PORTABLE, &off, &state, "1.20.0", None).state,
        SelfUpdate::Disabled
    );
    // Without the archive for this platform the entry names the download page and installs
    // nothing.
    assert_eq!(
        entry_for(PORTABLE, &offer).map(|entry| entry.enabled),
        Some(false)
    );
}

/// No service installs software here unless this agent's own configuration allows it, and then
/// only the version this agent found newer itself.
#[test]
fn the_services_request_needs_the_agents_consent() {
    let state = offered("1.21.0");
    let default = Config::default();
    assert!(!default.allow_remote, "off by default");
    assert!(remote_decision(&default, &state, "1.20.0", "1.21.0").is_err());
    let allowed = Config {
        check: true,
        allow_remote: true,
        auto_install: false,
    };
    assert!(remote_decision(&allowed, &state, "1.20.0", "1.21.0").is_ok());
    assert!(remote_decision(&allowed, &state, "1.20.0", "1.22.0").is_err());
    assert!(
        remote_decision(&allowed, &state, "1.21.0", "1.21.0").is_err(),
        "nothing newer is offered any more"
    );
}

#[test]
fn the_service_answer_names_the_channel_and_a_request_once() {
    use reqwest::header::{HeaderMap, HeaderValue};
    let shared = Shared::default();
    assert_eq!(shared.channel(), None);
    let mut answer = HeaderMap::new();
    answer.insert(
        rd_update::agent::report::CHANNEL_HEADER,
        HeaderValue::from_static("beta"),
    );
    answer.insert(
        rd_update::agent::report::REQUEST_HEADER,
        HeaderValue::from_static("1.21.0"),
    );
    shared.learn(&answer);
    assert_eq!(shared.channel(), Some(Channel::Beta));
    assert_eq!(shared.take_request().as_deref(), Some("1.21.0"));
    assert_eq!(shared.take_request(), None, "once");
    shared.learn(&HeaderMap::new());
    assert_eq!(
        shared.channel(),
        Some(Channel::Beta),
        "kept until named again"
    );
}

#[test]
fn the_switches_and_the_state_survive_a_restart() {
    let directory = tempfile_dir("survive");
    assert_eq!(Config::load(&directory), Config::default());
    let switched = Config {
        check: false,
        allow_remote: true,
        auto_install: true,
    };
    switched.store(&directory).expect("store");
    assert_eq!(Config::load(&directory), switched);
    std::fs::write(directory.join(CONFIG_FILE), b"{ broken").expect("break");
    assert_eq!(Config::load(&directory), Config::default());
    let state = State {
        service_channel: Some(Channel::Beta),
        ..State::default()
    };
    state.store(&directory).expect("store");
    assert_eq!(State::load(&directory).service_channel, Some(Channel::Beta));
    let _ = std::fs::remove_dir_all(&directory);
}

/// RD-1240-27: off by default; switched on, a portable agent alone installs what its check found,
/// with the archive. Beside the service the switch does not apply and the tray does not show it.
#[test]
fn the_automatic_install_applies_to_a_portable_agent_alone() {
    use super::auto::{AutoInstallEntry, auto_install_entry, installs_now, toggle};

    let mut settings = Config::default();
    assert!(!settings.auto_install, "off by default");
    let mut state = offered("1.21.0");
    let without_archive = state.offer.clone();
    if let Some(offer) = state.offer.as_mut() {
        offer.artifact = Some(Artifact {
            platform: "windows".to_owned(),
            arch: "x86_64".to_owned(),
            kind: "archive".to_owned(),
            url: "https://example.test/agent.zip".to_owned(),
            sha256: "ab".repeat(32),
            size: 1,
        });
    }
    let offer = state.current_offer("1.20.0");
    assert!(!installs_now(PORTABLE, &settings, offer));

    assert_eq!(toggle(PORTABLE, &mut settings), Ok(true));
    assert!(installs_now(PORTABLE, &settings, offer));
    assert!(!installs_now(PORTABLE, &settings, without_archive.as_ref()));
    assert!(!installs_now(PORTABLE, &settings, None));
    assert_eq!(
        auto_install_entry(PORTABLE, &settings),
        Some(AutoInstallEntry {
            enabled: true,
            checked: true
        })
    );

    assert!(toggle(AgentSetup::WithService, &mut settings).is_err());
    assert_eq!(auto_install_entry(AgentSetup::WithService, &settings), None);
    assert!(!installs_now(AgentSetup::WithService, &settings, offer));

    let homebrew = AgentSetup::Alone(InstallKind::Homebrew);
    assert!(toggle(homebrew, &mut settings).is_err());
    assert!(settings.auto_install, "a refused switch changes nothing");
    assert!(!installs_now(homebrew, &settings, offer));
    assert_eq!(
        auto_install_entry(homebrew, &settings),
        Some(AutoInstallEntry {
            enabled: false,
            checked: false
        })
    );

    assert_eq!(toggle(PORTABLE, &mut settings), Ok(false));
}

/// "Install update" pressed obeys the entry (RD-1240-24): it installs only what the entry would,
/// and otherwise names why, as the entry's text does.
#[tokio::test]
async fn the_update_shortcut_installs_only_what_the_entry_would() {
    let state = checked(&key(), "1.21.0", "1.20.0").await;
    assert_eq!(
        install_refusal(PORTABLE, state.current_offer("1.20.0")),
        None
    );
    assert_eq!(
        install_refusal(PORTABLE, None).as_deref(),
        Some("No update of rDownloader Capture is offered")
    );
    let without_archive = offered("1.21.0");
    assert_eq!(
        install_refusal(PORTABLE, without_archive.offer.as_ref()).as_deref(),
        Some("Update to 1.21.0 is on the download page")
    );
    assert!(install_refusal(AgentSetup::WithService, state.current_offer("1.20.0")).is_some());
}

/// A state that offers `version`, without an archive.
fn offered(version: &str) -> State {
    State {
        offer: Some(rd_update::Offer {
            version: version.to_owned(),
            channel: Channel::Stable,
            released_at: now(),
            notes: String::new(),
            changelog_anchor: None,
            artifact: None,
            schema_change: false,
        }),
        ..State::default()
    }
}

fn tempfile_dir(name: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "rd-capture-self-update-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    directory
}
