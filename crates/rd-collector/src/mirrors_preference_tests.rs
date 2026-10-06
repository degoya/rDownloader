//! Which member of a mirror group is chosen: preference, pin, hidden hoster, refused proposal.

use super::{HOSTS, link};
use crate::MirrorSeparations;
use crate::mirrors::{MirrorInput, group_mirrors, hoster_of};
use rd_core::{MirrorHint, MirrorPreference, MirrorSource};

/// The preference decides which member of a group is the chosen one, and it keeps
/// deciding: the same preference applied to a second package chooses there too. That is
/// the whole of "a default that carries over to the next package".
#[test]
fn the_preference_chooses_the_mirror_and_keeps_choosing() {
    let preference = MirrorPreference {
        quality: Some("1080p".to_owned()),
        language: Some("German".to_owned()),
        hoster: None,
        ..MirrorPreference::default()
    };
    let first_package = [
        link(HOSTS[0], Some("Show.E01.German.720p.mkv"), Some(1_000)),
        link(HOSTS[1], Some("Show.E01.German.1080p.mkv"), Some(1_000)),
    ];
    let second_package = [
        link(HOSTS[2], Some("Other.E02.English.1080p.mkv"), Some(2_000)),
        link(HOSTS[3], Some("Other.E02.German.1080p.mkv"), Some(2_000)),
    ];
    // The two members share a name only after the quality token is read off it, so each
    // pair is grouped by a declaration, exactly as a release page would deliver it.
    let hint = MirrorHint {
        group: "release".to_owned(),
        quality: None,
        language: None,
    };
    let declared = |set: [MirrorInput<'_>; 2]| -> Vec<Option<rd_core::CandidateMirror>> {
        let inputs: Vec<MirrorInput<'_>> = set
            .into_iter()
            .map(|input| MirrorInput {
                hint: Some(&hint),
                ..input
            })
            .collect();
        group_mirrors(&inputs, &preference, &MirrorSeparations::default())
    };
    let first = declared(first_package);
    assert!(
        !first[0].as_ref().expect("a group").selected,
        "720p loses to the preferred 1080p"
    );
    assert!(first[1].as_ref().expect("a group").selected);
    // Second package, same standing preference: the German 1080p one wins there too,
    // although it is not the first member.
    let second = declared(second_package);
    assert!(!second[0].as_ref().expect("a group").selected);
    assert!(second[1].as_ref().expect("a group").selected);
}

/// An empty preference changes nothing: the first member stays chosen, as before
/// RD-110-19, and a facet nobody set never hides or reorders anything.
#[test]
fn no_preference_keeps_the_first_member() {
    let inputs = [
        link(HOSTS[0], Some("Show.720p.mkv"), Some(1_000)),
        link(HOSTS[1], Some("Show.720p.mkv"), Some(1_000)),
    ];
    let groups = group_mirrors(
        &inputs,
        &MirrorPreference::default(),
        &MirrorSeparations::default(),
    );
    assert!(groups[0].as_ref().expect("a group").selected);
    assert!(!groups[1].as_ref().expect("a group").selected);
}

/// A pin outranks the preference: a decision somebody made by hand is not a default, and
/// a regroup must not quietly take it back. This is the per-package way out of a standing
/// preference.
#[test]
fn a_pinned_member_outranks_the_preference() {
    let preference = MirrorPreference {
        quality: Some("1080p".to_owned()),
        language: None,
        hoster: None,
        ..MirrorPreference::default()
    };
    let inputs = [
        MirrorInput {
            pinned: true,
            ..link(HOSTS[0], Some("Show.720p.mkv"), Some(1_000))
        },
        link(HOSTS[1], Some("Show.1080p.mkv"), Some(1_000)),
    ];
    // Two names, so the group is a declaration again.
    let hint = MirrorHint {
        group: "release".to_owned(),
        quality: None,
        language: None,
    };
    let inputs: Vec<MirrorInput<'_>> = inputs
        .into_iter()
        .map(|input| MirrorInput {
            hint: Some(&hint),
            ..input
        })
        .collect();
    let groups = group_mirrors(&inputs, &preference, &MirrorSeparations::default());
    let pinned = groups[0].as_ref().expect("a group");
    assert!(pinned.selected, "the pin wins over the preferred quality");
    assert!(pinned.pinned);
    assert!(!groups[1].as_ref().expect("a group").pinned);
}

