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
pub fn lease_tool(
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
/// post-processing fail with "ffprobe and ffmpeg not found", so the location is only handed
/// over when both live in the same folder.
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
        let mut leases = Vec::new();
        let ffmpeg = rd_core::locate_tool_leased(
            settings.media_ffmpeg_executable.as_deref(),
            vendor,
            "ffmpeg",
        )
        .map(|(tool, lease)| {
            leases.extend(lease);
            tool
        });
        // A configured ffmpeg path implies its sibling ffprobe: users point the setting at
        // one binary, not at both.
        let ffprobe = rd_core::locate_tool_leased(None, vendor, "ffprobe")
            .map(|(tool, lease)| {
                leases.extend(lease);
                tool
            })
            .or_else(|| {
                let directory = ffmpeg.as_ref()?.path.parent()?;
                Some(ResolvedTool {
                    path: rd_core::executable_in(directory, "ffprobe")?,
                    source: ffmpeg.as_ref()?.source,
                })
            });
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
        if !self.is_complete() {
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

    /// The directory to pass as `--ffmpeg-location`, i.e. one holding both binaries.
    #[must_use]
    pub fn location(&self) -> Option<&Path> {
        let ffmpeg = self.ffmpeg.as_ref()?.path.parent()?;
        let ffprobe = self.ffprobe.as_ref()?.path.parent()?;
        (ffmpeg == ffprobe).then_some(ffmpeg)
    }
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
mod tests {
    use rd_core::{ResolvedTool, ToolSource};

    use super::FfmpegTools;

    fn tool(path: &str) -> Option<ResolvedTool> {
        Some(ResolvedTool {
            path: path.into(),
            source: ToolSource::Path,
        })
    }

    #[test]
    fn location_requires_both_binaries_in_one_directory() {
        let split = FfmpegTools {
            ffmpeg: tool("/usr/bin/ffmpeg"),
            ffprobe: tool("/usr/local/bin/ffprobe"),
            leases: Vec::new(),
        };
        assert!(split.is_complete());
        assert!(split.location().is_none());

        let together = FfmpegTools {
            ffmpeg: tool("/opt/vendor/ffmpeg"),
            ffprobe: tool("/opt/vendor/ffprobe"),
            leases: Vec::new(),
        };
        assert_eq!(
            together.location().expect("shared directory").as_os_str(),
            "/opt/vendor"
        );

        let missing = FfmpegTools {
            ffmpeg: tool("/usr/bin/ffmpeg"),
            ffprobe: None,
            leases: Vec::new(),
        };
        assert!(!missing.is_complete());
        assert!(missing.location().is_none());
    }
}
