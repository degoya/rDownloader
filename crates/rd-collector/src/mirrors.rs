//! Links of one package that point at the same file (RD-110-18).
//!
//! A release page delivers what it is made for: the same episode at five hosters. Without a
//! word for that, those five arrive as five candidates and four of them get deleted by hand.
//! A mirror is not a duplicate, and the distinction is the whole point of this module: a
//! duplicate is the *same address* a second time and adds nothing, so it is marked and set
//! aside; a mirror is a *different address* for the same bytes and is kept, because it is
//! what remains when the chosen one goes offline.
//!
//! Nothing here reads or writes [`rd_core::LinkCandidateState`]. Grouping is computed from
//! what the source said and from the names and sizes on the links; the `Duplicate` state is
//! neither an input nor an output of it, so the two can never swallow one another. The one
//! place they meet is the rule that two links with the same address are never mirrors — which
//! is exactly the case the duplicate state already covers.
//!
//! A group lives inside one package. That is the unit the queue downloads, and merging
//! mirrors across packages would make "one of these runs" a statement about two releases.

use std::collections::BTreeMap;

use rd_core::{CandidateMirror, MirrorFacet, MirrorHint, MirrorPreference, MirrorSource};

use crate::MirrorSeparations;

/// How far two reported sizes may differ and still be the same file, as a fraction.
///
/// The same tolerance `rd_scheduler::mirrors` applies one layer down, and for the same
/// reason: hosters round and report inconsistently, and a percent covers that without
/// letting a genuinely different file in.
const SIZE_TOLERANCE: f64 = 0.01;

/// The qualities a release name may spell out, and what each is called once recognised.
///
/// Deliberately a closed list of the tokens scene names actually use. Anything else is no
/// quality, and guessing one from a number in a file name would put `1998` on a mirror.
const QUALITIES: &[(&str, &str)] = &[
    ("2160p", "2160p"),
    ("4k", "2160p"),
    ("uhd", "2160p"),
    ("1440p", "1440p"),
    ("1080p", "1080p"),
    ("1080i", "1080p"),
    ("720p", "720p"),
    ("576p", "576p"),
    ("540p", "540p"),
    ("480p", "480p"),
    ("360p", "360p"),
    ("240p", "240p"),
];

/// The language tokens a release name may carry, and what each is called once recognised.
///
/// `ml` and `dl` are the scene's own abbreviations for a file carrying several audio tracks;
/// they name no single language, which is why they map to a word of their own rather than to
/// the first language in the name.
const LANGUAGES: &[(&str, &str)] = &[
    ("german", "German"),
    ("deutsch", "German"),
    ("english", "English"),
    ("french", "French"),
    ("spanish", "Spanish"),
    ("italian", "Italian"),
    ("dutch", "Dutch"),
    ("japanese", "Japanese"),
    ("korean", "Korean"),
    ("multi", "Multi"),
    ("ml", "Multi"),
    ("dl", "Dual"),
];

/// One link, as far as mirror grouping is concerned.
#[derive(Clone, Copy, Debug)]
pub struct MirrorInput<'a> {
    /// What a separation names this link by (RD-110-34).
    ///
    /// The candidate's own identifier rather than its address: a person's decision that two
    /// links are not the same file has to survive a regroup and a move to another package,
    /// and the identifier is the only thing about a link that survives both.
    pub id: &'a str,
    /// The address, verbatim. Two links sharing one are never mirrors of each other.
    pub url: &'a str,
    pub file_name: Option<&'a str>,
    /// Whether the name came from the source rather than from the address's last segment.
    ///
    /// Intake substitutes that segment when the source names none, and afterwards the two
    /// look alike — but `/download` on two unrelated hosts is no evidence of anything, so
    /// such a name needs a size beside it before a group is built on it.
    pub file_name_declared: bool,
    pub size: Option<u64>,
    /// What the source stated about this link's group, if it stated anything.
    pub hint: Option<&'a MirrorHint>,
    /// Whether a person chose this link as their group's mirror by hand (RD-110-19).
    ///
    /// A pin outranks the preference and survives every regroup. Two pinned members of one
    /// group cannot happen — pinning clears the siblings — but if it ever did, the first in
    /// order wins, because an arbitrary stable answer beats a changing one.
    pub pinned: bool,
}

