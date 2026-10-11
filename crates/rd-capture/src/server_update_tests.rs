use chrono::{Duration, TimeZone, Utc};
use reqwest::StatusCode;
use url::Url;

use super::{
    Choice, Installing, Offered, Pending, Reading, Resolution, choose, refused, resolve, started,
    update_page, view,
};
use crate::{client::ServiceRefusal, self_update::OfferEntry};

fn offered(action: &str, may_install: bool) -> Reading {
    Reading {
        available: Some(Offered {
            version: "1.25.0".to_owned(),
            action: action.to_owned(),
            command: (action == "command").then(|| "sudo apt install rdownloader".to_owned()),
        }),
        may_install,
        install: None,
        ..Reading::default()
    }
}

fn installing(state: &str, reason: Option<&str>) -> Installing {
    Installing {
        state: state.to_owned(),
        target_version: "1.25.0".to_owned(),
        reason: reason.map(str::to_owned),
    }
}

fn pending_since(requested_at: chrono::DateTime<Utc>) -> Pending {
    Pending {
        target: "1.25.0".to_owned(),
        requested_at,
    }
}

fn refusal(status: StatusCode, body: &str) -> anyhow::Error {
    ServiceRefusal::new("server update install", status, body).into()
}

/// The entry shows only while there is an update, with its version; the label is the same with
/// and without the right, since without it the click opens the update page.
#[test]
fn the_entry_appears_only_for_an_offered_update_with_its_version() {
    assert_eq!(view(None, None).entry, None);
    assert_eq!(view(Some(&Reading::default()), None).entry, None);
    for may_install in [true, false] {
        assert_eq!(
            view(Some(&offered("install", may_install)), None).entry,
            Some(OfferEntry {
                label: "Install server update 1.25.0".to_owned(),
                enabled: true,
            })
        );
    }
}

/// A package manager's or a container's installation shows its command, greyed out: a hint,
/// never an install.
#[test]
fn an_installation_that_does_not_install_itself_shows_a_hint() {
    let reading = offered("command", true);
    assert_eq!(
        view(Some(&reading), None).entry,
        Some(OfferEntry {
            label: "Server update 1.25.0: sudo apt install rdownloader".to_owned(),
            enabled: false,
        })
    );
    assert_eq!(
        choose(Some(&reading), None),
        Choice::Tell("Server update 1.25.0: sudo apt install rdownloader".to_owned())
    );
}

/// With the right the choice installs; without it, or for an update downloaded by hand, it opens
/// the update page.
#[test]
fn the_choice_installs_only_with_the_right() {
    assert_eq!(
        choose(Some(&offered("install", true)), None),
        Choice::Install
    );
    assert_eq!(
        choose(Some(&offered("install", false)), None),
        Choice::OpenPage
    );
    assert_eq!(
        choose(Some(&offered("download", true)), None),
        Choice::OpenPage
    );
    assert!(matches!(
        choose(Some(&Reading::default()), None),
        Choice::Tell(_)
    ));
    assert!(matches!(choose(None, None), Choice::Tell(_)));
}

/// While an install runs -- reported by the service, or started here and not heard the end of
/// while the service restarts -- the entry says so, the server line names the version, and a
/// second choice installs nothing.
#[test]
fn while_it_installs_the_entry_and_the_server_line_say_so() {
    let mut reading = offered("install", true);
    reading.install = Some(installing("restarting", None));
    let shown = view(Some(&reading), None);
    assert_eq!(shown.updating.as_deref(), Some("1.25.0"));
    assert_eq!(
        shown.entry,
        Some(OfferEntry {
            label: "Installing server update 1.25.0...".to_owned(),
            enabled: false,
        })
    );
    assert!(matches!(choose(Some(&reading), None), Choice::Tell(_)));

    let pending = pending_since(Utc::now());
    let away = view(Some(&offered("install", true)), Some(&pending));
    assert_eq!(away.updating.as_deref(), Some("1.25.0"));

    reading.install = Some(installing("done", None));
    assert_eq!(view(Some(&reading), None).updating, None);
}

