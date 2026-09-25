//! Resolves [`MediaFormatCriteria`] against a probed inventory into one yt-dlp `-f`
//! expression.
//!
//! Two properties matter more than anything else here:
//!
//! * **Determinism.** The same inventory and criteria must always produce the same choice,
//!   including when a re-probe returns the formats in a different order — which is why the
//!   ranking ends in a lexicographic `format_id` tiebreak and never compares floats.
//! * **Honesty about ffmpeg.** Without it, only progressive formats are reachable. That is
//!   enforced in exactly one place ([`resolve`]'s pool step) and mirrored by the UI from the
//!   same [`MediaCapabilities`] flag, rather than being re-derived anywhere else.

use rd_core::{
    CriterionKind, CriterionMatch, MediaCompatibilityWarning, MediaFormat, MediaFormatCriteria,
    MediaFormatInventory, MediaFormatKind, MediaOutput, MediaResolution, MediaSelectionError,
    MediaStrictness, MediaTarget, RELAXATION_ORDER,
};

/// What the installed tools allow.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MediaCapabilities {
    /// ffmpeg *and* ffprobe are present, so a separate video and audio stream can be merged.
    pub can_merge: bool,
    /// Audio can be extracted and re-encoded (same requirement, named for what it enables).
    pub can_transcode_audio: bool,
}

impl MediaCapabilities {
    /// Everything available — the common case, and what the tests assume unless they care.
    #[must_use]
    pub const fn complete() -> Self {
        Self {
            can_merge: true,
            can_transcode_audio: true,
        }
    }

    /// yt-dlp only: no merging, no audio extraction.
    #[must_use]
    pub const fn ytdlp_only() -> Self {
        Self {
            can_merge: false,
            can_transcode_audio: false,
        }
    }
}

/// Containers preferred when nothing else separates two candidates. Lower index wins.
const CONTAINER_PREFERENCE: [&str; 4] = ["mp4", "mkv", "webm", "m4a"];

/// Picks the best format(s) for `criteria`, relaxing when allowed.
///
/// # Errors
///
/// Returns [`MediaSelectionError`] when the page offers nothing usable, when a video is
/// wanted but only mergeable streams exist and ffmpeg is missing, or when the criteria are
/// [`MediaStrictness::Required`] and match nothing.
pub fn resolve(
    inventory: &MediaFormatInventory,
    criteria: &MediaFormatCriteria,
    capabilities: MediaCapabilities,
) -> Result<MediaResolution, MediaSelectionError> {
    if inventory.is_empty() {
        return Err(MediaSelectionError::NoFormats);
    }
    let merge_allowed = criteria.allow_merge && capabilities.can_merge;
    let mut warnings = Vec::new();
    if inventory.truncated {
        warnings.push(MediaCompatibilityWarning::InventoryTruncated);
    }
    if criteria.allow_merge && !capabilities.can_merge {
        warnings.push(MediaCompatibilityWarning::MergeUnavailable);
    }

    // 1. Pool. Without merging, a video can only come from a progressive format — this is
    //    the single place that rule is enforced.
    let pool: Vec<&MediaFormat> = match criteria.target {
        MediaTarget::AudioOnly => inventory
            .formats
            .iter()
            .filter(|format| format.has_audio())
            .collect(),
        MediaTarget::Video if merge_allowed => inventory
            .formats
            .iter()
            .filter(|format| format.has_video())
            .collect(),
        MediaTarget::Video => inventory
            .of_kind(MediaFormatKind::Muxed)
            .collect::<Vec<_>>(),
    };
    if pool.is_empty() {
        return Err(if criteria.target == MediaTarget::Video && !merge_allowed {
            MediaSelectionError::MergeRequired
        } else {
            MediaSelectionError::NoFormats
        });
    }
    let candidate_total = pool.len();

    // 2. Filter, counting what each criterion would keep on its own so an empty result can
    //    be explained rather than merely reported.
    let active = active_criteria(criteria);
    let matched_counts: Vec<CriterionMatch> = active
        .iter()
        .map(|&criterion| CriterionMatch {
            criterion,
            matched: pool
                .iter()
                .filter(|format| matches_criterion(format, criteria, criterion))
                .count(),
        })
        .collect();

    // 3. Relax in a fixed order until something survives.
    let mut dropped: Vec<CriterionKind> = Vec::new();
    let mut kept = filtered(&pool, criteria, &active, &dropped);
    if kept.is_empty() {
        if criteria.strictness == MediaStrictness::Required {
            return Err(MediaSelectionError::NoMatch {
                unsatisfiable: matched_counts
                    .iter()
                    .filter(|entry| entry.matched == 0)
                    .map(|entry| entry.criterion)
                    .collect(),
                matched_counts,
                candidate_total,
            });
        }
        for criterion in RELAXATION_ORDER {
            if !active.contains(&criterion) {
                continue;
            }
            dropped.push(criterion);
            kept = filtered(&pool, criteria, &active, &dropped);
            if !kept.is_empty() {
                break;
            }
        }
    }
    let matched_total = kept.len();
    // Relaxing everything still leaves the pool, so this cannot fail — but the pool is
    // non-empty by construction and `first()` keeps that provable without an unwrap.
    let Some(video) = rank(&mut kept, criteria).first().copied().cloned() else {
        return Err(MediaSelectionError::NoFormats);
    };

    // 4. Pair with an audio stream when the chosen format carries none.
    let audio = if video.kind == MediaFormatKind::Video {
        best_audio(inventory, criteria)
    } else {
        None
    };
    if video.kind == MediaFormatKind::Video && audio.is_none() {
        // Merging is allowed here — the pool holds video-only streams only when it is — so
        // what is missing is the audio, not ffmpeg.
        return Err(MediaSelectionError::NoAudio);
    }

    let container = output_container(criteria, &video);
    warnings.extend(compatibility_warnings(&container, &video, audio.as_ref()));
    let estimated_bytes = match (
        video.filesize_approx,
        audio.as_ref().and_then(|format| format.filesize_approx),
    ) {
        (None, None) => None,
        (video_size, audio_size) => Some(
            video_size
                .unwrap_or(0)
                .saturating_add(audio_size.unwrap_or(0)),
        ),
    };

    Ok(MediaResolution {
        format_expression: expression(criteria, &video, audio.as_ref(), merge_allowed),
        container,
        estimated_bytes,
        relaxations: dropped,
        warnings,
        matched_counts,
        matched_total,
        candidate_total,
        video: Some(video),
        audio,
    })
}

