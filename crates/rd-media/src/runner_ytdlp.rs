//! The yt-dlp side of a media run: the `-f` expression and the literal output path.

use std::{path::PathBuf, time::Duration};

use rd_core::{Failure, FailureKind, MediaFormatCriteria, MediaKind, MediaSelection};

use crate::{
    args::FINAL_PATH_MARKER,
    select::{MediaCapabilities, resolve},
};

/// The finished file's path out of one yt-dlp stdout line, or `None` for every other line.
///
/// yt-dlp is asked to print the path behind [`FINAL_PATH_MARKER`], so the answer is
/// recognisable. The rule used to be "any non-empty line that does not start with `[`",
/// which meant a single unprefixed line from an extractor — a warning, a merge note, a
/// plugin printing to stdout — overwrote the value, and the runner then reported a
/// `final_name` that had never been written to the queue and to post-processing.
pub(super) fn final_path_line(line: &str) -> Option<&str> {
    let value = line.trim().strip_prefix(FINAL_PATH_MARKER)?.trim();
    (!value.is_empty()).then_some(value)
}

/// Room kept free after the stem for what yt-dlp appends while downloading, e.g.
/// `.f2120404511882385v.mp4.part` for a per-format fragment of a merged video.
pub(super) const YTDLP_SUFFIX_RESERVE: usize = 40;

/// Drops the `video+audio` alternatives from a yt-dlp format expression, keeping only the
/// last (pre-muxed) alternative so no merge is required.
///
/// `bv*[height<=1080]+ba/b[height<=1080]` becomes `b[height<=1080]`; an expression that
/// offers no such alternative falls back to `b`.
pub(super) fn progressive_format(format: &str) -> String {
    format
        .split('/')
        .map(str::trim)
        .rfind(|alternative| !alternative.is_empty() && !alternative.contains('+'))
        .unwrap_or("b")
        .to_owned()
}

/// Where yt-dlp writes the finished file.
///
/// The result is a literal path plus yt-dlp's extension placeholder, and nothing else. Our
/// own template grammar is expanded here, *before* the value is handed over, so none of
/// yt-dlp's `%(field)s` syntax is reachable from anything a site or a user supplied — the
/// `-o` argument stays an opaque literal.
///
/// A template that cannot be expanded — an unknown field, traversal from a hostile title,
/// nothing left after substitution — falls back to the plain file name rather than failing
/// the download. The template was already validated when it was saved, so reaching this is
/// a data problem with one particular page, not a configuration error worth stopping for.
pub(super) fn output_template(
    directory: &std::path::Path,
    stem: &str,
    template: Option<&str>,
    values: &rd_files::TemplateValues,
) -> PathBuf {
    let plain = || with_ext_placeholder(&directory.join(stem));
    let Some(template) = template.map(str::trim).filter(|value| !value.is_empty()) else {
        return plain();
    };
    match rd_files::expand(directory, template, values, YTDLP_SUFFIX_RESERVE) {
        Ok(path) => with_ext_placeholder(&path),
        Err(error) => {
            tracing::warn!(%error, "output template could not be expanded; using the file name");
            plain()
        }
    }
}

