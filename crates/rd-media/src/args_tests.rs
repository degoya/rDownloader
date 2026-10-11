use std::path::Path;

use rd_core::{
    AudioTrackPolicy, MediaEmbedPolicy, MediaOutput, MediaPauses, SponsorBlockPolicy,
    SponsorCategory, SponsorMode, SubtitleMode, SubtitlePolicy, TrackSelection,
};

use super::{DownloadPlan, output_mode, ytdlp_command};

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
        section: None,
        pauses: MediaPauses::NONE,
        page_url: "https://example.test/watch?v=1",
    };
    assert_eq!(
        strings(&plan),
        vec![
            "--newline",
            "--no-playlist",
            "--no-color",
            "--encoding",
            "utf-8",
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
        section: None,
        pauses: MediaPauses::NONE,
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
        section: None,
        pauses: MediaPauses::NONE,
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
        section: None,
        pauses: MediaPauses::NONE,
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
        section: None,
        pauses: MediaPauses::NONE,
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
        section: None,
        pauses: MediaPauses::NONE,
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
        section: None,
        pauses: MediaPauses::NONE,
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
        section: None,
        pauses: MediaPauses::NONE,
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

/// RD-1240-08: the download's proxy is yt-dlp's own `--proxy`; its credentials reach yt-dlp
/// through the environment and never the command line; without a proxy there is none.
#[test]
fn the_proxy_is_on_the_command_line_and_its_credentials_are_not() {
    use rd_scheduler::{ToolNetwork, ToolProxy};
    use secrecy::SecretString;

    let command_line = |network: &ToolNetwork| -> Vec<String> {
        ytdlp_command(Path::new("yt-dlp"), network)
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    };
    let proxy = |password: Option<&str>| {
        let password = password.map(|password| SecretString::from(password.to_owned()));
        ToolNetwork::with_proxy(
            ToolProxy::new(
                rd_core::ProxyKind::Socks5,
                "socks5h://proxy.example:1080".parse().expect("endpoint"),
                password.as_ref().map(|_| "alice"),
                password.as_ref(),
            )
            .expect("proxy"),
        )
    };
    assert_eq!(
        command_line(&proxy(None)),
        ["--proxy", "socks5h://proxy.example:1080"]
    );
    let network = proxy(Some("pr0xy-secret"));
    assert!(command_line(&network).is_empty());
    let command = ytdlp_command(Path::new("yt-dlp"), &network);
    let environment: Vec<_> = command.as_std().get_envs().collect();
    assert!(
        environment.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("HTTPS_PROXY")
                && value.is_some_and(|value| value.to_string_lossy().contains("pr0xy-secret"))
        }),
        "{environment:?}"
    );
    assert!(command_line(&ToolNetwork::direct()).is_empty());
}
