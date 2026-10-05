//! Which files of a finished package a sort places where (RD-1100-08).
//!
//! Pure: names in, moves out. The pipeline step in `rd-extract` does the moving, under the
//! collision policy and with the target checked against the root on disk.
//!
//! * A video is recognised from its own name, else from the folder it sits in, else — when it
//!   is the package's only video — from the package name, which is what an obfuscated file
//!   (`a8f3c2.mkv`) in a well-named package needs.
//! * A companion (subtitle, NFO) whose name starts with a video's name follows that video and
//!   keeps the rest of its name: `Show.S01E01.en.srt` becomes `<new name>.en.srt`. In a package
//!   with one video every companion follows it: the NFO as `<new name>.nfo`, a subtitle as
//!   `<new name>.<its old name>`, so `English.srt` keeps the language it was named after.
//! * Everything else stays where it is: unrecognised videos, kinds without a template, other
//!   files, and anything a template refused for this name.

use std::{collections::HashSet, path::Path, path::PathBuf};

use rd_core::{SortKind, SortTemplates};

use crate::{
    SortTemplateError, expand_sort_template, recognize_release, sanitize_file_name, sort_extension,
    sort_name::SORT_VIDEO_EXTENSIONS,
};

/// One file to move: from its name inside the package (`/` between folders) to its place.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SortMove {
    pub from: String,
    pub to: PathBuf,
    pub kind: SortKind,
}

/// What a sort of one package will do.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SortPlan {
    pub moves: Vec<SortMove>,
    /// Videos that stay: not recognised, or of a kind the category has no template for.
    pub unsorted: Vec<String>,
    /// Videos a template refused for their name, with the reason.
    pub refused: Vec<(String, SortTemplateError)>,
}

/// Plans the sort of a package whose files are `files`, relative to the package folder.
#[must_use]
pub fn plan_sort(
    root: &Path,
    package_name: &str,
    files: &[String],
    templates: &SortTemplates,
) -> SortPlan {
    let is_video = |name: &str| {
        sort_extension(name).is_some_and(|ext| SORT_VIDEO_EXTENSIONS.contains(&ext.as_str()))
    };
    let mut videos: Vec<&String> = files.iter().filter(|name| is_video(name)).collect();
    let companions: Vec<&String> = files
        .iter()
        .filter(|name| !is_video(name) && sort_extension(name).is_some())
        .collect();
    let single = videos.len() == 1;
    // The longest name first, so `Show.S01E01.Extended.srt` follows `Show.S01E01.Extended.mkv`
    // rather than `Show.S01E01.mkv`.
    videos.sort_by_key(|name| std::cmp::Reverse(name.len()));
    let mut plan = SortPlan::default();
    let mut claimed: HashSet<&String> = HashSet::new();
    for video in videos {
        let (folder, file) = split(video);
        let found = recognize_release(file)
            .or_else(|| folder.rsplit('/').next().and_then(recognize_release))
            .or_else(|| single.then(|| recognize_release(package_name)).flatten());
        let Some(found) = found else {
            plan.unsorted.push(video.clone());
            continue;
        };
        let Some(template) = templates.for_kind(found.kind) else {
            plan.unsorted.push(video.clone());
            continue;
        };
        let target = match expand_sort_template(root, template, &found) {
            Ok(target) => target,
            Err(error) => {
                plan.refused.push((video.clone(), error));
                continue;
            }
        };
        let (video_stem, extension) = file.rsplit_once('.').unwrap_or((file, ""));
        // Companions first, the video last: a sort stopped between two moves finds the video
        // still in the package and recognises it again, and its companions with it — a
        // companion left behind on its own would follow nothing.
        for companion in &companions {
            if claimed.contains(*companion) {
                continue;
            }
            let (companion_folder, companion_file) = split(companion);
            let suffix = match follows(companion_file, video_stem) {
                Some(rest) if companion_folder == folder => rest.to_owned(),
                _ if single => single_suffix(companion_file),
                _ => continue,
            };
            claimed.insert(*companion);
            plan.moves.push(SortMove {
                from: (*companion).clone(),
                to: target
                    .directory
                    .join(sanitize_file_name(&format!("{}{suffix}", target.stem))),
                kind: found.kind,
            });
        }
        plan.moves.push(SortMove {
            from: video.clone(),
            to: target
                .directory
                .join(sanitize_file_name(&format!("{}.{extension}", target.stem))),
            kind: found.kind,
        });
    }
    plan
}

/// `("Season 1", "Show.S01E01.mkv")` from `Season 1/Show.S01E01.mkv`.
fn split(name: &str) -> (&str, &str) {
    name.rsplit_once('/').unwrap_or(("", name))
}

/// The rest of `companion` after `stem`, `.en.srt`, when it begins with the video's name.
fn follows<'a>(companion: &'a str, stem: &str) -> Option<&'a str> {
    let length = stem.len();
    if companion.len() <= length || !companion.is_char_boundary(length) {
        return None;
    }
    let (head, rest) = companion.split_at(length);
    (head.eq_ignore_ascii_case(stem) && rest.starts_with('.')).then_some(rest)
}

/// How a companion that does not share the video's name follows the only video.
fn single_suffix(companion: &str) -> String {
    match sort_extension(companion).as_deref() {
        Some("nfo") => ".nfo".to_owned(),
        _ => format!(".{companion}"),
    }
}