/// Appends yt-dlp's extension placeholder to a path that has to stay literal.
///
/// Every `%` already in the path is doubled first. `rd_files::sanitize_file_name` replaces
/// the characters a file system objects to — `<>:"/\|?*` and the control characters — and
/// leaves `%` alone, because a per cent sign is a perfectly ordinary character in a file
/// name. It is not an ordinary character to yt-dlp: without this, a page titled
/// `50%(title)s off` reaches it as a real output template, so the file lands under a name
/// nobody chose or the job fails outright on an unknown field. `%%` is yt-dlp's escape for a
/// literal per cent and collapses back to one character on disk, which is also why doubling
/// does not overrun the length budget `rd_files::sanitize_file_name_within` reserved: the
/// argument grows, the file that gets written does not.
///
/// The escaping belongs here rather than in `rd-files`: the rule is yt-dlp's, not a general
/// file-name rule, and every other consumer of a sanitised name wants the `%` left alone.
fn with_ext_placeholder(path: &std::path::Path) -> PathBuf {
    let mut argument = match path.to_str() {
        Some(text) => std::ffi::OsString::from(text.replace('%', "%%")),
        // A path that is not valid UTF-8 cannot be rewritten without losing bytes, so it is
        // handed over as it stands rather than mangled. Every path this crate builds comes
        // out of the sanitiser, so this is the theoretical branch.
        None => path.as_os_str().to_owned(),
    };
    argument.push(".%(ext)s");
    PathBuf::from(argument)
}

/// The values an output template is expanded against.
///
/// Deliberately narrow: exactly the allowlisted fields, taken from what the selection
/// already carries. Nothing here reaches back into the extractor's raw metadata.
pub(super) fn template_values(selection: &MediaSelection, stem: &str) -> rd_files::TemplateValues {
    let mut values = rd_files::TemplateValues::new();
    let title = if selection.title.trim().is_empty() {
        stem.to_owned()
    } else {
        selection.title.clone()
    };
    values.insert("title".to_owned(), title);
    values.insert("ext".to_owned(), selection.ext.clone());
    if let Some(resolved) = selection.resolved.as_deref() {
        if !resolved.label.is_empty() {
            values.insert("resolution".to_owned(), resolved.label.clone());
        }
        if !resolved.container.is_empty() {
            values.insert("ext".to_owned(), resolved.container.clone());
        }
    }
    values
}

/// The `-f` expression for one download.
///
/// A [`MediaStrictness::Preferred`] selection rides on the stored expression: it already
/// lists the pinned ids, the same choice expressed semantically, and a merge-free last
/// resort, which yt-dlp evaluates left to right. Only a `Required` selection re-probes,
/// because only there does silently accepting the next alternative amount to handing over
/// something the user explicitly refused.
pub(super) async fn resolve_format(
    ytdlp: &std::path::Path,
    settings: &rd_core::MediaSettings,
    selection: &MediaSelection,
    criteria: Option<&MediaFormatCriteria>,
    capabilities: MediaCapabilities,
) -> Result<String, Failure> {
    let Some(criteria) =
        criteria.filter(|criteria| criteria.strictness == rd_core::MediaStrictness::Required)
    else {
        return Ok(degrade(&selection.format, selection.kind, capabilities));
    };
    let timeout = Duration::from_secs(u64::from(settings.media_check_timeout_seconds.max(5)));
    let inventory = crate::probe::probe_inventory(ytdlp, &selection.page_url, timeout).await?;
    let resolution = resolve(&inventory, criteria, capabilities).map_err(|error| {
        let failure = Failure::coded(FailureKind::Permanent, error.code(), error.to_string());
        match &error {
            rd_core::MediaSelectionError::NoMatch { unsatisfiable, .. } => failure.with_param(
                "criteria",
                unsatisfiable
                    .iter()
                    .map(|criterion| criterion.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            rd_core::MediaSelectionError::NoFormats
            | rd_core::MediaSelectionError::MergeRequired
            | rd_core::MediaSelectionError::NoAudio => failure,
        }
    })?;
    Ok(resolution.format_expression)
}

/// Drops the merge alternatives when the tools cannot merge.
///
/// Without ffmpeg a merged format leaves two unusable stream files behind, so ask for a
/// single pre-muxed one instead. Warnings stay on — they carry exactly the "ffmpeg is not
/// installed" diagnostics that used to be swallowed.
fn degrade(format: &str, kind: MediaKind, capabilities: MediaCapabilities) -> String {
    if kind == MediaKind::Video && !capabilities.can_merge {
        progressive_format(format)
    } else {
        format.to_owned()
    }
}
