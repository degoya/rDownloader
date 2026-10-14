//! RD-1290-04: every pause source announces `paused` once, its end `resumed` once, a queue that
//! ran empty `finished` once, and a pause set again announces nothing.

use chrono::Utc;
use rd_core::{DownloadFile, DownloadState};
use rd_scheduler::PauseReason;

use super::{QueueStateWatch, Sample};

fn file(state: DownloadState) -> DownloadFile {
    let now = Utc::now();
    DownloadFile {
        recording: None,
        id: rd_core::DownloadId::new(),
        package_id: rd_core::PackageId::new(),
        source: "https://hoster.example/a".parse().expect("url"),
        file_name: "a.bin".to_owned(),
        state,
        total_bytes: None,
        committed_bytes: rd_core::ByteCount::default(),
        retry_count: 0,
        next_retry_at: None,
        expected_checksum: None,
        computed_checksum: None,
        last_error: None,
        account_id: None,
        proxy_profile_id: None,
        remote_credential_id: None,
        mirror_group: None,
        auth_profile: rd_core::AuthProfileSelection::Auto,
        position: 0,
        kind: rd_core::DownloadKind::Http,
        nzb_file_id: None,
        recovery: false,
        media: None,
        enrichment: Vec::new(),
        created_at: now,
        updated_at: now,
    }
}

/// The same files in other states, as the next tick reads them.
fn now_in(files: &[DownloadFile], state: DownloadState) -> Vec<DownloadFile> {
    files
        .iter()
        .map(|file| DownloadFile {
            state,
            ..file.clone()
        })
        .collect()
}

/// One tick: the states announced, and the reason of a pause.
fn tick(
    watch: &mut QueueStateWatch,
    downloads: &[DownloadFile],
    queue_pause: Option<PauseReason>,
    schedule_pauses: bool,
) -> Vec<String> {
    watch
        .observe(&Sample {
            queue_pause,
            schedule_pauses,
            downloads,
            postprocessing: false,
        })
        .into_iter()
        .map(|change| match change["reason"].as_str() {
            Some(reason) => format!("{}:{reason}", change["state"].as_str().unwrap_or("?")),
            None => change["state"].as_str().unwrap_or("?").to_owned(),
        })
        .collect()
}

#[test]
fn every_queue_pause_is_announced_once_with_who_set_it() {
    for (reason, code) in [
        (PauseReason::Manual, "manual"),
        (PauseReason::Automation, "automation"),
        (PauseReason::StopMark, "stop_mark"),
        (PauseReason::GameMode, "game_mode"),
    ] {
        let mut watch = QueueStateWatch::default();
        let running = vec![file(DownloadState::Downloading)];
        assert!(
            tick(&mut watch, &running, None, false).is_empty(),
            "a start only learns"
        );
        let paused = now_in(&running, DownloadState::Paused);
        assert_eq!(
            tick(&mut watch, &paused, Some(reason), false),
            [format!("paused:{code}")]
        );
        // Set again, or by somebody else on top: still the same pause.
        for again in [Some(reason), Some(PauseReason::Manual), Some(reason)] {
            assert!(tick(&mut watch, &paused, again, false).is_empty(), "{code}");
        }
        let queued = now_in(&running, DownloadState::Queued);
        assert_eq!(
            tick(&mut watch, &queued, None, false),
            ["resumed"],
            "{code}"
        );
        assert!(tick(&mut watch, &queued, None, false).is_empty(), "{code}");
    }
}

#[test]
fn the_schedule_s_pause_is_a_pause_and_a_queue_pause_on_top_is_none() {
    let mut watch = QueueStateWatch::default();
    let queued = vec![file(DownloadState::Queued), file(DownloadState::Queued)];
    tick(&mut watch, &queued, None, false);
    assert_eq!(tick(&mut watch, &queued, None, true), ["paused:schedule"]);
    assert!(tick(&mut watch, &queued, None, true).is_empty());
    let paused = now_in(&queued, DownloadState::Paused);
    assert!(tick(&mut watch, &paused, Some(PauseReason::Manual), true).is_empty());
    // The schedule ends first; the person's pause still holds the queue.
    assert!(tick(&mut watch, &paused, Some(PauseReason::Manual), false).is_empty());
    assert_eq!(tick(&mut watch, &queued, None, false), ["resumed"]);
}

#[test]
fn pause_all_by_hand_is_a_pause_until_something_moves_again() {
    let mut watch = QueueStateWatch::default();
    let running = vec![
        file(DownloadState::Downloading),
        file(DownloadState::Queued),
    ];
    tick(&mut watch, &running, None, false);
    let paused = now_in(&running, DownloadState::Paused);
    assert_eq!(tick(&mut watch, &paused, None, false), ["paused:manual"]);
    assert!(
        tick(&mut watch, &paused, None, false).is_empty(),
        "no flapping"
    );
    let resumed = now_in(&running, DownloadState::Queued);
    assert_eq!(tick(&mut watch, &resumed, None, false), ["resumed"]);
}

#[test]
fn a_queue_that_ran_empty_is_finished_once() {
    let mut watch = QueueStateWatch::default();
    // A file paused long before is no pause of the queue: around it, the queue ran empty.
    let held_back = file(DownloadState::Paused);
    let running = vec![file(DownloadState::Downloading), held_back.clone()];
    tick(&mut watch, &running, None, false);
    let done = vec![
        DownloadFile {
            state: DownloadState::Completed,
            ..running[0].clone()
        },
        held_back,
    ];
    assert_eq!(tick(&mut watch, &done, None, false), ["finished"]);
    assert!(tick(&mut watch, &done, None, false).is_empty(), "once");
    assert!(tick(&mut watch, &[], None, false).is_empty(), "still empty");
}

#[test]
fn post_processing_keeps_the_queue_busy() {
    let mut watch = QueueStateWatch::default();
    let running = vec![file(DownloadState::Downloading)];
    tick(&mut watch, &running, None, false);
    let done = now_in(&running, DownloadState::Completed);
    let unpacking = Sample {
        queue_pause: None,
        schedule_pauses: false,
        downloads: &done,
        postprocessing: true,
    };
    assert!(watch.observe(&unpacking).is_empty(), "still unpacking");
    assert_eq!(tick(&mut watch, &done, None, false), ["finished"]);
}

#[test]
fn a_paused_queue_never_finishes() {
    let mut watch = QueueStateWatch::default();
    let running = vec![
        file(DownloadState::Downloading),
        file(DownloadState::Queued),
    ];
    tick(&mut watch, &running, None, false);
    // The stop mark: the waiting file pauses, the running one finishes.
    let at_mark = vec![
        DownloadFile {
            state: DownloadState::Completed,
            ..running[0].clone()
        },
        DownloadFile {
            state: DownloadState::Paused,
            ..running[1].clone()
        },
    ];
    assert_eq!(
        tick(&mut watch, &at_mark, Some(PauseReason::StopMark), false),
        ["paused:stop_mark"]
    );
    assert!(tick(&mut watch, &at_mark, Some(PauseReason::StopMark), false).is_empty());
    // Resumed, it runs the last file and then ran empty.
    let last = vec![
        at_mark[0].clone(),
        now_in(&at_mark[1..], DownloadState::Downloading)[0].clone(),
    ];
    assert_eq!(tick(&mut watch, &last, None, false), ["resumed"]);
    let done = now_in(&last, DownloadState::Completed);
    assert_eq!(tick(&mut watch, &done, None, false), ["finished"]);
}
