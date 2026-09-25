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

use rd_core::{MediaEmbedPolicy, MediaFormatCriteria, MediaOutput, TrackSelection};

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
    /// Directory holding ffmpeg/ffprobe, when they were found outside `PATH`.
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rd_core::{
        AudioTrackPolicy, MediaEmbedPolicy, MediaOutput, SponsorBlockPolicy, SponsorCategory,
        SponsorMode, SubtitleMode, SubtitlePolicy, TrackSelection,
    };

    use super::{DownloadPlan, output_mode};

    fn strings(plan: &DownloadPlan) -> Vec<String> {
        plan.build()
            .into_iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_video_download_remuxes_and_ends_with_the_url() {
        let output = Path::new("/downloads/pkg/clip.%(ext)s");
        let plan = DownloadPlan {
            format: "137+251/bv*[height<=1080]+ba/b[height<=1080]/b",
            output,
            ffmpeg_location: None,
            cookies: None,
            limit_rate: None,
            output_mode: &MediaOutput::Remux {
                container: "mp4".to_owned(),
            },
            tracks: &TrackSelection::default(),
            embed: &MediaEmbedPolicy::default(),
            page_url: "https://example.test/watch?v=1",
        };
        assert_eq!(
            strings(&plan),
            vec![
                "--newline",
                "--no-playlist",
                "--no-color",
                "--continue",
                "-f",
                "137+251/bv*[height<=1080]+ba/b[height<=1080]/b",
                "-o",
                "/downloads/pkg/clip.%(ext)s",
                "--print",
                "after_move:rdownloader-final-path:%(filepath)s",
                "--progress",
                "--no-simulate",
                "--merge-output-format",
                "mp4",
                "--",
                "https://example.test/watch?v=1",
            ]
        );
    }

    #[test]
    fn audio_extraction_and_a_rate_limit_are_expressed_as_flags() {
        let output = Path::new("/downloads/pkg/song.%(ext)s");
        let plan = DownloadPlan {
            format: "ba/b",
            output,
            ffmpeg_location: Some(Path::new("/opt/vendor")),
            cookies: None,
            limit_rate: Some(1_048_576),
            output_mode: &MediaOutput::ExtractAudio {
                codec: "mp3".to_owned(),
                quality: 0,
            },
            tracks: &TrackSelection::default(),
            embed: &MediaEmbedPolicy::default(),
            page_url: "https://example.test/song",
        };
        let args = strings(&plan);
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--ffmpeg-location".to_owned(), "/opt/vendor".to_owned()])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--limit-rate".to_owned(), "1048576".to_owned()])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--audio-format".to_owned(), "mp3".to_owned()])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--audio-quality".to_owned(), "0".to_owned()])
        );
        assert_eq!(
            args.last().map(String::as_str),
            Some("https://example.test/song")
        );
    }

    #[test]
    fn passthrough_adds_no_conversion_flags() {
        let output = Path::new("/downloads/pkg/raw.%(ext)s");
        let plan = DownloadPlan {
            format: "b",
            output,
            ffmpeg_location: None,
            cookies: None,
            limit_rate: None,
            output_mode: &MediaOutput::Passthrough,
            tracks: &TrackSelection::default(),
            embed: &MediaEmbedPolicy::default(),
            page_url: "https://example.test/raw",
        };
        let args = strings(&plan);
        assert!(!args.iter().any(|arg| arg == "--merge-output-format"));
        assert!(!args.iter().any(|arg| arg == "--extract-audio"));
    }

    #[test]
    fn subtitles_are_only_requested_when_asked_for() {
        let output = Path::new("/downloads/pkg/clip.%(ext)s");
        let plain = DownloadPlan {
            format: "b",
            output,
            ffmpeg_location: None,
            cookies: None,
            limit_rate: None,
            output_mode: &MediaOutput::Passthrough,
            tracks: &TrackSelection::default(),
            embed: &MediaEmbedPolicy::default(),
            page_url: "https://example.test/clip",
        };
        let args = strings(&plain);
        assert!(!args.iter().any(|arg| arg.starts_with("--write-sub")));
        assert!(!args.iter().any(|arg| arg == "--audio-multistreams"));

        let tracks = TrackSelection {
            audio: AudioTrackPolicy {
                extra_languages: vec!["de".to_owned()],
            },
            subtitles: SubtitlePolicy {
                mode: SubtitleMode::SidecarAndEmbed,
                languages: vec!["de".to_owned(), "en".to_owned()],
                include_automatic: false,
                convert_to: Some("srt".to_owned()),
            },
        };
        let args = strings(&DownloadPlan {
            tracks: &tracks,
            ..plain.clone()
        });
        assert!(args.iter().any(|arg| arg == "--audio-multistreams"));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--sub-langs".to_owned(), "de,en".to_owned()])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--convert-subs".to_owned(), "srt".to_owned()])
        );
        assert!(args.iter().any(|arg| arg == "--embed-subs"));
        assert!(
            args.iter().any(|arg| arg == "--keep-subs"),
            "asking for both a sidecar and an embed must keep the sidecar"
        );
        assert!(
            !args.iter().any(|arg| arg == "--write-auto-subs"),
            "automatic captions are never implied"
        );
    }

    #[test]
    fn automatic_captions_need_the_explicit_opt_in() {
        let output = Path::new("/downloads/pkg/clip.%(ext)s");
        let tracks = TrackSelection {
            subtitles: SubtitlePolicy {
                mode: SubtitleMode::Sidecar,
                include_automatic: true,
                ..SubtitlePolicy::default()
            },
            ..TrackSelection::default()
        };
        let args = strings(&DownloadPlan {
            format: "b",
            output,
            ffmpeg_location: None,
            cookies: None,
            limit_rate: None,
            output_mode: &MediaOutput::Passthrough,
            tracks: &tracks,
            embed: &MediaEmbedPolicy::default(),
            page_url: "https://example.test/clip",
        });
        assert!(args.iter().any(|arg| arg == "--write-auto-subs"));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--sub-langs".to_owned(), "all".to_owned()])
        );
        assert!(!args.iter().any(|arg| arg == "--embed-subs"));
    }

    fn plan_with<'a>(
        output: &'a Path,
        tracks: &'a TrackSelection,
        embed: &'a MediaEmbedPolicy,
    ) -> DownloadPlan<'a> {
        DownloadPlan {
            format: "b",
            output,
            ffmpeg_location: None,
            cookies: None,
            limit_rate: None,
            output_mode: &MediaOutput::Passthrough,
            tracks,
            embed,
            page_url: "https://example.test/clip",
        }
    }

    #[test]
    fn nothing_is_embedded_unless_it_was_asked_for() {
        let output = Path::new("/downloads/pkg/clip.%(ext)s");
        let args = strings(&plan_with(
            output,
            &TrackSelection::default(),
            &MediaEmbedPolicy::default(),
        ));
        assert!(!args.iter().any(|arg| arg.starts_with("--embed-")));
        assert!(!args.iter().any(|arg| arg.starts_with("--sponsorblock-")));
    }

    #[test]
    fn embed_metadata_says_explicitly_what_it_does_not_want() {
        // `--embed-metadata` pulls chapters and the info JSON in with it, so refusing them
        // has to be stated or the file quietly gains what was switched off.
        let output = Path::new("/downloads/pkg/clip.%(ext)s");
        let embed = MediaEmbedPolicy {
            thumbnail: true,
            chapters: false,
            metadata: true,
            info_json: false,
            sponsorblock: SponsorBlockPolicy::default(),
        };
        let args = strings(&plan_with(output, &TrackSelection::default(), &embed));
        assert!(args.iter().any(|arg| arg == "--embed-thumbnail"));
        assert!(args.iter().any(|arg| arg == "--embed-metadata"));
        assert!(args.iter().any(|arg| arg == "--no-embed-chapters"));
        assert!(args.iter().any(|arg| arg == "--no-embed-info-json"));
        assert!(!args.iter().any(|arg| arg == "--embed-chapters"));
    }

    #[test]
    fn sponsorblock_marks_by_default_and_only_removes_when_told_to() {
        let output = Path::new("/downloads/pkg/clip.%(ext)s");
        let marking = MediaEmbedPolicy {
            sponsorblock: SponsorBlockPolicy {
                mode: SponsorMode::Mark,
                categories: Vec::new(),
            },
            ..MediaEmbedPolicy::default()
        };
        let args = strings(&plan_with(output, &TrackSelection::default(), &marking));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--sponsorblock-mark".to_owned(), "sponsor".to_owned()]),
            "an empty category list means sponsor segments only, never all of them: {args:?}"
        );

        let removing = MediaEmbedPolicy {
            sponsorblock: SponsorBlockPolicy {
                mode: SponsorMode::Remove,
                categories: vec![SponsorCategory::Sponsor, SponsorCategory::SelfPromo],
            },
            ..MediaEmbedPolicy::default()
        };
        let args = strings(&plan_with(output, &TrackSelection::default(), &removing));
        assert!(args.windows(2).any(|pair| pair
            == [
                "--sponsorblock-remove".to_owned(),
                "sponsor,selfpromo".to_owned()
            ]));
        assert!(!args.iter().any(|arg| arg == "--sponsorblock-mark"));
    }

    #[test]
    fn a_cookie_file_is_passed_by_path_and_never_by_value() {
        let cookies = Path::new("/tmp/rd-cookies-abc.txt");
        let plan = DownloadPlan {
            format: "b",
            output: Path::new("/downloads/pkg/clip.%(ext)s"),
            ffmpeg_location: None,
            cookies: Some(cookies),
            limit_rate: None,
            output_mode: &MediaOutput::Passthrough,
            tracks: &TrackSelection::default(),
            embed: &MediaEmbedPolicy::default(),
            page_url: "https://example.test/watch?v=1",
        };
        let args = strings(&plan);
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--cookies" && pair[1] == "/tmp/rd-cookies-abc.txt"),
            "{args:?}"
        );
        // The flag must stay ahead of the `--` separator, or yt-dlp reads it as a URL.
        let separator = args.iter().position(|arg| arg == "--").expect("separator");
        let flag = args
            .iter()
            .position(|arg| arg == "--cookies")
            .expect("flag");
        assert!(flag < separator);
    }

    #[test]
    fn no_cookie_selection_emits_no_cookie_flag() {
        let plan = DownloadPlan {
            format: "b",
            output: Path::new("/downloads/pkg/clip.%(ext)s"),
            ffmpeg_location: None,
            cookies: None,
            limit_rate: None,
            output_mode: &MediaOutput::Passthrough,
            tracks: &TrackSelection::default(),
            embed: &MediaEmbedPolicy::default(),
            page_url: "https://example.test/watch?v=1",
        };
        assert!(!strings(&plan).iter().any(|arg| arg == "--cookies"));
    }

    #[test]
    fn a_legacy_row_without_criteria_keeps_doing_what_it_always_did() {
        assert_eq!(
            output_mode(None, false),
            MediaOutput::Remux {
                container: "mp4".to_owned()
            }
        );
        assert_eq!(
            output_mode(None, true),
            MediaOutput::ExtractAudio {
                codec: "mp3".to_owned(),
                quality: 0
            }
        );
    }
}
