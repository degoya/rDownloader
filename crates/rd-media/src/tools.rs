//! Locating and describing the external executables.

use std::path::{Path, PathBuf};

use rd_core::{MediaSettings, ResolvedTool, ToolSource};
use rd_tools::{Assessment, Capability, compat};
use serde::Serialize;

use crate::select::MediaCapabilities;

/// Where a tool was found, which version it reports and what the compatibility rules make of
/// that version.
#[derive(Clone, Debug, Serialize)]
pub struct ToolStatus {
    pub name: String,
    pub path: Option<String>,
    /// The first line the tool printed, verbatim. Kept raw because a line no parser
    /// recognised is still the most useful thing to put in front of a person.
    pub version: Option<String>,
    /// Explicit setting, vendor folder or `PATH`; `None` when the tool was not found.
    pub source: Option<ToolSource>,
    /// The verdict, the floor and the capabilities at stake (RD-102-03). `Unknown` with no
    /// capabilities for a tool no rule has an opinion about.
    #[serde(skip)]
    pub compatibility: Assessment,
}

/// Explicit absolute path when configured, else the managed store, a vendor folder or `PATH`.
#[must_use]
pub fn locate_tool(explicit: Option<&str>, vendor: Option<&str>, name: &str) -> Option<PathBuf> {
    rd_core::locate_tool(explicit, vendor, name).map(|tool| tool.path)
}

/// [`locate_tool`], keeping the lease a caller that is about to *run* the binary must hold.
///
/// A managed tool version is not removed while a lease on it is alive, so a running job keeps
/// the binary it started with even when another version is activated underneath it
/// (RD-102-02). The lease is `None` for an explicit, vendor or `PATH` binary, which the store
/// does not own and therefore never removes.
#[must_use]
pub(crate) fn lease_tool(
    explicit: Option<&str>,
    vendor: Option<&str>,
    name: &str,
) -> Option<(PathBuf, Option<rd_core::ToolLease>)> {
    rd_core::locate_tool_leased(explicit, vendor, name).map(|(tool, lease)| (tool.path, lease))
}

/// ffmpeg and ffprobe as yt-dlp needs them.
///
/// yt-dlp treats `--ffmpeg-location` as authoritative and looks for *both* binaries there
/// without falling back to `PATH`. Pointing it at a directory that only holds ffmpeg makes
/// post-processing fail with "ffprobe and ffmpeg not found", so a directory is only handed
/// over when both live in it. The managed store keeps every tool in its own version folder,
/// so ffmpeg and ffprobe from it never share one; that pair is still the one used — the
/// versions this installation set up win over any folder that happens to hold both, and only
/// an ffmpeg path the person set wins over them (owner, 2026-10-10, RD-1240-33) — and
/// [`FfmpegTools::location`] names the ffmpeg binary itself, which yt-dlp also accepts. Passing nothing at all left yt-dlp without ffmpeg, and every
/// merged format came out as two separate streams ("ffmpeg is not installed. The formats
/// won't be merged").
#[derive(Clone, Debug)]
pub struct FfmpegTools {
    pub ffmpeg: Option<ResolvedTool>,
    pub ffprobe: Option<ResolvedTool>,
    /// Leases on the managed versions these paths belong to, held for as long as this value
    /// lives (RD-102-02). Empty when neither binary came from the managed store.
    ///
    /// Held and then dropped, never read: that is the whole mechanism, and it is precisely
    /// the shape `dead_code` cannot see.
    #[allow(dead_code)]
    leases: Vec<rd_core::ToolLease>,
}

impl FfmpegTools {
    /// Resolves both binaries from the media settings.
    #[must_use]
    pub fn resolve(settings: &MediaSettings) -> Self {
        let vendor = settings.vendor_directory.as_deref();
        let ffmpeg = rd_core::locate_tool_leased(
            settings.media_ffmpeg_executable.as_deref(),
            vendor,
            "ffmpeg",
        );
        let ffprobe = rd_core::locate_tool_leased(None, vendor, "ffprobe");
        let path_directories = std::env::var_os("PATH")
            .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
            .unwrap_or_default();
        let shared = rd_core::vendor_directories(vendor)
            .into_iter()
            .map(|directory| (directory, ToolSource::Vendor))
            .chain(
                path_directories
                    .into_iter()
                    .map(|directory| (directory, ToolSource::Path)),
            );
        Self::pair(ffmpeg, ffprobe, shared)
    }

