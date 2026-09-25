//! What each output container can actually hold.
//!
//! One table, consulted from three places: the selector warns before queueing, the runner
//! refuses an impossible remux, and the UI greys out what cannot work. Keeping it a real
//! table rather than a scattering of `matches!` is what makes those three agree — and it is
//! the table multiple audio tracks (RD-080-02) and metadata embedding (RD-080-03) read
//! further columns from.

use super::format::{AudioCodecFamily, VideoCodecFamily};

/// The capabilities of one container.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContainerCapabilities {
    /// Normalised extension (`mp4`, `webm`, `mkv`, `m4a`, `mp3`, `opus`, `flac`).
    pub container: &'static str,
    pub video_codecs: &'static [VideoCodecFamily],
    pub audio_codecs: &'static [AudioCodecFamily],
    /// Whether more than one audio track can be stored (RD-080-02).
    pub multiple_audio: bool,
    /// Whether subtitle tracks can be embedded (RD-080-02).
    pub subtitles: bool,
    /// Whether a cover image can be embedded (RD-080-03).
    pub thumbnail: bool,
    /// Whether chapter markers can be stored (RD-080-03).
    pub chapters: bool,
    /// Whether the container is audio-only.
    pub audio_only: bool,
}

use AudioCodecFamily as A;
use VideoCodecFamily as V;

/// Every container rDownloader will write. Anything not listed is treated as unknown and
/// passes every check, because refusing an unlisted container would break sites that offer
/// something exotic but perfectly downloadable.
pub const CONTAINERS: &[ContainerCapabilities] = &[
    ContainerCapabilities {
        container: "mp4",
        video_codecs: &[V::Avc, V::Hevc, V::Av1],
        audio_codecs: &[A::Aac, A::Ac3, A::Eac3, A::Mp3, A::Flac, A::Opus],
        multiple_audio: true,
        subtitles: true,
        thumbnail: true,
        chapters: true,
        audio_only: false,
    },
    ContainerCapabilities {
        container: "webm",
        video_codecs: &[V::Vp8, V::Vp9, V::Av1],
        audio_codecs: &[A::Opus, A::Vorbis],
        multiple_audio: true,
        subtitles: true,
        thumbnail: false,
        chapters: true,
        audio_only: false,
    },
    ContainerCapabilities {
        container: "mkv",
        video_codecs: &[V::Avc, V::Hevc, V::Av1, V::Vp8, V::Vp9, V::Other],
        audio_codecs: &[
            A::Aac,
            A::Opus,
            A::Vorbis,
            A::Mp3,
            A::Flac,
            A::Ac3,
            A::Eac3,
            A::Other,
        ],
        multiple_audio: true,
        subtitles: true,
        thumbnail: true,
        chapters: true,
        audio_only: false,
    },
    ContainerCapabilities {
        container: "m4a",
        video_codecs: &[],
        audio_codecs: &[A::Aac, A::Flac, A::Mp3],
        multiple_audio: false,
        subtitles: false,
        thumbnail: true,
        chapters: true,
        audio_only: true,
    },
    ContainerCapabilities {
        container: "mp3",
        video_codecs: &[],
        audio_codecs: &[A::Mp3],
        multiple_audio: false,
        subtitles: false,
        thumbnail: true,
        chapters: true,
        audio_only: true,
    },
    ContainerCapabilities {
        container: "opus",
        video_codecs: &[],
        audio_codecs: &[A::Opus],
        multiple_audio: false,
        subtitles: false,
        thumbnail: false,
        chapters: true,
        audio_only: true,
    },
    ContainerCapabilities {
        container: "flac",
        video_codecs: &[],
        audio_codecs: &[A::Flac],
        multiple_audio: false,
        subtitles: false,
        thumbnail: true,
        chapters: false,
        audio_only: true,
    },
    ContainerCapabilities {
        container: "ogg",
        video_codecs: &[],
        audio_codecs: &[A::Vorbis, A::Opus, A::Flac],
        multiple_audio: false,
        subtitles: false,
        thumbnail: false,
        chapters: false,
        audio_only: true,
    },
];

/// The capabilities of `container`, or `None` when it is not one we write ourselves.
#[must_use]
pub fn capabilities(container: &str) -> Option<&'static ContainerCapabilities> {
    let container = container.trim_start_matches('.').to_ascii_lowercase();
    CONTAINERS.iter().find(|entry| entry.container == container)
}

/// Whether `container` can hold `codec`. An unknown container permits everything — see the
/// note on [`CONTAINERS`].
#[must_use]
pub fn supports_video_codec(container: &str, codec: VideoCodecFamily) -> bool {
    capabilities(container).is_none_or(|entry| entry.video_codecs.contains(&codec))
}

/// Whether `container` can hold `codec`.
#[must_use]
pub fn supports_audio_codec(container: &str, codec: AudioCodecFamily) -> bool {
    capabilities(container).is_none_or(|entry| entry.audio_codecs.contains(&codec))
}

/// Whether `container` can hold more than one audio track (RD-080-02).
#[must_use]
pub fn supports_multiple_audio(container: &str) -> bool {
    capabilities(container).is_some_and(|entry| entry.multiple_audio)
}

/// Whether `container` can hold embedded subtitles (RD-080-02).
#[must_use]
pub fn supports_subtitles(container: &str) -> bool {
    capabilities(container).is_some_and(|entry| entry.subtitles)
}

/// Whether `container` can hold an embedded cover image (RD-080-03).
#[must_use]
pub fn supports_thumbnail(container: &str) -> bool {
    capabilities(container).is_some_and(|entry| entry.thumbnail)
}

/// Whether `container` can hold chapter markers (RD-080-03).
#[must_use]
pub fn supports_chapters(container: &str) -> bool {
    capabilities(container).is_some_and(|entry| entry.chapters)
}

/// Whether `container` holds audio only.
#[must_use]
pub fn is_audio_only(container: &str) -> bool {
    capabilities(container).is_some_and(|entry| entry.audio_only)
}

#[cfg(test)]
mod tests {
    use super::{
        AudioCodecFamily, VideoCodecFamily, is_audio_only, supports_audio_codec,
        supports_multiple_audio, supports_subtitles, supports_video_codec,
    };

    #[test]
    fn mp4_and_webm_disagree_about_codecs() {
        assert!(supports_video_codec("mp4", VideoCodecFamily::Avc));
        assert!(!supports_video_codec("mp4", VideoCodecFamily::Vp9));
        assert!(supports_video_codec("webm", VideoCodecFamily::Vp9));
        assert!(!supports_video_codec("webm", VideoCodecFamily::Avc));
        assert!(supports_audio_codec("webm", AudioCodecFamily::Opus));
        assert!(!supports_audio_codec("webm", AudioCodecFamily::Aac));
    }

    #[test]
    fn unknown_containers_permit_everything() {
        assert!(supports_video_codec("avi", VideoCodecFamily::Hevc));
        assert!(supports_audio_codec("avi", AudioCodecFamily::Flac));
        // …but they are not claimed to support features we would have to implement.
        assert!(!supports_multiple_audio("avi"));
        assert!(!supports_subtitles("avi"));
    }

    #[test]
    fn leading_dot_and_case_are_ignored() {
        assert!(supports_video_codec(".MP4", VideoCodecFamily::Avc));
        assert!(is_audio_only("MP3"));
        assert!(!is_audio_only("mp4"));
    }
}