/// Assigns a mirror group to the links of one package, one answer per input.
///
/// The three sources in the order they are trusted: what the source declared, then a name and
/// a size that agree, then a name alone as a proposal. A link the earlier source already
/// grouped is never reconsidered by a later one, so a declaration cannot be overruled by two
/// file names that happen to match.
///
/// `separations` is what a person already refused (RD-110-34), and it is checked before any
/// source is allowed to speak. That order is the point: whether a set would have become a
/// declaration, a name-and-size agreement or a proposal is re-decided on every regroup, so a
/// refusal that only bound one of the three would come back the moment a size arrived.
#[must_use]
pub fn group_mirrors(
    inputs: &[MirrorInput<'_>],
    preference: &MirrorPreference,
    separations: &MirrorSeparations,
) -> Vec<Option<CandidateMirror>> {
    let mut out: Vec<Option<CandidateMirror>> = vec![None; inputs.len()];
    let mut used: Vec<String> = Vec::new();

    // Source 1: the page said so.
    let mut declared: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, input) in inputs.iter().enumerate() {
        let Some(key) = input
            .hint
            .map(|hint| hint.group.trim())
            .filter(|key| !key.is_empty())
        else {
            continue;
        };
        declared.entry(key).or_default().push(index);
    }
    for (key, members) in declared {
        let members = distinct_addresses(inputs, &members);
        if members.len() < 2 || refused(inputs, &members, separations) {
            continue;
        }
        let key = unique_key(&mut used, key.to_owned());
        assign(
            inputs,
            &mut out,
            &members,
            &key,
            MirrorSource::Declared,
            preference,
        );
    }

    // Sources 2 and 3: the names, corroborated by a size or not.
    let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, input) in inputs.iter().enumerate() {
        if out[index].is_some() {
            continue;
        }
        // A name taken from the address proves nothing on its own.
        if !input.file_name_declared && input.size.is_none() {
            continue;
        }
        let Some(name) = normalized_name(input.file_name) else {
            continue;
        };
        by_name.entry(name).or_default().push(index);
    }
    for (name, members) in by_name {
        let members = distinct_addresses(inputs, &members);
        if members.len() < 2 {
            continue;
        }
        // Same name, wildly different size: a different file that happens to share a name.
        let mut clusters: Vec<(Option<u64>, Vec<usize>)> = Vec::new();
        for index in members {
            let size = inputs[index].size;
            match clusters
                .iter_mut()
                .find(|(reference, _)| same_file_size(*reference, size))
            {
                Some((reference, cluster)) => {
                    // A cluster that learns a size keeps it, so the next unknown still joins.
                    if reference.is_none() {
                        *reference = size;
                    }
                    cluster.push(index);
                }
                None => clusters.push((size, vec![index])),
            }
        }
        let split = clusters.len() > 1;
        for (position, (_, cluster)) in clusters.iter().enumerate() {
            if cluster.len() < 2 || refused(inputs, cluster, separations) {
                continue;
            }
            // A size only corroborates a name when every member of the cluster reported one.
            // One unknown size among them means the agreement was never measured, which is a
            // proposal and has to read as one.
            let source = if cluster.iter().all(|index| inputs[*index].size.is_some()) {
                MirrorSource::NameAndSize
            } else {
                MirrorSource::Name
            };
            let key = if split {
                format!("{name}#{position}")
            } else {
                name.clone()
            };
            let key = unique_key(&mut used, key);
            assign(inputs, &mut out, cluster, &key, source, preference);
        }
    }
    out
}