/// A facet a mirror says nothing about is not a mismatch and not a match. An unlabelled
/// mirror must not outrank a labelled one, and it must not be ranked below one either on
/// the strength of a guess.
#[test]
fn an_unlabelled_mirror_matches_no_facet() {
    let preference = MirrorPreference {
        quality: Some("1080p".to_owned()),
        language: None,
        hoster: None,
        ..MirrorPreference::default()
    };
    let hint = MirrorHint {
        group: "release".to_owned(),
        quality: None,
        language: None,
    };
    let inputs: Vec<MirrorInput<'_>> = [
        link(HOSTS[0], Some("Show.part1.rar"), Some(1_000)),
        link(HOSTS[1], Some("Show.part2.rar"), Some(1_000)),
    ]
    .into_iter()
    .map(|input| MirrorInput {
        hint: Some(&hint),
        ..input
    })
    .collect();
    let groups = group_mirrors(&inputs, &preference, &MirrorSeparations::default());
    assert!(
        groups[0].as_ref().expect("a group").selected,
        "nothing matched, so the stable first member stays"
    );
}

/// The hoster facet is compared against the same reduction the interface shows.
#[test]
fn a_hoster_is_the_host_without_www() {
    assert_eq!(
        hoster_of("https://www.Rapidgator.net/file/1"),
        "rapidgator.net"
    );
    assert_eq!(hoster_of("not a url"), "");
}

/// The preference is a conjunction, so a mirror satisfying two facets beats one
/// satisfying one. Without that, "German 1080p" would settle for the first German mirror.
#[test]
fn more_matched_facets_win() {
    let preference = MirrorPreference {
        quality: Some("1080p".to_owned()),
        language: Some("German".to_owned()),
        hoster: None,
        ..MirrorPreference::default()
    };
    let hint = MirrorHint {
        group: "release".to_owned(),
        quality: None,
        language: None,
    };
    let inputs: Vec<MirrorInput<'_>> = [
        link(HOSTS[0], Some("Show.German.720p.mkv"), Some(1_000)),
        link(HOSTS[1], Some("Show.English.1080p.mkv"), Some(1_000)),
        link(HOSTS[2], Some("Show.German.1080p.mkv"), Some(1_000)),
    ]
    .into_iter()
    .map(|input| MirrorInput {
        hint: Some(&hint),
        ..input
    })
    .collect();
    let groups = group_mirrors(&inputs, &preference, &MirrorSeparations::default());
    assert!(groups[2].as_ref().expect("a group").selected);
    assert_eq!(
        groups
            .iter()
            .flatten()
            .filter(|entry| entry.selected)
            .count(),
        1
    );
}

/// RD-130-21: a hidden hoster's mirror is the fallback, not the chosen one — even when it
/// is first and even when it matches the preferred quality better. A pin still wins.
#[test]
fn a_hidden_hoster_is_never_chosen_while_a_shown_one_exists() {
    let preference = MirrorPreference {
        quality: Some("1080p".to_owned()),
        hidden_hosters: vec!["A.example".to_owned()],
        ..MirrorPreference::default()
    };
    let hint = MirrorHint {
        group: "release".to_owned(),
        quality: None,
        language: None,
    };
    let declared = |inputs: [MirrorInput<'_>; 3]| -> Vec<Option<rd_core::CandidateMirror>> {
        let inputs: Vec<MirrorInput<'_>> = inputs
            .into_iter()
            .map(|input| MirrorInput {
                hint: Some(&hint),
                ..input
            })
            .collect();
        group_mirrors(&inputs, &preference, &MirrorSeparations::default())
    };
    let groups = declared([
        link(HOSTS[0], Some("Show.1080p.mkv"), Some(1_000)),
        link(HOSTS[1], Some("Show.720p.mkv"), Some(1_000)),
        link(HOSTS[2], Some("Show.1080p.mkv"), Some(1_000)),
    ]);
    let selected: Vec<bool> = groups
        .iter()
        .map(|entry| entry.as_ref().expect("a group").selected)
        .collect();
    assert_eq!(
        selected,
        [false, false, true],
        "the shown 1080p mirror, not the hidden one ahead of it"
    );

    let pinned = declared([
        MirrorInput {
            pinned: true,
            ..link(HOSTS[0], Some("Show.1080p.mkv"), Some(1_000))
        },
        link(HOSTS[1], Some("Show.720p.mkv"), Some(1_000)),
        link(HOSTS[2], Some("Show.1080p.mkv"), Some(1_000)),
    ]);
    assert!(
        pinned[0].as_ref().expect("a group").selected,
        "a mirror chosen by hand stays chosen, hidden hoster or not"
    );

    let only_hidden = MirrorPreference {
        hidden_hosters: vec!["a.example".to_owned(), "b.example".to_owned()],
        ..MirrorPreference::default()
    };
    let inputs = [
        link(HOSTS[0], Some("Show.mkv"), Some(1_000)),
        link(HOSTS[1], Some("Show.mkv"), Some(1_000)),
    ];
    let groups = group_mirrors(&inputs, &only_hidden, &MirrorSeparations::default());
    assert!(
        groups[0].as_ref().expect("a group").selected,
        "with every member hidden the stable first one stays"
    );
}

