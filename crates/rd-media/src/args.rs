//! Builds the yt-dlp command line for one download.
//!
//! Every argument the runner passes is constructed here, from a plan rather than from
//! scattered `if`s around the spawn site. Two reasons: the argument list becomes testable
//! without running a process, and there is exactly one place that decides what reaches
//! yt-dlp — which is what lets metadata and SponsorBlock options (RD-080-03) be added under
//! an allowlist later without re-auditing the spawn path.
//!
//! Anything interpolated into a `-f` expression has already passed
//! [`rd_core::MediaFormatCriteria::sanitized`]; the `-o` value is treated as an opaque
//! literal, never as a yt-dlp output template.

use std::{ffi::OsString, path::Path};

use rd_core::{
    MediaEmbedPolicy, MediaFormatCriteria, MediaOutput, MediaPauses, MediaSection, TrackSelection,
};
use rd_files::NoConsoleWindow as _;
use rd_scheduler::ToolNetwork;

use crate::tracks::sub_langs;

/// Prefix yt-dlp is asked to print in front of the finished file's path.
///
/// `--print after_move:filepath` on its own produces a bare path on stdout, indistinguishable
/// from any other unprefixed line an extractor happens to write there. The marker makes the
/// answer identifiable, so one stray line cannot be mistaken for it; `crate::runner` reads it
/// back with `final_path_line`.
pub(crate) const FINAL_PATH_MARKER: &str = "rdownloader-final-path:";

/// Everything one yt-dlp invocation needs.
#[derive(Clone, Debug)]
pub struct DownloadPlan<'a> {
    /// The `-f` expression, already resolved.
    pub format: &'a str,
    /// Absolute output path with a literal stem and yt-dlp's `%(ext)s` placeholder.
    pub output: &'a Path,
    /// Directory holding ffmpeg/ffprobe, or the ffmpeg binary itself when ffprobe lives
    /// elsewhere (see `FfmpegTools::location`); passed to yt-dlp unchanged.
    pub ffmpeg_location: Option<&'a Path>,
    /// Scoped cookie file for a private or age-restricted page (RD-080-04).
    ///
    /// A path, never the cookies themselves: an argument list is readable by every process
    /// on the machine, so the credential must not be in one.
    pub cookies: Option<&'a Path>,
    /// Global speed limit in bytes per second.
    pub limit_rate: Option<u64>,
    /// What to do with the downloaded streams.
    pub output_mode: &'a MediaOutput,
    /// Extra audio tracks and subtitles (RD-080-02).
    pub tracks: &'a TrackSelection,
    /// What to embed and what to cut (RD-080-03). Already reduced to what the container and
    /// the tools actually allow by [`rd_core::effective_policy`], so this builder only
    /// translates it into flags.
    pub embed: &'a MediaEmbedPolicy,
    /// Only this part of the video (RD-1240-15), already sanitised with the criteria.
    pub section: Option<MediaSection>,
    /// Pauses between requests and before the download (RD-1240-15); see `crate::pacing`.
    pub pauses: MediaPauses,
    /// The page to download.
    pub page_url: &'a str,
}

