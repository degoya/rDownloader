use rd_core::{CategoryId, CollectorPackageId, LinkCandidateState, NzbImportId, NzbImportState};

use super::{ImportFacts, Selection, select};

const NO_PACKAGES: [CollectorPackageId; 0] = [];
const NO_LINKS: [(CollectorPackageId, LinkCandidateState); 0] = [];

fn no_imports() -> Vec<ImportFacts> {
    Vec::new()
}

fn import(state: NzbImportState, duplicate: bool) -> ImportFacts {
    ImportFacts {
        id: NzbImportId::new(),
        state,
        duplicate,
        category: None,
    }
}

/// A package goes when one of its links is online, with every link the enqueue takes, in the
/// LinkGrabber's order; one that holds nothing online stays, as it does for the web's `E`.
#[test]
fn packages_with_an_online_link_go_in_order_with_their_links() {
    let (first, second, offline) = (
        CollectorPackageId::new(),
        CollectorPackageId::new(),
        CollectorPackageId::new(),
    );
    let selection = select(
        [second, offline, first],
        [
            (first, LinkCandidateState::Online),
            (first, LinkCandidateState::Offline),
            (first, LinkCandidateState::Unresolvable),
            (second, LinkCandidateState::Online),
            (offline, LinkCandidateState::Offline),
        ],
        no_imports(),
    );
    assert_eq!(
        selection,
        Selection {
            packages: vec![(second, 1), (first, 2)],
            nzbs: vec![],
            duplicates: 0,
        }
    );
}

/// The web asks before adding duplicates again; the tray cannot ask, so a package holding one
/// stays whole in the LinkGrabber and is counted.
#[test]
fn a_package_holding_a_duplicate_stays_and_is_counted() {
    let (clean, mixed) = (CollectorPackageId::new(), CollectorPackageId::new());
    let selection = select(
        [clean, mixed],
        [
            (clean, LinkCandidateState::Online),
            (mixed, LinkCandidateState::Online),
            (mixed, LinkCandidateState::Duplicate),
        ],
        no_imports(),
    );
    assert_eq!(selection.packages, vec![(clean, 1)]);
    assert_eq!(selection.duplicates, 1);
}

/// NZB imports go when imported; a failed or already queued one is not the tray's, and a
/// duplicate stays like a package holding one.
#[test]
fn nzb_imports_go_unless_failed_queued_or_duplicate() {
    let category = CategoryId::new();
    let mut wanted = import(NzbImportState::Imported, false);
    wanted.category = Some(category);
    let wanted_id = wanted.id;
    let selection = select(
        NO_PACKAGES,
        NO_LINKS,
        [
            wanted,
            import(NzbImportState::Imported, true),
            import(NzbImportState::Failed, false),
            import(NzbImportState::Enqueued, false),
        ],
    );
    assert_eq!(selection.nzbs, vec![(wanted_id, Some(category))]);
    assert_eq!(selection.duplicates, 1);
    assert!(selection.packages.is_empty());
}

/// An empty LinkGrabber selects nothing, which the agent reports as such.
#[test]
fn an_empty_linkgrabber_selects_nothing() {
    assert_eq!(
        select(NO_PACKAGES, NO_LINKS, no_imports()),
        Selection::default()
    );
}