    /// Settles on the binaries to use, keeping the lookup's precedence unless it split them.
    ///
    /// `shared` are the directories, in lookup order, that may supply both binaries at once.
    fn pair(
        mut ffmpeg: Option<(ResolvedTool, Option<rd_core::ToolLease>)>,
        mut ffprobe: Option<(ResolvedTool, Option<rd_core::ToolLease>)>,
        shared: impl IntoIterator<Item = (PathBuf, ToolSource)>,
    ) -> Self {
        // A configured ffmpeg path implies its sibling ffprobe: users point the setting at
        // one binary, not at both. The same holds for any ffmpeg that has one next to it.
        if let Some((tool, _)) = &ffmpeg
            && !same_directory(tool, ffprobe.as_ref())
            && let Some(path) = sibling(&tool.path, "ffprobe")
        {
            ffprobe = Some((
                ResolvedTool {
                    path,
                    source: tool.source,
                },
                None,
            ));
        }
        // Still split: a directory holding both beats the split pair — unless ffmpeg is the
        // person's own path, or both come from the managed store, one folder per tool. The
        // managed pair takes precedence over a vendor or `PATH` folder holding both (owner,
        // 2026-10-10, RD-1240-33: `/usr/bin` won over an installed and active managed pair).
        // A managed ffmpeg beside an ffprobe from elsewhere is still split and paired as before.
        let split = match (&ffmpeg, &ffprobe) {
            (Some((tool, _)), Some((probe, _))) => {
                let managed_pair =
                    tool.source == ToolSource::Managed && probe.source == ToolSource::Managed;
                tool.source != ToolSource::Explicit
                    && !managed_pair
                    && !same_directory(tool, ffprobe.as_ref())
            }
            _ => false,
        };
        if split
            && let Some((ffmpeg_path, ffprobe_path, source)) =
                shared.into_iter().find_map(|(directory, source)| {
                    Some((
                        rd_core::executable_in(&directory, "ffmpeg")?,
                        rd_core::executable_in(&directory, "ffprobe")?,
                        source,
                    ))
                })
        {
            ffmpeg = Some((
                ResolvedTool {
                    path: ffmpeg_path,
                    source,
                },
                None,
            ));
            ffprobe = Some((
                ResolvedTool {
                    path: ffprobe_path,
                    source,
                },
                None,
            ));
        }
        let mut leases = Vec::new();
        let mut keep = |found: Option<(ResolvedTool, Option<rd_core::ToolLease>)>| {
            found.map(|(tool, lease)| {
                leases.extend(lease);
                tool
            })
        };
        let ffmpeg = keep(ffmpeg);
        let ffprobe = keep(ffprobe);
        Self {
            ffmpeg,
            ffprobe,
            leases,
        }
    }

    /// Whether MP3 conversion and video/audio merging can run at all.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.ffmpeg.is_some() && self.ffprobe.is_some()
    }

    /// What these binaries actually allow, once their versions have been judged.
    ///
    /// The same flag the interface reads, derived in one place: "is ffmpeg complete?" was
    /// already the single source for merging and MP3 output, and RD-102-03 only adds "and is
    /// the version one this build supports?" to it rather than inventing a second gate.
    pub async fn media_capabilities(&self) -> MediaCapabilities {
        // Without a location yt-dlp cannot find ffmpeg, and a merge it was asked for would
        // leave the streams as separate files.
        if !self.is_complete() || self.location().is_none() {
            return MediaCapabilities::ytdlp_only();
        }
        let mut capabilities = MediaCapabilities::complete();
        for assessment in self.assessments().await {
            capabilities.can_merge &= !assessment.blocks(Capability::MediaMerge);
            capabilities.can_transcode_audio &= !assessment.blocks(Capability::AudioExtraction);
        }
        capabilities
    }

    /// The compatibility verdict of each binary that resolved.
    pub async fn assessments(&self) -> Vec<Assessment> {
        let mut assessments = Vec::with_capacity(2);
        for (name, tool) in [("ffmpeg", &self.ffmpeg), ("ffprobe", &self.ffprobe)] {
            if let Some(tool) = tool {
                assessments.push(compat::assess(name, &tool.path).await);
            }
        }
        assessments
    }

    /// The first resolved binary whose version forbids `capability`, if any.
    ///
    /// Returned rather than a boolean so the failure can name the tool and the version that
    /// caused it, which is the difference between a warning somebody can act on and one they
    /// cannot.
    pub async fn blocking(&self, capability: Capability) -> Option<Assessment> {
        self.assessments()
            .await
            .into_iter()
            .find(|assessment| assessment.blocks(capability))
    }

    /// What to pass as `--ffmpeg-location`: the directory holding both binaries, or the
    /// ffmpeg binary itself when ffprobe lives elsewhere.
    ///
    /// yt-dlp then finds ffmpeg, which is all a merge needs; it looks for ffprobe next to it
    /// and does without when there is none.
    #[must_use]
    pub fn location(&self) -> Option<&Path> {
        let ffmpeg = &self.ffmpeg.as_ref()?.path;
        let ffprobe = &self.ffprobe.as_ref()?.path;
        let directory = ffmpeg.parent()?;
        if ffprobe.parent() == Some(directory) {
            Some(directory)
        } else {
            Some(ffmpeg)
        }
    }
}

/// Whether `other` sits in the same directory as `tool`.
fn same_directory(
    tool: &ResolvedTool,
    other: Option<&(ResolvedTool, Option<rd_core::ToolLease>)>,
) -> bool {
    other.is_some_and(|(other, _)| other.path.parent() == tool.path.parent())
}

/// `name` in the directory `path` sits in.
fn sibling(path: &Path, name: &str) -> Option<PathBuf> {
    rd_core::executable_in(path.parent()?, name)
}

/// Reads the tool's version and judges it against the compatibility rules in force.
///
/// The version comes from `rd-tools`, which caches it against the binary's modification time
/// and size. Before that this function spawned a process every time it was called, and the
/// settings page calls it eight times per load.
pub async fn tool_status(name: &str, tool: Option<&ResolvedTool>) -> ToolStatus {
    let Some(tool) = tool else {
        return ToolStatus {
            name: name.to_owned(),
            path: None,
            version: None,
            source: None,
            compatibility: Assessment::unknown(name),
        };
    };
    let detected = rd_tools::version::detect(name, &tool.path).await;
    let compatibility = compat::assess_detected(name, &detected);
    ToolStatus {
        name: name.to_owned(),
        path: Some(tool.path.to_string_lossy().into_owned()),
        version: detected.raw,
        source: Some(tool.source),
        compatibility,
    }
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
