//! What a release name is read as, and what it is deliberately not read as (RD-110-21).

use super::parse;

#[test]
fn a_scene_name_gives_up_its_series_season_and_episode() {
    let release = parse("The.Expanse.S05E03.German.DL.1080p.WEB.x264-GROUP");
    assert_eq!(release.series, "the expanse");
    assert_eq!(release.season, Some(5));
    assert_eq!(release.episode, Some(3));
    assert_eq!(release.quality.as_deref(), Some("1080p"));
    // `german` comes before `dl` in the closed list, and the first match wins.
    assert_eq!(release.language.as_deref(), Some("German"));
    assert_eq!(release.height(), Some(1080));
    assert_eq!(release.key().as_deref(), Some("the expanse|s05e03"));
}

#[test]
fn the_same_episode_in_another_release_has_the_same_key() {
    // The case the whole job exists for: another group, another quality, another week.
    let first = parse("The.Expanse.S05E03.German.DL.1080p.WEB.x264-GROUP");
    let second = parse("The Expanse - S05E03 - 720p HDTV x264 [OTHER]");
    let third = parse("the.expanse.s05e03.2160p.uhd.bluray.remux-THIRD");
    assert_eq!(first.key(), second.key());
    assert_eq!(first.key(), third.key());
    assert_eq!(second.quality.as_deref(), Some("720p"));
    assert_eq!(third.quality.as_deref(), Some("2160p"));
}

#[test]
fn a_different_episode_is_a_different_key() {
    assert_ne!(
        parse("Show.S01E01.1080p").key(),
        parse("Show.S01E02.1080p").key()
    );
    assert_ne!(
        parse("Show.S01E01.1080p").key(),
        parse("Show.S02E01.1080p").key()
    );
    assert_ne!(
        parse("Show.S01E01.1080p").key(),
        parse("Other.Show.S01E01.1080p").key()
    );
}

#[test]
fn the_three_spellings_that_occur_all_read_as_one() {
    assert_eq!(parse("Show.S01E02.1080p").key(), parse("Show 1x02").key());
    let pack = parse("Show.S02.COMPLETE.German.1080p.WEB-GROUP");
    assert_eq!(pack.season, Some(2));
    assert_eq!(pack.episode, None);
    assert_eq!(pack.key().as_deref(), Some("show|s02"));
    // A pack is not episode one of its season, and the two keys must not collide.
    assert_ne!(pack.key(), parse("Show.S02E01.1080p").key());
}

#[test]
fn a_double_episode_is_its_own_release() {
    let double = parse("Show.S01E02E03.1080p.WEB-GROUP");
    assert_eq!(double.key().as_deref(), Some("show|s01e02e03"));
    assert_eq!(double.episode, Some(2));
    assert_ne!(double.key(), parse("Show.S01E02.1080p").key());
}

#[test]
fn a_name_that_names_no_episode_yields_no_key() {
    // Recognition then falls back to the address, which is what every other kind does. A
    // wrong key would silence a genuinely new release forever.
    for name in [
        "Some.Movie.2026.1080p.BluRay.x264-GROUP",
        "Various Artists - Discography FLAC",
        "",
    ] {
        assert!(parse(name).key().is_none(), "{name}");
    }
}

#[test]
fn a_year_a_resolution_and_a_bare_number_are_not_a_season() {
    // `s2026` is a year, `s1080` never a season, and a lone `02` is nothing at all. Each
    // would otherwise invent an episode and hide every later release under its key.
    for name in ["Show.S2026.1080p", "Show.S1080.WEB", "Show.02.1080p"] {
        assert!(parse(name).key().is_none(), "{name}");
    }
    // The tokens after a real marker never become the series either.
    assert_eq!(parse("S01E02.Show.1080p").key(), None);
}

#[test]
fn quality_and_language_come_from_the_shared_token_lists() {
    // The closed lists of RD-110-18, reused rather than copied: a number that is not a
    // quality token stays no quality.
    let name = parse("Show.S01E01.2160.mkv");
    assert_eq!(name.quality, None);
    assert_eq!(name.height(), None);
    assert_eq!(
        parse("Show.S01E01.4K.HDR").quality.as_deref(),
        Some("2160p")
    );
    assert_eq!(
        parse("Show.S01E01.MULTi.1080p").language.as_deref(),
        Some("Multi")
    );
    assert_eq!(parse("Show.S01E01.1080p").language, None);
}
