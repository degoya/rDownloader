//! Mirror groups in the LinkGrabber: where a group came from, what a source stated and what a
//! person prefers (RD-110-18, RD-110-19).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Where a mirror group came from (RD-110-18).
///
/// Kept beside the group because the three are not equally strong, and the interface has to
/// be able to say so: a release page that states the five links are one file is a fact, two
/// links agreeing on name *and* size is evidence, and a bare name in common is a suggestion
/// somebody may want to overrule.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MirrorSource {
    /// The source said so — a site rule whose page is one release, or a crawler that named
    /// the group. Ordered first because it outranks anything derived from a file name.
    Declared,
    /// Same file name and a size that agrees, after the online check.
    NameAndSize,
    /// Same file name and nothing to corroborate it. A proposal, not a fact.
    Name,
}

/// What a source stated about one link's place among mirrors.
///
/// The input side of [`CandidateMirror`]: it travels with a link into the collector and is
/// stored as it arrived, so regrouping after an online check never loses what the page said.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct MirrorHint {
    /// The key the source used for "these links are the same file". Only compared with the
    /// keys of the same package, so a rule may use anything stable within one page.
    pub group: String,
    /// The quality the source named for this link, e.g. `1080p`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// The language the source named for this link, e.g. `German`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// What a person prefers when a mirror group offers a choice (RD-110-19).
///
/// Three facets, each optional and each combined with the others by conjunction. It is a
/// *preference*, not a rule: it decides which member of a group is the chosen one, it never
/// deletes a mirror and it never overrules a member somebody pinned by hand. Stored on the
/// server under its own settings key, so it applies to the next package that arrives and to
/// every package after a restart.
///
/// The three are compared case-insensitively against the values [`CandidateMirror`] carries,
/// which is what the source named or what the release name spells; the hoster is the link's
/// host without a leading `www.`.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct MirrorPreference {
    /// e.g. `1080p`. Matched against [`CandidateMirror::quality`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// e.g. `German`. Matched against [`CandidateMirror::language`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// e.g. `rapidgator.net`. Matched against the link's host.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hoster: Option<String>,
    /// Hosters the LinkGrabber hides (RD-130-21), each as [`Self::hoster`] spells one.
    ///
    /// Not a fourth facet. A facet narrows the list to what matches it; this takes named
    /// hosters out of it, several at once. Inside a mirror group it only ranks: a member at a
    /// hidden hoster is never the chosen one while a member at a shown hoster exists, and it
    /// stays in the group as the fallback the queue switches to. Stored with the facets because
    /// it is the same kind of standing decision and has to survive a restart the same way.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hidden_hosters: Vec<String>,
}

impl MirrorPreference {
    /// Whether nothing is preferred, in which case the first member stays the chosen one.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.facets().next().is_none() && self.hidden_hosters.is_empty()
    }

    /// Whether this hoster is one of [`Self::hidden_hosters`], compared case-insensitively.
    #[must_use]
    pub fn hides(&self, hoster: &str) -> bool {
        !hoster.is_empty()
            && self
                .hidden_hosters
                .iter()
                .any(|hidden| hidden.trim().eq_ignore_ascii_case(hoster))
    }

    /// The facets actually set, trimmed and lowercased, as `(what, value)` pairs.
    ///
    /// Empty strings are dropped rather than matched: a select that was cleared sends `""`,
    /// and a preference for the empty quality would match nothing and hide the whole list.
    pub fn facets(&self) -> impl Iterator<Item = (MirrorFacet, String)> + '_ {
        [
            (MirrorFacet::Quality, self.quality.as_deref()),
            (MirrorFacet::Language, self.language.as_deref()),
            (MirrorFacet::Hoster, self.hoster.as_deref()),
        ]
        .into_iter()
        .filter_map(|(facet, value)| {
            let value = value.map(str::trim).filter(|value| !value.is_empty())?;
            Some((facet, value.to_ascii_lowercase()))
        })
    }
}

/// Which of the three dimensions a [`MirrorPreference`] entry speaks about.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirrorFacet {
    Quality,
    Language,
    Hoster,
}

/// One candidate's membership in a mirror group.
///
/// A mirror is deliberately **not** a duplicate: [`LinkCandidateState::Duplicate`] means the
/// very same address is already in the collector and the copy adds nothing, while a mirror is
/// a *different* address for the same bytes and is kept precisely because it is needed the
/// moment the chosen one goes offline. Nothing here reads or writes that state, and two links
/// with the same address are never mirrors of each other.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct CandidateMirror {
    /// Key shared by the members of the group. Unique within the package and nowhere else:
    /// a group lives inside one package, because that is the unit the queue downloads.
    pub group: String,
    pub source: MirrorSource,
    /// Whether this is the member the package would use. Exactly one member of a group
    /// carries it.
    pub selected: bool,
    /// Whether a person chose this member by hand (RD-110-19).
    ///
    /// A pin outranks [`MirrorPreference`] and survives a regroup: a standing preference is a
    /// default, and a default that silently revises a decision somebody already made is worse
    /// than no default at all. At most one member of a group carries it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// The quality of this mirror, as the source named it or as its release name spells it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// The language of this mirror, on the same terms as [`Self::quality`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}