/// The quality a release name spells out, if it spells one out.
#[must_use]
pub fn quality_of(file_name: &str) -> Option<String> {
    token_lookup(file_name, QUALITIES)
}

/// The language a release name spells out, if it spells one out.
#[must_use]
pub fn language_of(file_name: &str) -> Option<String> {
    token_lookup(file_name, LANGUAGES)
}

/// Writes one group onto its members and marks one of them as the chosen mirror.
///
/// Four rules in order, and the order is the whole point (RD-110-19). A member somebody
/// pinned wins outright: a standing preference is a default, and a default that revises a
/// decision a person already made is worse than no default at all. Otherwise a member at a
/// shown hoster beats one at a hidden hoster (RD-130-21), because the hidden one is meant to
/// be the fallback and a chosen mirror the LinkGrabber does not show would be queued first.
/// Among those the member satisfying the most facets of the preference wins. With nothing
/// pinned, hidden or preferred it is the first in submission order, as it was before this
/// job — not the largest or the fastest, because nothing has been measured at this point and
/// a choice that changes between two views of the same list is worse than one that is merely
/// arbitrary.
fn assign(
    inputs: &[MirrorInput<'_>],
    out: &mut [Option<CandidateMirror>],
    members: &[usize],
    key: &str,
    source: MirrorSource,
    preference: &MirrorPreference,
) {
    let facets: Vec<(MirrorFacet, String)> = preference.facets().collect();
    let described: Vec<CandidateMirror> = members
        .iter()
        .map(|index| {
            let hint = inputs[*index].hint;
            let name = inputs[*index].file_name.unwrap_or_default();
            CandidateMirror {
                group: key.to_owned(),
                source,
                selected: false,
                pinned: inputs[*index].pinned,
                quality: facet(hint.and_then(|hint| hint.quality.as_deref()), || {
                    quality_of(name)
                }),
                language: facet(hint.and_then(|hint| hint.language.as_deref()), || {
                    language_of(name)
                }),
            }
        })
        .collect();
    let rank = |position: usize| {
        let url = inputs[members[position]].url;
        (
            !preference.hides(&hoster_of(url)),
            matched_facets(&facets, &described[position], url),
        )
    };
    let chosen = members
        .iter()
        .position(|index| inputs[*index].pinned)
        .or_else(|| {
            (!facets.is_empty() || !preference.hidden_hosters.is_empty()).then(|| {
                // `max_by_key` keeps the *last* maximum, so the scan runs by hand to keep the
                // first: with two equally good mirrors the earlier one is the stable answer.
                let mut best = (0_usize, rank(0));
                for position in 1..members.len() {
                    let score = rank(position);
                    if score > best.1 {
                        best = (position, score);
                    }
                }
                best.0
            })
        })
        .unwrap_or(0);
    for (position, index) in members.iter().enumerate() {
        let mut entry = described[position].clone();
        entry.selected = position == chosen;
        out[*index] = Some(entry);
    }
}

/// How many facets of the preference this mirror satisfies.
///
/// A facet the mirror says nothing about counts as unmatched rather than as a mismatch: the
/// closed token list of RD-110-18 deliberately guesses nothing, so an unlabelled mirror is
/// simply not evidence either way and must not outrank a labelled one.
fn matched_facets(facets: &[(MirrorFacet, String)], mirror: &CandidateMirror, url: &str) -> usize {
    facets
        .iter()
        .filter(|(facet, wanted)| {
            let value = match facet {
                MirrorFacet::Quality => mirror.quality.clone(),
                MirrorFacet::Language => mirror.language.clone(),
                MirrorFacet::Hoster => Some(hoster_of(url)),
            };
            value.is_some_and(|value| value.to_ascii_lowercase() == *wanted)
        })
        .count()
}

/// The hoster a link is read as: the host without a leading `www.`.
///
/// The same reduction `hosterOf` makes in the interface, so a preference set there matches
/// what is compared here. An address that does not parse has no hoster rather than a made-up
/// one, which is the "nothing is guessed" rule again.
#[must_use]
pub fn hoster_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_ascii_lowercase))
        .map(|host| host.strip_prefix("www.").unwrap_or(&host).to_owned())
        .unwrap_or_default()
}

