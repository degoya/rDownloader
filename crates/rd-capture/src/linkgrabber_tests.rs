use reqwest::StatusCode;

use super::{Enqueued, notice};
use crate::client::ServiceRefusal;

fn answer(links: u32, nzbs: u32, duplicates: u32, failed: u32) -> Enqueued {
    Enqueued {
        links,
        nzbs,
        duplicates,
        failed,
        first_error: (failed > 0).then(|| "collector.package_busy".to_owned()),
    }
}

fn said(answer: Enqueued, paused: bool) -> String {
    notice(&Ok(answer), paused)
}

/// The notification counts what went, says when it went paused, and nothing else when nothing
/// else happened.
#[test]
fn what_was_added_is_counted() {
    assert_eq!(
        said(answer(1, 0, 0, 0), false),
        "1 link added to the downloads"
    );
    assert_eq!(
        said(answer(12, 0, 0, 0), true),
        "12 links added to the downloads, paused"
    );
    assert_eq!(
        said(answer(3, 2, 0, 0), false),
        "3 links and 2 NZBs added to the downloads"
    );
    assert_eq!(
        said(answer(0, 1, 0, 0), false),
        "1 NZB added to the downloads"
    );
}

/// An empty LinkGrabber is no failure, and says so.
#[test]
fn an_empty_linkgrabber_says_so() {
    assert_eq!(
        said(Enqueued::default(), false),
        "The LinkGrabber has nothing to add"
    );
}

/// What stayed behind is named after what went: duplicates, which the web interface asks
/// about, and failures by their code.
#[test]
fn duplicates_and_failures_follow_what_was_added() {
    assert_eq!(
        said(answer(4, 0, 2, 1), false),
        "4 links added to the downloads; 2 entries with links already added left in the \
         LinkGrabber; 1 failed (collector.package_busy)"
    );
    assert_eq!(
        said(answer(0, 0, 1, 0), true),
        "Nothing added from the LinkGrabber; 1 entry with links already added left in the \
         LinkGrabber"
    );
    let mut without_code = answer(0, 0, 0, 2);
    without_code.first_error = None;
    assert_eq!(
        said(without_code, false),
        "Nothing added from the LinkGrabber; 2 failed"
    );
}

/// A refusal is reported by its code; an agent paired without queue control is told how to get
/// it, as the greyed queue entries tell it.
#[test]
fn a_refusal_is_reported_by_its_code() {
    let refusal = |status: StatusCode, body: &str| -> anyhow::Error {
        ServiceRefusal::new("LinkGrabber enqueue", status, body).into()
    };
    assert_eq!(
        notice(
            &Err(refusal(
                StatusCode::FORBIDDEN,
                r#"{"error":"no","code":"auth.scope_insufficient","params":{"scope":"capture:queue"}}"#
            )),
            false
        ),
        "Pair the agent again to add from the LinkGrabber"
    );
    assert_eq!(
        notice(
            &Err(refusal(
                StatusCode::CONFLICT,
                r#"{"error":"busy","code":"collector.package_busy"}"#
            )),
            true
        ),
        "Nothing added from the LinkGrabber (collector.package_busy)"
    );
    assert_eq!(
        notice(&Err(refusal(StatusCode::BAD_GATEWAY, "")), false),
        "Nothing added from the LinkGrabber (HTTP 502)"
    );
    assert_eq!(
        notice(&Err(anyhow::anyhow!("connection refused")), false),
        "Nothing added from the LinkGrabber: rDownloader did not answer"
    );
}
