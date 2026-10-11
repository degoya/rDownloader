use reqwest::StatusCode;

use super::{RestartChoice, RestartReading, choose, entry, pending, refused, started};
use crate::server_update::{Reading, view};
use crate::{client::ServiceRefusal, self_update::OfferEntry};

fn reading(pending: bool, can_restart: bool, may_install: bool) -> Reading {
    Reading {
        may_install,
        restart: RestartReading {
            pending,
            can_restart,
        },
        ..Reading::default()
    }
}

/// The entry shows only while a restart is pending and the agent may carry it out, greyed out
/// while it cannot begin; the server line hears of a pending restart either way.
#[test]
fn the_entry_appears_only_while_a_restart_is_pending_and_allowed() {
    let shown = OfferEntry {
        label: "Restart server".to_owned(),
        enabled: true,
    };
    assert_eq!(entry(None), None);
    assert_eq!(entry(Some(&reading(false, true, true))), None);
    assert_eq!(entry(Some(&reading(true, true, true))), Some(shown.clone()));
    assert_eq!(
        entry(Some(&reading(true, false, true))),
        Some(OfferEntry {
            enabled: false,
            ..shown
        })
    );
    assert_eq!(
        entry(Some(&reading(true, true, false))),
        None,
        "hidden without the right"
    );
    assert!(pending(Some(&reading(true, true, false))));
    let current = view(Some(&reading(true, true, true)), None);
    assert!(current.restart_pending && current.restart.is_some());
    assert!(!view(None, None).restart_pending);
}

/// A choice restarts only when pending, allowed and possible; otherwise it says why.
#[test]
fn the_choice_restarts_only_when_it_can() {
    assert_eq!(
        choose(Some(&reading(true, true, true))),
        RestartChoice::Restart
    );
    for refused in [
        reading(false, true, true),
        reading(true, false, true),
        reading(true, true, false),
    ] {
        assert!(matches!(choose(Some(&refused)), RestartChoice::Tell(_)));
    }
    assert!(matches!(choose(None), RestartChoice::Tell(_)));
}

/// What the notification says follows how the service comes back.
#[test]
fn the_notice_says_who_starts_the_service_again() {
    assert!(started("self").contains("back in a moment"));
    assert!(started("supervisor").contains("service manager"));
    assert!(started("manual").contains("start it again"));
}

/// Running downloads refuse once; choosing again within the window restarts anyway.
#[test]
fn running_downloads_ask_for_a_second_choice() {
    let error: anyhow::Error = ServiceRefusal::new(
        "server restart",
        StatusCode::CONFLICT,
        r#"{"code":"restart.transfers_active","params":{"count":"3"}}"#,
    )
    .into();
    let (text, confirm) = refused(&error);
    assert!(confirm);
    assert!(text.starts_with("3 downloads are running"), "{text}");
    let error: anyhow::Error = ServiceRefusal::new(
        "server restart",
        StatusCode::CONFLICT,
        r#"{"code":"restart.update_running"}"#,
    )
    .into();
    assert!(!refused(&error).1);
}