/// The criteria that are actually set, in [`RELAXATION_ORDER`] so callers see a stable order.
fn active_criteria(criteria: &MediaFormatCriteria) -> Vec<CriterionKind> {
    RELAXATION_ORDER
        .into_iter()
        .filter(|criterion| match criterion {
            CriterionKind::DynamicRange => !criteria.dynamic_range.is_empty(),
            CriterionKind::VideoCodec => !criteria.video_codecs.is_empty(),
            CriterionKind::AudioCodec => !criteria.audio_codecs.is_empty(),
            CriterionKind::Container => !criteria.containers.is_empty(),
            CriterionKind::Fps => criteria.min_fps.is_some() || criteria.max_fps.is_some(),
            CriterionKind::Bitrate => criteria.max_total_bitrate_kbps.is_some(),
            CriterionKind::Language => !criteria.audio_languages.is_empty(),
            CriterionKind::Height => criteria.min_height.is_some() || criteria.max_height.is_some(),
        })
        .collect()
}

/// Whether `format` satisfies one criterion in isolation.
///
/// A criterion the format cannot speak to at all counts as satisfied — filtering a
/// video-only stream out because it declares no audio codec would make every merge
/// selection impossible.
fn matches_criterion(
    format: &MediaFormat,
    criteria: &MediaFormatCriteria,
    criterion: CriterionKind,
) -> bool {
    match criterion {
        CriterionKind::DynamicRange => {
            !format.has_video() || criteria.dynamic_range.contains(&format.dynamic_range)
        }
        CriterionKind::VideoCodec => format
            .video_codec
            .is_none_or(|codec| criteria.video_codecs.contains(&codec)),
        CriterionKind::AudioCodec => format
            .audio_codec
            .is_none_or(|codec| criteria.audio_codecs.contains(&codec)),
        CriterionKind::Container => {
            format.container.is_empty() || criteria.containers.contains(&format.container)
        }
        CriterionKind::Fps => format.fps.is_none_or(|fps| {
            criteria.min_fps.is_none_or(|min| fps >= min)
                && criteria.max_fps.is_none_or(|max| fps <= max)
        }),
        CriterionKind::Bitrate => format.effective_bitrate_kbps().is_none_or(|rate| {
            criteria
                .max_total_bitrate_kbps
                .is_none_or(|max| rate <= max)
        }),
        CriterionKind::Language => format.language.as_deref().is_none_or(|language| {
            criteria
                .audio_languages
                .iter()
                .any(|wanted| language_matches(language, wanted))
        }),
        CriterionKind::Height => format.height.is_none_or(|height| {
            criteria.min_height.is_none_or(|min| height >= min)
                && criteria.max_height.is_none_or(|max| height <= max)
        }),
    }
}

