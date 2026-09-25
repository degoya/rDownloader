//! What gets written *into* the finished file, and what gets cut out of it.
//!
//! Both halves are opt-in and both are allowlisted. Embedding is irreversible in practice —
//! nobody re-downloads a file to strip a wrong tag — so every piece is toggled on its own
//! rather than hidden behind one "add metadata" switch. SponsorBlock is stricter still:
//! removing segments re-cuts the media, and the segment boundaries come from a public
//! crowd-sourced database that is occasionally wrong, so it is off unless asked for and
//! marking chapters is offered as the non-destructive alternative.

use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use super::criteria::CriteriaError;

/// A SponsorBlock segment category. A closed set: these are the identifiers the public API
/// defines, and anything else would be passed to yt-dlp unvalidated.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SponsorCategory {
    Sponsor,
    Intro,
    Outro,
    SelfPromo,
    Preview,
    Filler,
    Interaction,
    MusicOfftopic,
}

impl SponsorCategory {
    /// The identifier yt-dlp and the SponsorBlock API use.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sponsor => "sponsor",
            Self::Intro => "intro",
            Self::Outro => "outro",
            Self::SelfPromo => "selfpromo",
            Self::Preview => "preview",
            Self::Filler => "filler",
            Self::Interaction => "interaction",
            Self::MusicOfftopic => "music_offtopic",
        }
    }

    /// Every category, for the UI to list.
    #[must_use]
    pub const fn all() -> [Self; 8] {
        [
            Self::Sponsor,
            Self::Intro,
            Self::Outro,
            Self::SelfPromo,
            Self::Preview,
            Self::Filler,
            Self::Interaction,
            Self::MusicOfftopic,
        ]
    }
}

/// What to do with the segments SponsorBlock reports.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SponsorMode {
    /// Nothing is queried and nothing is changed.
    #[default]
    Off,
    /// Segments become chapter markers; the media is untouched.
    Mark,
    /// Segments are cut out, re-encoding the file.
    Remove,
}

impl SponsorMode {
    #[must_use]
    pub const fn is_off(self) -> bool {
        matches!(self, Self::Off)
    }

    /// Whether the media itself is altered.
    #[must_use]
    pub const fn is_destructive(self) -> bool {
        matches!(self, Self::Remove)
    }
}

/// SponsorBlock handling for one job.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct SponsorBlockPolicy {
    pub mode: SponsorMode,
    /// Categories to act on; empty with an active mode means `sponsor` only, never "all" —
    /// a mode toggle should not silently start cutting intros as well.
    pub categories: Vec<SponsorCategory>,
}

impl SponsorBlockPolicy {
    /// The categories actually acted on.
    #[must_use]
    pub fn effective_categories(&self) -> Vec<SponsorCategory> {
        if self.mode.is_off() {
            Vec::new()
        } else if self.categories.is_empty() {
            vec![SponsorCategory::Sponsor]
        } else {
            let mut categories = self.categories.clone();
            categories.dedup();
            categories
        }
    }
}

/// What to write into the finished file.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct MediaEmbedPolicy {
    /// Cover image from the page's thumbnail.
    pub thumbnail: bool,
    /// Chapter markers the extractor reported.
    pub chapters: bool,
    /// Title, uploader, date and the rest of the tag set.
    pub metadata: bool,
    /// The extractor's full info JSON, which carries the description and the source URL.
    pub info_json: bool,
    pub sponsorblock: SponsorBlockPolicy,
}

impl MediaEmbedPolicy {
    /// Whether anything at all is written or cut.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.thumbnail
            && !self.chapters
            && !self.metadata
            && !self.info_json
            && self.sponsorblock.mode.is_off()
    }

    /// Validates the policy. Nothing here is free-form, so this only bounds the list.
    pub fn sanitized(mut self) -> Result<Self, CriteriaError> {
        if self.sponsorblock.categories.len() > SponsorCategory::all().len() {
            return Err(CriteriaError::TooMany {
                field: "sponsorblock.categories",
            });
        }
        self.sponsorblock.categories.dedup();
        Ok(self)
    }
}

/// Something an embed policy asks for that cannot be delivered as stated.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EmbedWarning {
    /// The container cannot hold a cover image.
    ThumbnailUnsupported { container: String },
    /// The container cannot hold chapter markers.
    ChaptersUnsupported { container: String },
    /// The source URL carries a signature or token, so it is not written into the file.
    SourceUrlWithheld,
    /// ffmpeg is needed to write tags or cut segments and is not available.
    ToolUnavailable,
    /// Removing segments re-encodes the media; marking them does not.
    SponsorBlockRewritesMedia,
}

/// Checks an embed policy against the container, the tools, and the source URL.
///
/// The source-URL check is the load-bearing one. A signed URL carries a credential in its
/// query — that is what makes it signed — and embedding it writes that credential into a
/// file that then gets shared, uploaded, or handed to a post-processing script. The
/// info JSON is therefore withheld for such links rather than sanitised, because we do not
/// control what the extractor puts in it.
#[must_use]
pub fn embed_warnings(
    policy: &MediaEmbedPolicy,
    container: &str,
    page_url: &Url,
    can_transcode: bool,
) -> Vec<EmbedWarning> {
    let mut warnings = Vec::new();
    if policy.thumbnail && !super::container::supports_thumbnail(container) {
        warnings.push(EmbedWarning::ThumbnailUnsupported {
            container: container.to_owned(),
        });
    }
    if policy.chapters && !super::container::supports_chapters(container) {
        warnings.push(EmbedWarning::ChaptersUnsupported {
            container: container.to_owned(),
        });
    }
    if policy.info_json && crate::redact::is_signed_url(page_url) {
        warnings.push(EmbedWarning::SourceUrlWithheld);
    }
    if !can_transcode && !policy.is_empty() {
        warnings.push(EmbedWarning::ToolUnavailable);
    }
    if policy.sponsorblock.mode.is_destructive() {
        warnings.push(EmbedWarning::SponsorBlockRewritesMedia);
    }
    warnings
}

