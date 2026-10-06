use super::LinkCandidateState;

#[test]
fn only_links_being_worked_on_are_refused() {
    for state in [
        LinkCandidateState::Online,
        LinkCandidateState::Duplicate,
        LinkCandidateState::Offline,
        LinkCandidateState::Unsupported,
        LinkCandidateState::Error,
    ] {
        assert!(state.is_enqueueable(), "{state:?} should be enqueueable");
    }
    for state in [
        LinkCandidateState::Resolving,
        LinkCandidateState::Checking,
        LinkCandidateState::Enqueued,
        LinkCandidateState::Unresolvable,
    ] {
        assert!(!state.is_enqueueable(), "{state:?} should be refused");
    }
}

/// The defect RD-110-07 exists for: a link whose address answers with a page must never
/// reach the queue, however it got into the list. Every other unhappy state still may.
#[test]
fn a_page_that_is_not_a_file_can_never_be_queued() {
    assert!(!LinkCandidateState::Unresolvable.is_enqueueable());
    for state in [
        LinkCandidateState::Offline,
        LinkCandidateState::Unsupported,
        LinkCandidateState::Error,
    ] {
        assert!(state.is_enqueueable(), "{state:?} must stay queueable");
    }
}

/// A check that fails outright leaves the link in `Error`, and that used to be the one
/// state the enqueue handler refused — stricter than `Offline`, which means the file is
/// known to be gone. Nothing about a failed check is evidence about the file.
#[test]
fn a_failed_check_is_not_worse_than_a_confirmed_offline_file() {
    assert_eq!(
        LinkCandidateState::Error.is_enqueueable(),
        LinkCandidateState::Offline.is_enqueueable()
    );
}
