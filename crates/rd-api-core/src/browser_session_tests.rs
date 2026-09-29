//! The request registry of RD-120-45: who may answer what, and for how long.

use chrono::{Duration, TimeZone, Utc};

use super::{BrowserSessionState, BrowserSessions, KEEP, Refusal, Request, WAIT};

fn start() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 23, 20, 0, 0)
        .single()
        .expect("time")
}

fn request(account: rd_core::AccountId) -> Request {
    Request {
        account_id: account,
        account_label: "Main".to_owned(),
        provider: "ddownload".to_owned(),
        provider_name: "DDownload".to_owned(),
        scope: "https://ddownload.com/".parse().expect("scope"),
    }
}

#[test]
fn a_request_is_listed_for_the_extension_with_the_scope_the_service_chose() {
    let sessions = BrowserSessions::default();
    let account = rd_core::AccountId::new();
    let begun = sessions.begin(request(account), start());

    assert_eq!(begun.state, BrowserSessionState::Waiting);
    assert_eq!(begun.host, "ddownload.com");
    let waiting = sessions.waiting(start());
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].id, begun.id);
    assert_eq!(waiting[0].scope, "https://ddownload.com/");
    assert_eq!(waiting[0].host, "ddownload.com");
    assert_eq!(waiting[0].account_label, "Main");
}

#[test]
fn a_request_is_answered_once_and_then_reads_as_delivered() {
    let sessions = BrowserSessions::default();
    let account = rd_core::AccountId::new();
    let begun = sessions.begin(request(account), start());

    let claimed = sessions.claim(begun.id, start()).expect("claimed");
    assert_eq!(claimed.account_id, account);
    assert!(
        sessions.waiting(start()).is_empty(),
        "a request being stored is not offered again"
    );
    assert_eq!(
        sessions.claim(begun.id, start()).err(),
        Some(Refusal::NotWaiting),
        "a second delivery cannot race the first"
    );
    sessions.finish(begun.id, true);

    assert_eq!(
        sessions.status(account, start()).expect("status").state,
        BrowserSessionState::Delivered
    );
    assert_eq!(
        sessions.claim(begun.id, start()).err(),
        Some(Refusal::NotWaiting)
    );
}

#[test]
fn a_refused_delivery_leaves_the_request_waiting_for_another_try() {
    let sessions = BrowserSessions::default();
    let account = rd_core::AccountId::new();
    let begun = sessions.begin(request(account), start());

    sessions.claim(begun.id, start()).expect("claimed");
    sessions.finish(begun.id, false);

    assert_eq!(sessions.waiting(start()).len(), 1);
    assert!(sessions.claim(begun.id, start()).is_ok());
}

#[test]
fn an_unanswered_request_expires_and_cannot_be_answered_late() {
    let sessions = BrowserSessions::default();
    let account = rd_core::AccountId::new();
    let begun = sessions.begin(request(account), start());
    let late = start() + WAIT;

    assert!(sessions.waiting(late).is_empty());
    assert_eq!(
        sessions.claim(begun.id, late).err(),
        Some(Refusal::NotWaiting)
    );
    assert_eq!(sessions.decline(begun.id, late), Err(Refusal::NotWaiting));
    assert_eq!(
        sessions.status(account, late).expect("status").state,
        BrowserSessionState::Expired
    );
    assert!(
        sessions
            .status(account, start() + KEEP + Duration::seconds(1))
            .is_none(),
        "an old request is forgotten"
    );
}

#[test]
fn a_declined_request_says_so_and_asking_again_replaces_it() {
    let sessions = BrowserSessions::default();
    let account = rd_core::AccountId::new();
    let first = sessions.begin(request(account), start());

    sessions.decline(first.id, start()).expect("declined");
    assert_eq!(
        sessions.status(account, start()).expect("status").state,
        BrowserSessionState::Declined
    );

    let second = sessions.begin(request(account), start());
    assert_ne!(first.id, second.id);
    assert_eq!(
        sessions.waiting(start()).len(),
        1,
        "one request per account"
    );
    assert_eq!(
        sessions.claim(first.id, start()).err(),
        Some(Refusal::NotWaiting)
    );
    assert!(sessions.cancel(account));
    assert!(sessions.waiting(start()).is_empty());
    assert!(!sessions.cancel(account));
}
