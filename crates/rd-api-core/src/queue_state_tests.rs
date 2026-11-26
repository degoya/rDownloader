//! RD-1290-04: every pause source announces `paused` once, its end `resumed` once, a queue that
//! ran empty `finished` once, and a pause set again announces nothing.

use rd_core::{DownloadFigures, DownloadState};
use rd_scheduler::PauseReason;

use super::{QueueStateWatch, Sample};

fn file(state: DownloadState) -> DownloadFigures {
    DownloadFigures {
        id: rd_core::DownloadId::new(),
        state,
        total_bytes: None,
        committed_bytes: rd_core::ByteCount::default(),
    }
}

/// The same files in other states, as the next tick reads them.
fn now_in(files: &[DownloadFigures], state: DownloadState) -> Vec<DownloadFigures> {
    files
        .iter()
        .map(|file| DownloadFigures { state, ..*file })
        .collect()
}

/// One tick: the states announced, and the reason of a pause.
fn tick(
    watch: &mut QueueStateWatch,
    downloads: &[DownloadFigures],
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
    let running = vec![file(DownloadState::Downloading), held_back];
    tick(&mut watch, &running, None, false);
    let done = vec![
        DownloadFigures {
            state: DownloadState::Completed,
            ..running[0]
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
        DownloadFigures {
            state: DownloadState::Completed,
            ..running[0]
        },
        DownloadFigures {
            state: DownloadState::Paused,
            ..running[1]
        },
    ];
    assert_eq!(
        tick(&mut watch, &at_mark, Some(PauseReason::StopMark), false),
        ["paused:stop_mark"]
    );
    assert!(tick(&mut watch, &at_mark, Some(PauseReason::StopMark), false).is_empty());
    // Resumed, it runs the last file and then ran empty.
    let last = vec![
        at_mark[0],
        now_in(&at_mark[1..], DownloadState::Downloading)[0],
    ];
    assert_eq!(tick(&mut watch, &last, None, false), ["resumed"]);
    let done = now_in(&last, DownloadState::Completed);
    assert_eq!(tick(&mut watch, &done, None, false), ["finished"]);
}

/// RD-1430-06: a download queued, run and finished between two ticks was never busy in a
/// sample and announced nothing. The file that finished since the last tick is the trace it left:
/// one `finished`, and none on the next tick of the same queue.
#[test]
fn a_download_finished_before_any_tick_saw_it_is_finished_once() {
    let mut watch = QueueStateWatch::default();
    let earlier = file(DownloadState::Completed);
    assert!(tick(&mut watch, std::slice::from_ref(&earlier), None, false).is_empty());
    let quick = vec![earlier, file(DownloadState::Completed)];
    assert_eq!(tick(&mut watch, &quick, None, false), ["finished"]);
    assert!(tick(&mut watch, &quick, None, false).is_empty(), "once");

    // One that failed at once ends the queue's work too, as a failure after a seen tick does.
    let failed = vec![earlier, quick[1], file(DownloadState::Failed)];
    assert_eq!(tick(&mut watch, &failed, None, false), ["finished"]);

    // A paused file resumed and finished between ticks is work as well.
    let held = file(DownloadState::Paused);
    let mut queue = failed.clone();
    queue.push(held);
    assert!(tick(&mut watch, &queue, None, false).is_empty());
    let resumed = now_in(std::slice::from_ref(&held), DownloadState::Completed);
    let mut queue = failed;
    queue.extend(resumed);
    assert_eq!(tick(&mut watch, &queue, None, false), ["finished"]);
}

/// The first tick after a start only learns the state, finished files included; a quick file
/// that finished while the queue is paused, or while another still runs, finishes nothing.
#[test]
fn an_unseen_finish_is_no_moment_at_start_while_paused_or_while_busy() {
    let mut watch = QueueStateWatch::default();
    assert!(tick(&mut watch, &[file(DownloadState::Completed)], None, false).is_empty());

    let mut watch = QueueStateWatch::default();
    tick(&mut watch, &[], Some(PauseReason::Manual), false);
    let quick = vec![file(DownloadState::Completed)];
    assert!(tick(&mut watch, &quick, Some(PauseReason::Manual), false).is_empty());

    let mut watch = QueueStateWatch::default();
    let running = file(DownloadState::Downloading);
    tick(&mut watch, std::slice::from_ref(&running), None, false);
    let beside = vec![running, file(DownloadState::Completed)];
    assert!(
        tick(&mut watch, &beside, None, false).is_empty(),
        "still busy"
    );
    let done = now_in(&beside, DownloadState::Completed);
    assert_eq!(tick(&mut watch, &done, None, false), ["finished"]);
}