/// `de` matches `de-DE`; `de-DE` does not match `de-CH`.
fn language_matches(language: &str, wanted: &str) -> bool {
    language == wanted
        || language
            .strip_prefix(wanted)
            .is_some_and(|rest| rest.starts_with('-'))
}

/// The pool with every active, not-yet-dropped criterion applied.
fn filtered<'a>(
    pool: &[&'a MediaFormat],
    criteria: &MediaFormatCriteria,
    active: &[CriterionKind],
    dropped: &[CriterionKind],
) -> Vec<&'a MediaFormat> {
    pool.iter()
        .copied()
        .filter(|format| {
            active
                .iter()
                .filter(|criterion| !dropped.contains(criterion))
                .all(|&criterion| matches_criterion(format, criteria, criterion))
        })
        .collect()
}

/// Total order over candidates, richest first.
///
/// Every component is an integer and the last one is the format id, so the result does not
/// depend on the order the extractor happened to list the formats in — that is what makes a
/// re-probe keep the same choice.
fn rank<'a>(
    formats: &'a mut Vec<&'a MediaFormat>,
    criteria: &MediaFormatCriteria,
) -> &'a Vec<&'a MediaFormat> {
    formats.sort_by(|left, right| {
        right
            .height
            .unwrap_or(0)
            .cmp(&left.height.unwrap_or(0))
            .then_with(|| right.fps.unwrap_or(0).cmp(&left.fps.unwrap_or(0)))
            .then_with(|| right.dynamic_range.rank().cmp(&left.dynamic_range.rank()))
            .then_with(|| {
                right
                    .effective_bitrate_kbps()
                    .unwrap_or(0)
                    .cmp(&left.effective_bitrate_kbps().unwrap_or(0))
            })
            .then_with(|| {
                container_rank(&left.container, criteria)
                    .cmp(&container_rank(&right.container, criteria))
            })
            .then_with(|| left.format_id.cmp(&right.format_id))
    });
    formats
}

/// Lower is better: the user's own container order first, then a sane default.
fn container_rank(container: &str, criteria: &MediaFormatCriteria) -> usize {
    if let Some(index) = criteria
        .containers
        .iter()
        .position(|entry| entry == container)
    {
        return index;
    }
    CONTAINER_PREFERENCE
        .iter()
        .position(|entry| *entry == container)
        .map_or(CONTAINER_PREFERENCE.len() + 1, |index| {
            index + criteria.containers.len()
        })
}

/// The audio stream merged onto a video-only format.
fn best_audio(
    inventory: &MediaFormatInventory,
    criteria: &MediaFormatCriteria,
) -> Option<MediaFormat> {
    let pool: Vec<&MediaFormat> = inventory.of_kind(MediaFormatKind::Audio).collect();
    if pool.is_empty() {
        return None;
    }
    let audio_criteria = [CriterionKind::AudioCodec, CriterionKind::Language];
    let active: Vec<CriterionKind> = active_criteria(criteria)
        .into_iter()
        .filter(|criterion| audio_criteria.contains(criterion))
        .collect();
    // Preferred criteria; an audio filter that matches nothing must not lose the track.
    let mut kept = filtered(&pool, criteria, &active, &[]);
    if kept.is_empty() {
        kept = pool;
    }
    kept.sort_by(|left, right| {
        right
            .audio_bitrate_kbps
            .unwrap_or(0)
            .cmp(&left.audio_bitrate_kbps.unwrap_or(0))
            .then_with(|| left.format_id.cmp(&right.format_id))
    });
    kept.first().copied().cloned()
}

/// The container the finished file will carry.
fn output_container(criteria: &MediaFormatCriteria, video: &MediaFormat) -> String {
    criteria
        .output_container()
        .map_or_else(|| video.container.clone(), std::borrow::ToOwned::to_owned)
}

/// What the chosen combination cannot honour.
fn compatibility_warnings(
    container: &str,
    video: &MediaFormat,
    audio: Option<&MediaFormat>,
) -> Vec<MediaCompatibilityWarning> {
    let mut warnings = Vec::new();
    if let Some(codec) = video.video_codec
        && !rd_core::supports_video_codec(container, codec)
    {
        warnings.push(MediaCompatibilityWarning::CodecContainerMismatch {
            codec: video
                .video_codec_raw
                .clone()
                .unwrap_or_else(|| format!("{codec:?}")),
            container: container.to_owned(),
        });
    }
    let audio_codec = audio
        .and_then(|format| format.audio_codec)
        .or(video.audio_codec);
    if let Some(codec) = audio_codec
        && !rd_core::supports_audio_codec(container, codec)
    {
        warnings.push(MediaCompatibilityWarning::CodecContainerMismatch {
            codec: audio
                .and_then(|format| format.audio_codec_raw.clone())
                .or_else(|| video.audio_codec_raw.clone())
                .unwrap_or_else(|| format!("{codec:?}")),
            container: container.to_owned(),
        });
    }
    warnings
}