impl DownloadPlan<'_> {
    /// The full argument list, in the order yt-dlp receives it.
    #[must_use]
    pub fn build(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = Vec::new();
        let mut push = |value: &str| args.push(OsString::from(value));
        push("--newline");
        push("--no-playlist");
        push("--no-color");
        // Progress lines and the final path are read as UTF-8. Python writes a pipe in the
        // locale's code page (cp1252 on Windows), and a frozen yt-dlp.exe may ignore
        // `PYTHONIOENCODING`, so the encoding is also yt-dlp's own flag.
        push("--encoding");
        push("utf-8");
        push("--continue");
        push("-f");
        push(self.format);
        args.push(OsString::from("-o"));
        args.push(self.output.as_os_str().to_owned());
        args.push(OsString::from("--print"));
        args.push(OsString::from(format!(
            "after_move:{FINAL_PATH_MARKER}%(filepath)s"
        )));
        // `--print` implies `--quiet`, which also silences the progress lines the runner
        // parses; `--progress` brings them back.
        args.push(OsString::from("--progress"));
        args.push(OsString::from("--no-simulate"));
        if let Some(directory) = self.ffmpeg_location {
            args.push(OsString::from("--ffmpeg-location"));
            args.push(directory.as_os_str().to_owned());
        }
        if let Some(path) = self.cookies {
            args.push(OsString::from("--cookies"));
            args.push(path.as_os_str().to_owned());
        }
        // yt-dlp owns its own sockets, so the limit is handed to the process instead of
        // being enforced in-band. It takes one rate for the whole job — a per-host or
        // per-category limit cannot be expressed, which the capability matrix states.
        if let Some(rate) = self.limit_rate {
            args.push(OsString::from("--limit-rate"));
            args.push(OsString::from(rate.to_string()));
        }
        match self.output_mode {
            MediaOutput::Passthrough => {}
            MediaOutput::Remux { container } => {
                args.push(OsString::from("--merge-output-format"));
                args.push(OsString::from(container));
            }
            MediaOutput::ExtractAudio { codec, quality } => {
                args.push(OsString::from("--extract-audio"));
                args.push(OsString::from("--audio-format"));
                args.push(OsString::from(codec));
                args.push(OsString::from("--audio-quality"));
                args.push(OsString::from(quality.to_string()));
            }
        }
        self.push_track_args(&mut args);
        self.push_embed_args(&mut args);
        self.push_pacing_args(&mut args);
        args.push(OsString::from("--"));
        args.push(OsString::from(self.page_url));
        args
    }

    /// Audio-track and subtitle flags (RD-080-02).
    ///
    /// Only what was actually asked for is emitted. `--write-auto-subs` in particular is
    /// never implied: an automatic caption is a speech-recognition guess, and adding one to
    /// a file nobody asked for it in is not a default anyone can undo afterwards.
    fn push_track_args(&self, args: &mut Vec<OsString>) {
        let mut push = |value: &str| args.push(OsString::from(value));
        if !self.tracks.audio.is_empty() {
            push("--audio-multistreams");
        }
        let subtitles = &self.tracks.subtitles;
        let Some(languages) = sub_langs(subtitles) else {
            return;
        };
        if subtitles.include_automatic {
            push("--write-auto-subs");
        }
        push("--write-subs");
        push("--sub-langs");
        push(&languages);
        if let Some(format) = subtitles.convert_to.as_deref() {
            push("--convert-subs");
            push(format);
        }
        if subtitles.mode.embeds() {
            push("--embed-subs");
        }
        // yt-dlp deletes the sidecar files it embedded unless told otherwise, so asking for
        // both is expressed explicitly rather than assumed.
        if subtitles.mode.embeds() && subtitles.mode.writes_sidecar() {
            push("--keep-subs");
        }
    }

    /// Metadata embedding and SponsorBlock flags (RD-080-03).
    ///
    /// Every flag is emitted from an allowlisted policy field; nothing here is built from a
    /// free-form string, and the negative forms are emitted explicitly because
    /// `--embed-metadata` otherwise pulls chapters and the info JSON in with it.
    fn push_embed_args(&self, args: &mut Vec<OsString>) {
        let mut push = |value: &str| args.push(OsString::from(value));
        let policy = self.embed;
        if policy.thumbnail {
            push("--embed-thumbnail");
        }
        if policy.metadata {
            push("--embed-metadata");
            // `--embed-metadata` implies both of these, so refusing them has to be said.
            if !policy.chapters {
                push("--no-embed-chapters");
            }
            if !policy.info_json {
                push("--no-embed-info-json");
            }
        }
        if policy.chapters {
            push("--embed-chapters");
        }
        if policy.info_json {
            push("--embed-info-json");
        }
        let categories = policy.sponsorblock.effective_categories();
        if categories.is_empty() {
            return;
        }
        let list = categories
            .iter()
            .map(|category| category.as_str())
            .collect::<Vec<_>>()
            .join(",");
        push(match policy.sponsorblock.mode {
            rd_core::SponsorMode::Remove => "--sponsorblock-remove",
            rd_core::SponsorMode::Mark | rd_core::SponsorMode::Off => "--sponsorblock-mark",
        });
        push(&list);
    }
}

/// The output mode a selection asks for, falling back to what the legacy presets did.
#[must_use]
pub fn output_mode(criteria: Option<&MediaFormatCriteria>, audio: bool) -> MediaOutput {
    match criteria.map(|criteria| criteria.output.clone()) {
        Some(output) => output,
        None if audio => MediaOutput::ExtractAudio {
            codec: "mp3".to_owned(),
            quality: 0,
        },
        None => MediaOutput::Remux {
            container: "mp4".to_owned(),
        },
    }
}

/// The network options of one yt-dlp invocation (RD-1240-08).
///
/// The proxy as yt-dlp's own `--proxy` when it carries no credentials; one with credentials
/// reaches yt-dlp through its environment only ([`ToolNetwork::apply`]), because an argument
/// list is readable by every process on the machine. With a custom CA, yt-dlp is told to use
/// the system trust, which reads the bundle in `SSL_CERT_FILE`, instead of its own certifi.
#[must_use]
pub(crate) fn network_args(network: &ToolNetwork) -> Vec<OsString> {
    let mut args = Vec::new();
    if let Some(proxy) = network.proxy_argument() {
        args.push(OsString::from("--proxy"));
        args.push(OsString::from(proxy));
    }
    if network.trust_bundle().is_some() {
        args.push(OsString::from("--compat-options"));
        args.push(OsString::from("no-certifi"));
    }
    args
}

/// A yt-dlp command with `network`'s environment and options set; the caller adds the rest.
pub(crate) fn ytdlp_command(ytdlp: &Path, network: &ToolNetwork) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(ytdlp);
    command.no_console_window();
    network.apply(&mut command);
    command.args(network_args(network));
    command
}

#[cfg(test)]
#[path = "args_tests.rs"]
mod tests;