/// The policy actually applied, with whatever the container or the URL forbids removed.
///
/// Returned rather than enforced at the argument builder so the UI can show the same result
/// before anything is queued.
#[must_use]
pub fn effective_policy(
    policy: &MediaEmbedPolicy,
    container: &str,
    page_url: &Url,
    can_transcode: bool,
) -> MediaEmbedPolicy {
    if !can_transcode {
        return MediaEmbedPolicy::default();
    }
    MediaEmbedPolicy {
        thumbnail: policy.thumbnail && super::container::supports_thumbnail(container),
        chapters: policy.chapters && super::container::supports_chapters(container),
        metadata: policy.metadata,
        info_json: policy.info_json && !crate::redact::is_signed_url(page_url),
        sponsorblock: policy.sponsorblock.clone(),
    }
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::{
        EmbedWarning, MediaEmbedPolicy, SponsorBlockPolicy, SponsorCategory, SponsorMode,
        effective_policy, embed_warnings,
    };

    fn url(value: &str) -> Url {
        value.parse().expect("url")
    }

    fn everything() -> MediaEmbedPolicy {
        MediaEmbedPolicy {
            thumbnail: true,
            chapters: true,
            metadata: true,
            info_json: true,
            sponsorblock: SponsorBlockPolicy::default(),
        }
    }

    #[test]
    fn a_signed_source_url_is_never_written_into_the_file() {
        // The signature *is* the credential. Embedding it puts it in a file that gets
        // shared, uploaded, or handed to a post-processing script.
        let signed = url("https://cdn.example.test/v.mp4?Expires=1&Signature=abc&Key-Pair-Id=k");
        assert!(
            embed_warnings(&everything(), "mp4", &signed, true)
                .contains(&EmbedWarning::SourceUrlWithheld)
        );
        assert!(!effective_policy(&everything(), "mp4", &signed, true).info_json);

        let plain = url("https://www.youtube.com/watch?v=abc");
        assert!(
            !embed_warnings(&everything(), "mp4", &plain, true)
                .contains(&EmbedWarning::SourceUrlWithheld)
        );
        assert!(effective_policy(&everything(), "mp4", &plain, true).info_json);
    }

    #[test]
    fn container_limits_are_reported_and_applied_consistently() {
        let plain = url("https://example.test/v");
        let warnings = embed_warnings(&everything(), "webm", &plain, true);
        assert!(warnings.contains(&EmbedWarning::ThumbnailUnsupported {
            container: "webm".to_owned()
        }));
        // WebM does hold chapters, so only the thumbnail is refused.
        assert!(
            !warnings
                .iter()
                .any(|warning| matches!(warning, EmbedWarning::ChaptersUnsupported { .. }))
        );
        let applied = effective_policy(&everything(), "webm", &plain, true);
        assert!(!applied.thumbnail, "the warning and the result must agree");
        assert!(applied.chapters);
    }

    #[test]
    fn nothing_is_embedded_without_ffmpeg() {
        let plain = url("https://example.test/v");
        assert!(
            embed_warnings(&everything(), "mp4", &plain, false)
                .contains(&EmbedWarning::ToolUnavailable)
        );
        assert!(effective_policy(&everything(), "mp4", &plain, false).is_empty());
        // An empty policy has nothing to warn about.
        assert!(embed_warnings(&MediaEmbedPolicy::default(), "mp4", &plain, false).is_empty());
    }

    #[test]
    fn sponsorblock_is_off_by_default_and_defaults_to_sponsor_segments_only() {
        let policy = SponsorBlockPolicy::default();
        assert!(policy.mode.is_off());
        assert!(policy.effective_categories().is_empty());

        // Turning it on must not silently start cutting intros as well.
        let marking = SponsorBlockPolicy {
            mode: SponsorMode::Mark,
            categories: Vec::new(),
        };
        assert_eq!(
            marking.effective_categories(),
            vec![SponsorCategory::Sponsor]
        );
        assert!(!marking.mode.is_destructive());
    }

    #[test]
    fn removing_segments_says_that_it_rewrites_the_media() {
        let policy = MediaEmbedPolicy {
            sponsorblock: SponsorBlockPolicy {
                mode: SponsorMode::Remove,
                categories: vec![SponsorCategory::Sponsor],
            },
            ..MediaEmbedPolicy::default()
        };
        assert!(
            embed_warnings(&policy, "mp4", &url("https://example.test/v"), true)
                .contains(&EmbedWarning::SponsorBlockRewritesMedia)
        );
        // Marking is the non-destructive alternative and says nothing.
        let marking = MediaEmbedPolicy {
            sponsorblock: SponsorBlockPolicy {
                mode: SponsorMode::Mark,
                categories: vec![SponsorCategory::Sponsor],
            },
            ..MediaEmbedPolicy::default()
        };
        assert!(
            !embed_warnings(&marking, "mp4", &url("https://example.test/v"), true)
                .contains(&EmbedWarning::SponsorBlockRewritesMedia)
        );
    }

    #[test]
    fn category_identifiers_match_the_public_api() {
        assert_eq!(SponsorCategory::SelfPromo.as_str(), "selfpromo");
        assert_eq!(SponsorCategory::MusicOfftopic.as_str(), "music_offtopic");
        assert_eq!(SponsorCategory::all().len(), 8);
    }
}
