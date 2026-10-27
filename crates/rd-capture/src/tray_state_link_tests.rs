//! What the tray shows about the link (RD-1390-02, `tray_state_link.rs`).

use url::Url;

use super::{
    IconKind, LinkEntry, TrayState,
    link::{CHANGE_LINK, LINK, NOT_LINKED, UNENCRYPTED},
    tests::apply,
};
use crate::{
    activity::{Activity, QueueEntries, QueueMenu},
    link_phase::LinkStatus,
    status::{ServerStatus, agent_label},
};

fn address(text: &str) -> Url {
    Url::parse(text).expect("valid URL")
}

fn linked(service: &str, insecure: bool) -> LinkStatus {
    LinkStatus::Linked {
        service: address(service),
        insecure,
    }
}

#[test]
fn an_unlinked_tray_offers_only_link_about_and_quit() {
    let mut state = TrayState::new(None);
    let mut shown = state.surface();
    let update = state.on_link(&LinkStatus::Unlinked);
    apply(&mut shown, &update);
    assert_eq!(shown, state.surface(), "the update is the whole change");

    let surface = state.surface();
    assert_eq!(
        surface.link,
        LinkEntry {
            label: LINK,
            linked: false
        }
    );
    assert!(
        surface.status_line.ends_with(NOT_LINKED),
        "{}",
        surface.status_line
    );
    assert_eq!(surface.icon, IconKind::Unlinked, "quiet, not an error");
    assert_eq!(surface.queue, QueueMenu::Hidden);
    assert!(!surface.game_mode.enabled());
    assert!(!surface.open_enabled);

    // A service that answers at the default address changes none of it.
    state.on_server_status(ServerStatus::Running, Some("1.39.0".to_owned()));
    assert!(
        !state.surface().open_enabled,
        "nothing to open without a link"
    );
    assert!(!state.surface().link.linked);
}

#[test]
fn an_unencrypted_link_says_so_in_the_server_line() {
    let mut state = TrayState::new(None);
    state.on_link(&LinkStatus::Unlinked);
    let update = state.on_link(&linked("http://192.168.1.10:8710/", true));
    assert!(
        update
            .server_line
            .as_deref()
            .is_some_and(|line| line.ends_with(UNENCRYPTED))
    );
    state.on_server_status(ServerStatus::Running, Some("1.39.0".to_owned()));
    let line = state.surface().server_line;
    assert_eq!(
        line,
        format!("Server v1.39.0 \u{2014} running {UNENCRYPTED}")
    );

    let mut safe = TrayState::new(None);
    safe.on_link(&linked("https://nas.example/", false));
    assert!(!safe.surface().server_line.contains(UNENCRYPTED));
}

#[test]
fn the_link_entry_is_relabelled_once_linked() {
    let service = "http://127.0.0.1:8710/";
    let mut state = TrayState::new(None);
    state.on_link(&LinkStatus::Unlinked);
    let mut shown = state.surface();
    let update = state.on_link(&linked(service, false));
    assert_eq!(
        update.link,
        Some(LinkEntry {
            label: CHANGE_LINK,
            linked: true
        })
    );
    assert_eq!(update.icon, Some(IconKind::Idle));
    apply(&mut shown, &update);
    assert_eq!(shown, state.surface());
    assert_eq!(
        state.surface().status_line,
        agent_label(Some(&address(service)))
    );
    // The same link again is nothing new.
    assert!(state.on_link(&linked(service, false)).is_empty());
}

#[test]
fn a_link_to_another_service_starts_its_half_over() {
    let mut state = TrayState::new(Some(address("http://127.0.0.1:8710/")));
    state.on_server_status(ServerStatus::Running, Some("1.38.0".to_owned()));
    state.on_transfers(Activity {
        running: true,
        detail: "1 active".to_owned(),
        queue: QueueMenu::Offered(QueueEntries {
            start: false,
            pause: true,
            timed_pause: true,
        }),
    });
    let update = state.on_link(&linked("https://nas.example/", false));
    assert_eq!(
        update.open_enabled,
        Some(false),
        "the new one has not answered"
    );
    assert_eq!(update.queue, Some(QueueMenu::Hidden));
    assert_eq!(state.surface().server_line, "Server \u{2014} starting");
    assert_eq!(state.surface().icon, IconKind::Idle);
}
