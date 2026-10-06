use super::{MirrorInput, group_mirrors, language_of, quality_of};
use crate::MirrorSeparations;
use rd_core::{MirrorHint, MirrorPreference, MirrorSource};

const HOSTS: [&str; 5] = [
    "https://a.example/1",
    "https://b.example/2",
    "https://c.example/3",
    "https://d.example/4",
    "https://e.example/5",
];

fn link<'a>(url: &'a str, file_name: Option<&'a str>, size: Option<u64>) -> MirrorInput<'a> {
    MirrorInput {
        id: url,
        url,
        file_name,
        file_name_declared: true,
        size,
        hint: None,
        pinned: false,
    }
}

/// Source 1: the release page states that its five links are one file.
///
/// The strongest of the three, and the only one that works when the hosters disagree
/// about the name — which they routinely do, because each rewrites it.
#[test]
fn a_page_that_names_its_mirrors_forms_one_group() {
    let hint = MirrorHint {
        group: "release-1".to_owned(),
        quality: None,
        language: None,
    };
    let inputs: Vec<MirrorInput<'_>> = HOSTS
        .iter()
        .enumerate()
        .map(|(index, url)| MirrorInput {
            hint: Some(&hint),
            ..link(
                url,
                Some(["a.rar", "b.rar", "c.rar", "d.rar", "e.rar"][index]),
                None,
            )
        })
        .collect();
    let groups = group_mirrors(
        &inputs,
        &MirrorPreference::default(),
        &MirrorSeparations::default(),
    );
    assert_eq!(groups.iter().filter(|entry| entry.is_some()).count(), 5);
    let first = groups[0].as_ref().expect("a group");
    assert_eq!(first.source, MirrorSource::Declared);
    assert!(
        groups
            .iter()
            .flatten()
            .all(|entry| entry.group == first.group)
    );
    assert_eq!(
        groups
            .iter()
            .flatten()
            .filter(|entry| entry.selected)
            .count(),
        1
    );
    assert!(first.selected);
}

/// Source 2: the same name and a size that agrees, which is what the online check leaves.
#[test]
fn a_name_and_a_size_that_agree_are_evidence() {
    let inputs = [
        link(HOSTS[0], Some("Show.S01E01.mkv"), Some(1_000_000)),
        link(HOSTS[1], Some("Show.S01E01.mkv"), Some(1_000_100)),
        link(HOSTS[2], Some("Other.mkv"), Some(5)),
    ];
    let groups = group_mirrors(
        &inputs,
        &MirrorPreference::default(),
        &MirrorSeparations::default(),
    );
    let first = groups[0].as_ref().expect("a group");
    assert_eq!(first.source, MirrorSource::NameAndSize);
    assert_eq!(
        groups[1].as_ref().map(|entry| entry.group.clone()),
        Some(first.group.clone())
    );
    assert!(groups[2].is_none(), "a link of its own is no mirror");
}

/// Source 3: a shared name with nothing to corroborate it is a proposal, not a fact.
#[test]
fn a_shared_name_alone_is_only_a_proposal() {
    let inputs = [
        link(HOSTS[0], Some("Show.S01E01.mkv"), None),
        link(HOSTS[1], Some("Show.S01E01.mkv"), None),
    ];
    let groups = group_mirrors(
        &inputs,
        &MirrorPreference::default(),
        &MirrorSeparations::default(),
    );
    assert_eq!(
        groups[0].as_ref().map(|entry| entry.source),
        Some(MirrorSource::Name)
    );
    assert_eq!(
        groups[1].as_ref().map(|entry| entry.source),
        Some(MirrorSource::Name)
    );
}

/// One member that never reported a size leaves the whole group a proposal: the sizes
/// were never compared, and saying they were would overstate what is known.
#[test]
fn one_unknown_size_keeps_the_group_a_proposal() {
    let inputs = [
        link(HOSTS[0], Some("Show.mkv"), Some(1_000)),
        link(HOSTS[1], Some("Show.mkv"), None),
    ];
    let groups = group_mirrors(
        &inputs,
        &MirrorPreference::default(),
        &MirrorSeparations::default(),
    );
    assert_eq!(
        groups[0].as_ref().map(|entry| entry.source),
        Some(MirrorSource::Name)
    );
}

/// The same address twice is a duplicate, which the collector already has a state for.
/// It must not come back as a mirror of itself and hand the group a pointless second turn.
#[test]
fn the_same_address_twice_is_not_a_mirror() {
    let inputs = [
        link(HOSTS[0], Some("Show.mkv"), Some(1_000)),
        link(HOSTS[0], Some("Show.mkv"), Some(1_000)),
    ];
    assert!(
        group_mirrors(
            &inputs,
            &MirrorPreference::default(),
            &MirrorSeparations::default()
        )
        .iter()
        .all(Option::is_none)
    );
}