/// RD-110-34: a proposal somebody refused does not come back, whatever the next source
/// would have made of the same links.
///
/// The two links below share a name and, at the second call, a size — which is exactly
/// the upgrade the online check performs. A refusal bound to the source `name` would be
/// undone there, so it is bound to the pair instead.
#[test]
fn a_refused_pair_is_not_regrouped_by_a_later_source() {
    let separations = MirrorSeparations::new([(HOSTS[0].to_owned(), HOSTS[1].to_owned())]);
    let bare = [
        link(HOSTS[0], Some("Show.S01E01.mkv"), None),
        link(HOSTS[1], Some("Show.S01E01.mkv"), None),
    ];
    assert!(
        group_mirrors(&bare, &MirrorPreference::default(), &separations)
            .iter()
            .all(Option::is_none),
        "the proposal that was refused is not offered again"
    );
    let sized = [
        link(HOSTS[0], Some("Show.S01E01.mkv"), Some(1_000_000)),
        link(HOSTS[1], Some("Show.S01E01.mkv"), Some(1_000_000)),
    ];
    assert!(
        group_mirrors(&sized, &MirrorPreference::default(), &separations)
            .iter()
            .all(Option::is_none),
        "a size arriving afterwards does not overrule a person who looked at both files"
    );
    let hint = MirrorHint {
        group: "release-1".to_owned(),
        quality: None,
        language: None,
    };
    let declared = [
        MirrorInput {
            hint: Some(&hint),
            ..link(HOSTS[0], Some("a.mkv"), None)
        },
        MirrorInput {
            hint: Some(&hint),
            ..link(HOSTS[1], Some("b.mkv"), None)
        },
    ];
    assert!(
        group_mirrors(&declared, &MirrorPreference::default(), &separations)
            .iter()
            .all(Option::is_none),
        "even a declaration does not put a refused pair back together"
    );
}

/// A refusal binds the set it was made about, and nothing else.
///
/// Two consequences, both of them deliberate: a third link with the same name does not
/// pick up one of the refused two as a partner, because the set it would join is the
/// refused one; and a link the refusal never named groups normally.
#[test]
fn a_refusal_binds_that_set_and_leaves_other_links_alone() {
    let separations = MirrorSeparations::new([(HOSTS[0].to_owned(), HOSTS[1].to_owned())]);
    let joined = [
        link(HOSTS[0], Some("Show.S01E01.mkv"), None),
        link(HOSTS[1], Some("Show.S01E01.mkv"), None),
        link(HOSTS[2], Some("Show.S01E01.mkv"), None),
    ];
    assert!(
        group_mirrors(&joined, &MirrorPreference::default(), &separations)
            .iter()
            .all(Option::is_none),
        "a later link with the same name does not revive the refused grouping"
    );
    let elsewhere = [
        link(HOSTS[2], Some("Other.S01E01.mkv"), None),
        link(HOSTS[3], Some("Other.S01E01.mkv"), None),
    ];
    let groups = group_mirrors(&elsewhere, &MirrorPreference::default(), &separations);
    assert_eq!(
        groups.iter().filter(|entry| entry.is_some()).count(),
        2,
        "links the refusal never named are grouped as before"
    );
    assert_eq!(
        groups[0].as_ref().expect("a group").source,
        MirrorSource::Name
    );
}