/// Builds the `-f` expression: pinned ids, then the same choice expressed semantically,
/// then a merge-free last resort.
///
/// yt-dlp evaluates `/` alternatives left to right, so a still-valid pinned id wins, a
/// rotated one degrades silently to the semantic expression, and the final alternative
/// always works without ffmpeg. That last property is what lets
/// `runner::progressive_format` keep doing its job unchanged.
fn expression(
    criteria: &MediaFormatCriteria,
    video: &MediaFormat,
    audio: Option<&MediaFormat>,
    merge_allowed: bool,
) -> String {
    let mut alternatives = Vec::new();

    let pinned = match audio {
        Some(audio) if !video.format_id.is_empty() && !audio.format_id.is_empty() => {
            Some(format!("{}+{}", video.format_id, audio.format_id))
        }
        None if !video.format_id.is_empty() => Some(video.format_id.clone()),
        _ => None,
    };
    alternatives.extend(pinned);

    let bounds = numeric_bounds(criteria, video);
    if criteria.target == MediaTarget::AudioOnly {
        let mut selector = String::from("ba");
        if let Some(codec) = video.audio_codec_raw.as_deref() {
            selector.push_str(&format!("[acodec^={}]", codec_head(codec)));
        }
        alternatives.push(selector);
        alternatives.push("ba".to_owned());
        alternatives.push("b".to_owned());
    } else {
        let mut video_selector = format!("bv*{bounds}");
        if let Some(codec) = video.video_codec_raw.as_deref() {
            video_selector.push_str(&format!("[vcodec^={}]", codec_head(codec)));
        }
        if merge_allowed && audio.is_some() {
            alternatives.push(format!("{video_selector}+ba"));
        }
        alternatives.push(format!("b{bounds}"));
        alternatives.push("b".to_owned());
    }

    let mut seen = Vec::new();
    for alternative in alternatives {
        if !seen.contains(&alternative) {
            seen.push(alternative);
        }
    }
    seen.join("/")
}

/// yt-dlp filter clauses for the numeric bounds, taken from the criteria and tightened to
/// the chosen format's own height so a rotated id cannot resolve to something larger.
fn numeric_bounds(criteria: &MediaFormatCriteria, video: &MediaFormat) -> String {
    let mut clauses = String::new();
    let max_height = match (criteria.max_height, video.height) {
        (Some(limit), Some(actual)) => Some(limit.min(actual)),
        (limit, actual) => limit.or(actual),
    };
    if let Some(height) = max_height {
        clauses.push_str(&format!("[height<={height}]"));
    }
    if let Some(height) = criteria.min_height {
        clauses.push_str(&format!("[height>={height}]"));
    }
    if let Some(fps) = criteria.max_fps {
        clauses.push_str(&format!("[fps<={fps}]"));
    }
    if let Some(fps) = criteria.min_fps {
        clauses.push_str(&format!("[fps>={fps}]"));
    }
    if let Some(rate) = criteria.max_total_bitrate_kbps {
        clauses.push_str(&format!("[tbr<={rate}]"));
    }
    clauses
}

/// The part of a codec string before its profile suffix, for a `^=` prefix match.
///
/// Safe by construction rather than by escaping: everything but `[a-z0-9]` is dropped, so
/// nothing a site puts in `vcodec` can close a filter bracket.
fn codec_head(codec: &str) -> String {
    codec
        .split('.')
        .next()
        .unwrap_or(codec)
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// The audio quality yt-dlp should extract at, when the output asks for extraction.
#[must_use]
pub fn extract_audio(criteria: &MediaFormatCriteria) -> Option<(&str, u8)> {
    match &criteria.output {
        MediaOutput::ExtractAudio { codec, quality } => Some((codec.as_str(), *quality)),
        MediaOutput::Passthrough | MediaOutput::Remux { .. } => None,
    }
}

/// The container yt-dlp should merge into, when the output asks for a remux.
#[must_use]
pub fn remux_container(criteria: &MediaFormatCriteria) -> Option<&str> {
    match &criteria.output {
        MediaOutput::Remux { container } => Some(container.as_str()),
        MediaOutput::Passthrough | MediaOutput::ExtractAudio { .. } => None,
    }
}