/// A name intake took from the address is no evidence: `/download` on two unrelated
/// hosts would otherwise make every such pair a mirror pair.
#[test]
fn a_name_from_the_address_needs_a_size() {
    let bare = [
        MirrorInput {
            file_name_declared: false,
            ..link(HOSTS[0], Some("download"), None)
        },
        MirrorInput {
            file_name_declared: false,
            ..link(HOSTS[1], Some("download"), None)
        },
    ];
    assert!(
        group_mirrors(
            &bare,
            &MirrorPreference::default(),
            &MirrorSeparations::default()
        )
        .iter()
        .all(Option::is_none)
    );
    let sized = [
        MirrorInput {
            file_name_declared: false,
            ..link(HOSTS[0], Some("download"), Some(7))
        },
        MirrorInput {
            file_name_declared: false,
            ..link(HOSTS[1], Some("download"), Some(7))
        },
    ];
    assert_eq!(
        group_mirrors(
            &sized,
            &MirrorPreference::default(),
            &MirrorSeparations::default()
        )[0]
        .as_ref()
        .map(|entry| entry.source),
        Some(MirrorSource::NameAndSize)
    );
}

/// Sizes that cannot be the same file split the name into two groups.
#[test]
fn sizes_that_disagree_split_the_name() {
    let inputs = [
        link(HOSTS[0], Some("Show.mkv"), Some(1_000)),
        link(HOSTS[1], Some("Show.mkv"), Some(1_000)),
        link(HOSTS[2], Some("Show.mkv"), Some(9_000_000)),
        link(HOSTS[3], Some("Show.mkv"), Some(9_000_000)),
    ];
    let groups = group_mirrors(
        &inputs,
        &MirrorPreference::default(),
        &MirrorSeparations::default(),
    );
    assert_eq!(
        groups[0].as_ref().map(|entry| entry.group.clone()),
        groups[1].as_ref().map(|entry| entry.group.clone())
    );
    assert_ne!(
        groups[0].as_ref().map(|entry| entry.group.clone()),
        groups[2].as_ref().map(|entry| entry.group.clone())
    );
    assert_eq!(
        groups[2].as_ref().map(|entry| entry.group.clone()),
        groups[3].as_ref().map(|entry| entry.group.clone())
    );
}

/// A declaration is not reconsidered by the file names: two hosters that renamed the
/// file the same way do not pull a third link out of the group the page put it in.
#[test]
fn a_declaration_outranks_the_names() {
    let hint = MirrorHint {
        group: "page".to_owned(),
        quality: None,
        language: None,
    };
    let inputs = [
        MirrorInput {
            hint: Some(&hint),
            ..link(HOSTS[0], Some("Show.mkv"), Some(10))
        },
        MirrorInput {
            hint: Some(&hint),
            ..link(HOSTS[1], Some("Different.mkv"), Some(10))
        },
        link(HOSTS[2], Some("Show.mkv"), Some(10)),
    ];
    let groups = group_mirrors(
        &inputs,
        &MirrorPreference::default(),
        &MirrorSeparations::default(),
    );
    assert_eq!(
        groups[0].as_ref().map(|entry| entry.source),
        Some(MirrorSource::Declared)
    );
    assert_eq!(
        groups[0].as_ref().map(|entry| entry.group.clone()),
        groups[1].as_ref().map(|entry| entry.group.clone())
    );
    assert!(groups[2].is_none(), "one link left over is no group");
}

/// The facets: what the source named wins, and the release name fills the rest in.
#[test]
fn quality_and_language_come_from_the_source_or_the_name() {
    let hint = MirrorHint {
        group: "page".to_owned(),
        quality: Some("SD".to_owned()),
        language: None,
    };
    let inputs = [
        MirrorInput {
            hint: Some(&hint),
            ..link(HOSTS[0], Some("Show.German.DL.1080p.WEB.x264.mkv"), None)
        },
        MirrorInput {
            hint: Some(&hint),
            ..link(HOSTS[1], Some("Show.German.DL.1080p.WEB.x264.mkv"), None)
        },
    ];
    let groups = group_mirrors(
        &inputs,
        &MirrorPreference::default(),
        &MirrorSeparations::default(),
    );
    let first = groups[0].as_ref().expect("a group");
    assert_eq!(first.quality.as_deref(), Some("SD"));
    assert_eq!(first.language.as_deref(), Some("German"));
}

#[test]
fn a_release_name_is_read_token_by_token() {
    assert_eq!(
        quality_of("Show.S01E01.1080p.WEB.mkv").as_deref(),
        Some("1080p")
    );
    assert_eq!(quality_of("Dune.2021.UHD.mkv").as_deref(), Some("2160p"));
    assert_eq!(quality_of("Show.2160.mkv"), None);
    assert_eq!(language_of("Show.German.DL.mkv").as_deref(), Some("German"));
    assert_eq!(language_of("mldonkey.iso"), None);
}

#[path = "mirrors_preference_tests.rs"]
mod preference;
