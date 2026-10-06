//! Reading a title, a year and a season or episode out of a release name, or finding none.

use super::{ReleaseName, parse};

#[test]
fn the_release_naming_out_of_usenet_and_torrents_yields_title_and_year() {
    let parsed = parse("Some.Film.2019.1080p.BluRay.x264-GRUPPE").expect("a release name");
    assert_eq!(parsed.title, "Some Film");
    assert_eq!(parsed.year, Some(2019));
    assert!(!parsed.is_series());
}

#[test]
fn the_shapes_a_film_release_actually_arrives_in_are_all_read() {
    for (name, title, year) in [
        (
            "Some.Film.2019.1080p.BluRay.x264-GRUPPE.mkv",
            "Some Film",
            2019,
        ),
        ("Some_Film_2019_720p_WEB-DL_x265.mkv", "Some Film", 2019),
        ("Some Film (2019) [1080p] [BluRay]", "Some Film", 2019),
        (
            "The.Longer.Title.Here.2004.German.DL.1080p.BluRay.x264-GRP",
            "The Longer Title Here",
            2004,
        ),
        // The year is behind a marker here, which is why the scan does not stop at the
        // first one it sees.
        (
            "Another.Film.German.2011.COMPLETE.UHD.BLURAY-GROUP",
            "Another Film",
            2011,
        ),
        (
            "Some.Film.2019.1080p.BluRay.x264-GRP.part03.rar",
            "Some Film",
            2019,
        ),
        (
            "/downloads/incoming/Some.Film.2019.1080p.WEB.h264-GRP/",
            "Some Film",
            2019,
        ),
    ] {
        let parsed = parse(name).unwrap_or_else(|| panic!("{name} should parse"));
        assert_eq!(parsed.title, title, "{name}");
        assert_eq!(parsed.year, Some(year), "{name}");
        assert!(!parsed.is_series(), "{name}");
    }
}

#[test]
fn a_number_in_the_title_is_not_mistaken_for_the_year() {
    let parsed = parse("Blade.Runner.2049.2017.2160p.UHD.BluRay.x265-GRP").expect("parses");
    assert_eq!(parsed.title, "Blade Runner 2049");
    assert_eq!(parsed.year, Some(2017));
}

#[test]
fn an_episode_is_recognised_as_one_with_its_season_and_number() {
    for (name, title, season, episode) in [
        (
            "Some.Show.S02E05.1080p.WEB-DL.x264-GRP",
            "Some Show",
            2,
            Some(5),
        ),
        ("Some.Show.s2e5.HDTV.x264", "Some Show", 2, Some(5)),
        ("Some.Show.2x05.720p.HDTV", "Some Show", 2, Some(5)),
        ("Some.Show.S02.E05.1080p", "Some Show", 2, Some(5)),
        // A season pack: the season is known, the episode is not, and that is a truthful
        // answer rather than a guess at episode one.
        ("Some.Show.S02.COMPLETE.1080p.WEB-DL", "Some Show", 2, None),
    ] {
        let parsed = parse(name).unwrap_or_else(|| panic!("{name} should parse"));
        assert!(parsed.is_series(), "{name}");
        assert_eq!(parsed.title, title, "{name}");
        assert_eq!(parsed.season, Some(season), "{name}");
        assert_eq!(parsed.episode, episode, "{name}");
    }
}

#[test]
fn an_episode_number_without_a_season_is_an_episode_with_an_unknown_season() {
    // The two names RD-108-14 was reported with. Both were read as films, so both were
    // looked up in a film catalogue, and both came back wearing a 1994 cinema release.
    for (name, episode) in [
        (
            "Dragon.Ball.DAIMA.E15.Das.dritte.Auge.German.DL.1080p.WEB.h264-GRP",
            15,
        ),
        (
            "Dragon.Ball.DAIMA.E14.Tabu.German.DL.1080p.WEB.h264-GRP",
            14,
        ),
    ] {
        let parsed = parse(name).unwrap_or_else(|| panic!("{name} should parse"));
        assert!(parsed.is_series(), "{name} is an episode, not a film");
        assert_eq!(parsed.title, "Dragon Ball DAIMA", "{name}");
        assert_eq!(parsed.episode, Some(episode), "{name}");
        // Nothing in the name says which season, so nothing is said about it.
        assert_eq!(parsed.season, None, "{name}");
        assert_eq!(parsed.year, None, "{name}");
    }
}