/// What the source named, or what the release name says when it named nothing.
fn facet(declared: Option<&str>, derived: impl FnOnce() -> Option<String>) -> Option<String> {
    match declared.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => Some(value.to_owned()),
        None => derived(),
    }
}

/// Whether a person has already stated that two of these links are not the same file.
///
/// The whole set is refused rather than trimmed to the members that are still compatible.
/// Trimming would have to pick which of three mutually separated links keeps the group, and
/// there is no honest answer to that: the evidence a proposal rests on — one shared name —
/// is the same for every member, so once it has been rejected for this set it carries none
/// of them. It is also why a link that arrives later with the same name joins nothing: the
/// set it would join is the one that was refused.
fn refused(inputs: &[MirrorInput<'_>], members: &[usize], separations: &MirrorSeparations) -> bool {
    if separations.is_empty() {
        return false;
    }
    members.iter().enumerate().any(|(position, left)| {
        members[position + 1..]
            .iter()
            .any(|right| separations.separated(inputs[*left].id, inputs[*right].id))
    })
}

/// The members with an address none of the earlier members already used.
///
/// The same address twice is a duplicate, which the collector already has a state for. Left
/// in, it would make a link a mirror of itself and hand the group a second turn that fetches
/// the very bytes the first one failed to get.
fn distinct_addresses(inputs: &[MirrorInput<'_>], members: &[usize]) -> Vec<usize> {
    let mut seen: Vec<&str> = Vec::new();
    let mut kept = Vec::with_capacity(members.len());
    for index in members {
        let url = inputs[*index].url;
        if seen.contains(&url) {
            continue;
        }
        seen.push(url);
        kept.push(*index);
    }
    kept
}

/// A key no other group in this package is already using.
fn unique_key(used: &mut Vec<String>, wanted: String) -> String {
    let mut key = wanted;
    let mut suffix = 1_u32;
    while used.contains(&key) {
        key = format!("{key}~{suffix}");
        suffix += 1;
    }
    used.push(key.clone());
    key
}

/// The file name in the form two links are compared by: sanitized and lowercased.
fn normalized_name(file_name: Option<&str>) -> Option<String> {
    let trimmed = file_name.unwrap_or_default().trim();
    if trimmed.is_empty() {
        return None;
    }
    let sanitized = rd_files::sanitize_file_name(trimmed).to_ascii_lowercase();
    (!sanitized.is_empty()).then_some(sanitized)
}

/// Whether two reported sizes are consistent with being the same file.
///
/// An unknown size matches anything: a hoster that announces none is not saying the file is
/// different, and refusing to group on that would leave the common case ungrouped. What such
/// a group may not claim is that a size confirmed it, which is why the caller looks at the
/// members again before naming the source.
fn same_file_size(left: Option<u64>, right: Option<u64>) -> bool {
    let (Some(left), Some(right)) = (left, right) else {
        return true;
    };
    if left == right {
        return true;
    }
    let larger = left.max(right) as f64;
    let difference = left.abs_diff(right) as f64;
    difference / larger <= SIZE_TOLERANCE
}

/// The first entry of `table` whose token appears in `text` as a whole word.
///
/// Whole word on purpose: `mldonkey` is not a multi-language release, and `Dune.2021` is not
/// 2160p because it contains `4k` nowhere. Separators are everything that is not a letter or
/// a digit, which is how release names are written.
fn token_lookup(text: &str, table: &[(&str, &str)]) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let tokens: Vec<&str> = lower
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();
    table
        .iter()
        .find(|(token, _)| tokens.contains(token))
        .map(|(_, name)| (*name).to_owned())
}

#[cfg(test)]
mod tests {
    use super::{MirrorInput, group_mirrors, hoster_of, language_of, quality_of};
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
}