/// The outcome of the install this agent started is announced once the service reports its end
/// for that version; an outcome of another version is not this one's.
#[test]
fn the_outcome_is_announced_for_the_version_this_agent_started() {
    let now = Utc
        .with_ymd_and_hms(2026, 10, 10, 12, 0, 0)
        .single()
        .expect("time");
    let pending = pending_since(now);
    let mut reading = offered("install", true);
    assert_eq!(resolve(&pending, Some(&reading), now), Resolution::Waiting);
    reading.install = Some(installing("restarting", None));
    assert_eq!(resolve(&pending, Some(&reading), now), Resolution::Waiting);

    reading.install = Some(installing("done", None));
    assert_eq!(
        resolve(&pending, Some(&reading), now),
        Resolution::Ended("rDownloader was updated to 1.25.0".to_owned())
    );
    reading.install = Some(installing("failed", Some("update.digest_mismatch")));
    assert_eq!(
        resolve(&pending, Some(&reading), now),
        Resolution::Ended(
            "The server update to 1.25.0 failed (update.digest_mismatch); the installed version \
             keeps running"
                .to_owned()
        )
    );
    reading.install = Some(installing("rolled_back", Some("update.health_timeout")));
    assert!(matches!(
        resolve(&pending, Some(&reading), now),
        Resolution::Ended(text) if text.contains("taken back (update.health_timeout)")
    ));

    let mut other = installing("done", None);
    other.target_version = "1.24.1".to_owned();
    reading.install = Some(other);
    assert_eq!(resolve(&pending, Some(&reading), now), Resolution::Waiting);
    assert!(matches!(
        resolve(&pending, Some(&reading), now + Duration::minutes(31)),
        Resolution::Ended(text) if text.starts_with("No word from the server update to 1.25.0")
    ));
}

#[test]
fn the_start_is_announced_with_the_restart() {
    assert_eq!(
        started("1.25.0"),
        "Installing server update 1.25.0; rDownloader restarts and is back in a moment"
    );
}

/// A refusal is said by its code; running downloads arm a second choice that installs anyway.
#[test]
fn a_refused_install_is_said_by_its_code() {
    let (text, confirm) = refused(&refusal(
        StatusCode::CONFLICT,
        r#"{"error":"busy","code":"update.transfers_active","params":{"count":3}}"#,
    ));
    assert!(confirm);
    assert!(text.starts_with("3 downloads are running."), "{text}");

    let (text, confirm) = refused(&refusal(
        StatusCode::FORBIDDEN,
        r#"{"error":"no","code":"auth.scope_insufficient","params":{"scope":"capture:server_update"}}"#,
    ));
    assert!(!confirm);
    assert!(text.contains("May install server updates"), "{text}");

    let (text, _) = refused(&refusal(
        StatusCode::CONFLICT,
        r#"{"error":"no","code":"update.install_unsupported"}"#,
    ));
    assert_eq!(
        text,
        "The server update was not started (update.install_unsupported)"
    );
    let (text, confirm) = refused(&anyhow::anyhow!("connection refused"));
    assert!(!confirm);
    assert!(text.ends_with("rDownloader did not answer"), "{text}");
}

#[test]
fn the_update_page_is_the_system_settings_updates_tab() {
    let service = Url::parse("http://nas.local:8710/rd/").expect("url");
    assert_eq!(
        update_page(&service).map(String::from).as_deref(),
        Some("http://nas.local:8710/rd/settings/system?tab=updates")
    );
}

/// The service's answer reads as the tray needs it, fields it does not know ignored.
#[test]
fn the_reading_parses_the_service_answer() {
    let reading: Reading = serde_json::from_str(
        r#"{"available":{"version":"1.25.0","action":"install","command":null},"may_install":true,"install":{"state":"preparing","target_version":"1.25.0","reason":null},"later":1}"#,
    )
    .expect("reads");
    assert!(reading.may_install);
    assert_eq!(
        reading.install.map(|install| install.state).as_deref(),
        Some("preparing")
    );
    let pending = pending_since(Utc::now());
    let stored = serde_json::to_vec(&pending).expect("writes");
    assert_eq!(
        serde_json::from_slice::<Pending>(&stored).expect("reads back"),
        pending
    );
}