#[test]
fn a_season_and_episode_marker_still_carries_both_halves() {
    // The third reported name. Its season was never in doubt; what was wrong sat one layer
    // up, in which catalogue entry the title was matched against.
    let parsed = parse("Apollo.Has.Fallen.S02E02.DL.GERMAN.1080p.WEB.h264-GRP").expect("parses");
    assert!(parsed.is_series());
    assert_eq!(parsed.title, "Apollo Has Fallen");
    assert_eq!(parsed.season, Some(2));
    assert_eq!(parsed.episode, Some(2));
}

#[test]
fn an_e_token_behind_the_markers_is_a_group_and_leaves_the_film_a_film() {
    // The cost of believing a lone episode number: a group or an edition written the same
    // way would make a series out of every film carrying one. Only an episode number that
    // still stands inside the title counts.
    for name in [
        "Some.Film.2019.1080p.BluRay.x264-E11",
        "Some.Film.2019.1080p.BluRay.x264.E5.mkv",
    ] {
        let parsed = parse(name).unwrap_or_else(|| panic!("{name} should parse"));
        assert!(!parsed.is_series(), "{name}");
        assert_eq!(parsed.episode, None, "{name}");
        assert_eq!(parsed.title, "Some Film", "{name}");
    }
}

#[test]
fn a_name_that_gives_no_title_is_answered_without_asking_anybody() {
    // The decision this plugin makes most often, and the one that has to be free: every
    // one of these returns `None`, which is the guest's instruction to make no request.
    for name in [
        "setup.exe",
        "holiday-photos.zip",
        "invoice.pdf",
        "Some.Album.2019.FLAC.mp3",
        "readme.nfo",
        "Some.Film.2019.1080p.BluRay.x264-GRP.srt",
        "archive.tar.gz",
        "document",
        "IMG_20190812_144501.jpg",
        "backup-2019-08-12.sql",
        "",
        "   ",
        "https://example.com/download?id=4711",
    ] {
        assert_eq!(parse(name), None, "{name} must not reach a service");
    }
}

#[test]
fn a_marker_without_a_title_in_front_of_it_is_still_nothing() {
    // `1080p.mkv` carries a marker but nothing to search for, and a request built from an
    // empty title is a request that tells a service something for no possible answer.
    assert_eq!(parse("1080p.mkv"), None);
    assert_eq!(parse("sample.1080p.mkv"), None);
    assert_eq!(parse("2019.mkv"), None);
}

#[test]
fn an_ordinary_video_file_without_any_marker_asks_nobody() {
    // A file that is plainly a video but carries no release marker: there is no title to
    // look up with any confidence, and guessing would mean sending file names of a
    // person's own recordings to a service.
    assert_eq!(parse("holiday.mkv"), None);
    assert_eq!(parse("clip.mp4"), None);
    assert_eq!(
        parse("Some.Film.mkv"),
        None,
        "no year, no resolution, no source: not a release name"
    );
}

#[test]
fn the_parsed_name_is_the_whole_decision() {
    // Stated as a test because it is the contract the guest relies on: a `Some` carries a
    // title that is worth a query, and a `None` means no query at all.
    let parsed: ReleaseName = parse("Some.Film.2019.1080p.BluRay.x264-GRP").expect("parses");
    assert!(!parsed.title.is_empty());
    assert!(parsed.title.chars().any(char::is_alphabetic));
}
