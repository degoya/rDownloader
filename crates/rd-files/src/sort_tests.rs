//! Sort templates (RD-1100-08): recognition against real scene names, the template language,
//! hostile values, and the plan for a whole package.

use std::path::{Component, Path, PathBuf};

/// A scene name, what it is taken for, and the fields read from it.
type Case<'a> = (&'a str, SortKind, &'a [(&'a str, &'a str)]);

use rd_core::{SortKind, SortTemplates};

use crate::{
    SortMatch, SortTemplateError, expand_sort_template, plan_sort, recognize_release, sort_values,
    validate_sort_template,
};

const SERIES: &str = "{show}/Season {season:00}/{show} - S{season:00}E{episode:00} - {title}";
const DATED: &str = "{show}/{year}/{show} - {date} - {title}";
const MOVIE: &str = "{movie} ({year})/{movie} ({year})";

fn root() -> PathBuf {
    Path::new("library").join("media")
}

fn templates() -> SortTemplates {
    SortTemplates {
        series: Some(SERIES.to_owned()),
        dated: Some(DATED.to_owned()),
        movie: Some(MOVIE.to_owned()),
    }
}

fn found(name: &str) -> SortMatch {
    recognize_release(name).unwrap_or_else(|| panic!("{name} is recognised"))
}

/// One row: the name, its kind, and the fields a template gets, as the preview lists them.
#[test]
fn real_scene_names_are_recognised() {
    let table: &[Case] = &[
        (
            "Breaking.Bad.S05E14.Ozymandias.720p.HDTV.x264-IMMERSE.mkv",
            SortKind::Series,
            &[
                ("show", "Breaking Bad"),
                ("season", "5"),
                ("episode", "14"),
                ("title", "Ozymandias"),
                ("resolution", "720p"),
                ("source", "HDTV"),
            ],
        ),
        (
            "Game.of.Thrones.S08E03.The.Long.Night.1080p.AMZN.WEB-DL.DDP5.1.H.264-GoT.mkv",
            SortKind::Series,
            &[
                ("show", "Game of Thrones"),
                ("season", "8"),
                ("episode", "3"),
                ("title", "The Long Night"),
                ("resolution", "1080p"),
                ("source", "WEB-DL"),
            ],
        ),
        (
            "Friends.S01E16E17.DVDRip.XviD-SAiNTS.avi",
            SortKind::Series,
            &[
                ("show", "Friends"),
                ("season", "1"),
                ("episode", "16-17"),
                ("source", "DVDRip"),
            ],
        ),
        (
            "Lost.S01E01-E02.Pilot.720p.BluRay.x264-SiNNERS.mkv",
            SortKind::Series,
            &[
                ("show", "Lost"),
                ("season", "1"),
                ("episode", "1-2"),
                ("title", "Pilot"),
                ("resolution", "720p"),
                ("source", "BluRay"),
            ],
        ),
        (
            "Doctor.Who.2005.S13E01.720p.HDTV.x264-GRP.mkv",
            SortKind::Series,
            &[
                ("show", "Doctor Who"),
                ("season", "13"),
                ("episode", "1"),
                ("year", "2005"),
                ("resolution", "720p"),
                ("source", "HDTV"),
            ],
        ),
        (
            "Futurama.4x12.Where.No.Fan.Has.Gone.Before.DVDRip.XviD.avi",
            SortKind::Series,
            &[
                ("show", "Futurama"),
                ("season", "4"),
                ("episode", "12"),
                ("title", "Where No Fan Has Gone Before"),
                ("source", "DVDRip"),
            ],
        ),
        (
            "The.Daily.Show.2024.03.15.Guest.Name.720p.WEB.h264-EDITH.mkv",
            SortKind::Dated,
            &[
                ("show", "The Daily Show"),
                ("date", "2024-03-15"),
                ("year", "2024"),
                ("title", "Guest Name"),
                ("resolution", "720p"),
                ("source", "WEB"),
            ],
        ),
        (
            "Inception.2010.1080p.BluRay.x264-SPARKS.mkv",
            SortKind::Movie,
            &[
                ("movie", "Inception"),
                ("year", "2010"),
                ("resolution", "1080p"),
                ("source", "BluRay"),
            ],
        ),
        (
            "2001.A.Space.Odyssey.1968.REMASTERED.1080p.BluRay.x264-AMIABLE.mkv",
            SortKind::Movie,
            &[
                ("movie", "2001 A Space Odyssey"),
                ("year", "1968"),
                ("resolution", "1080p"),
                ("source", "BluRay"),
            ],
        ),
        (
            "Blade.Runner.2049.2017.2160p.UHD.BluRay.x265-TERMiNAL.mkv",
            SortKind::Movie,
            &[
                ("movie", "Blade Runner 2049"),
                ("year", "2017"),
                ("resolution", "2160p"),
                ("source", "BluRay"),
            ],
        ),
        (
            "Am\u{e9}lie (2001).mkv",
            SortKind::Movie,
            &[("movie", "Am\u{e9}lie"), ("year", "2001")],
        ),
    ];
    for (name, kind, fields) in table {
        let found = found(name);
        assert_eq!(found.kind, *kind, "{name}");
        let values = sort_values(&found);
        let expected: Vec<(&str, String)> = fields
            .iter()
            .map(|(field, value)| (*field, (*value).to_owned()))
            .collect();
        let mut sorted_values = values.clone();
        sorted_values.sort();
        let mut sorted_expected = expected.clone();
        sorted_expected.sort();
        assert_eq!(sorted_values, sorted_expected, "{name}");
    }
}

#[test]
fn names_outside_the_rules_are_not_recognised() {
    for name in [
        "holiday_video.mp4",
        // A season pack names no episode.
        "Show.S01.1080p.BluRay.x264-GRP.mkv",
        // Absolute numbering is a documented limit.
        "[HorribleSubs] Show - 01 [720p].mkv",
        // A film needs its year.
        "Some.Film.1080p.BluRay.mkv",
        // An episode needs its show.
        "S01E02.mkv",
        "",
    ] {
        assert_eq!(recognize_release(name), None, "{name}");
    }
}

#[test]
fn an_episode_and_a_film_land_by_their_templates() {
    let episode = expand_sort_template(
        &root(),
        SERIES,
        &found("Breaking.Bad.S05E14.Ozymandias.720p.HDTV.x264-IMMERSE.mkv"),
    )
    .expect("expands");
    assert_eq!(
        episode.directory,
        root().join("Breaking Bad").join("Season 05")
    );
    assert_eq!(episode.stem, "Breaking Bad - S05E14 - Ozymandias");

    let film = expand_sort_template(
        &root(),
        MOVIE,
        &found("Inception.2010.1080p.BluRay.x264-SPARKS.mkv"),
    )
    .expect("expands");
    assert_eq!(film.directory, root().join("Inception (2010)"));
    assert_eq!(film.stem, "Inception (2010)");

    let dated = expand_sort_template(
        &root(),
        DATED,
        &found("The.Daily.Show.2024.03.15.Guest.Name.720p.WEB.h264-EDITH.mkv"),
    )
    .expect("expands");
    assert_eq!(dated.directory, root().join("The Daily Show").join("2024"));
    assert_eq!(dated.stem, "The Daily Show - 2024-03-15 - Guest Name");
}

#[test]
fn multi_episode_files_name_their_first_and_last_episode() {
    let target = expand_sort_template(
        &root(),
        SERIES,
        &found("Friends.S01E16E17.DVDRip.XviD-SAiNTS.avi"),
    )
    .expect("expands");
    // No title: the separator it would have needed is tidied away.
    assert_eq!(target.stem, "Friends - S01E16-E17");
    let range = expand_sort_template(&root(), SERIES, &found("Show.Name.S02E05-07.720p.HDTV.mkv"))
        .expect("expands");
    assert_eq!(range.stem, "Show Name - S02E05-E07");
    let cross = expand_sort_template(
        &root(),
        "{show}/{show} {season}x{episode:00}",
        &found("Futurama.4x12-13.DVDRip.avi"),
    )
    .expect("expands");
    assert_eq!(cross.stem, "Futurama 4x12-13");
}

#[test]
fn formats_and_missing_values() {
    let found = found("Breaking.Bad.S05E14.Ozymandias.720p.HDTV.x264-IMMERSE.mkv");
    let target = expand_sort_template(
        &root(),
        "{show:upper} ({year})/{show:dots}.S{season:000}E{episode}.{title:lower}",
        &found,
    )
    .expect("expands");
    // The show has no year: the brackets go with it.
    assert_eq!(target.directory, root().join("BREAKING BAD"));
    assert_eq!(target.stem, "Breaking.Bad.S005E14.ozymandias");
}

#[test]
fn templates_are_checked_when_they_are_saved() {
    let cases: &[(SortKind, &str, SortTemplateError)] = &[
        (SortKind::Series, "", SortTemplateError::Empty),
        (SortKind::Series, "   ", SortTemplateError::Empty),
        (
            SortKind::Series,
            "{show}/{movie}",
            SortTemplateError::UnknownField {
                field: "movie".to_owned(),
            },
        ),
        (
            SortKind::Movie,
            "{movie}/{season}",
            SortTemplateError::UnknownField {
                field: "season".to_owned(),
            },
        ),
        (
            SortKind::Series,
            "{show}/{season:abc}",
            SortTemplateError::UnknownFormat {
                field: "season".to_owned(),
                format: "abc".to_owned(),
            },
        ),
        (
            SortKind::Series,
            "{show:00}",
            SortTemplateError::UnknownFormat {
                field: "show".to_owned(),
                format: "00".to_owned(),
            },
        ),
        (SortKind::Series, "{show", SortTemplateError::Syntax),
        (SortKind::Series, "show}", SortTemplateError::Syntax),
        (SortKind::Series, "{{show}}", SortTemplateError::Syntax),
        (SortKind::Series, "a\\{show}", SortTemplateError::Syntax),
        (SortKind::Series, "../{show}", SortTemplateError::Outside),
        (
            SortKind::Series,
            "{show}/../{title}",
            SortTemplateError::Outside,
        ),
        (
            SortKind::Series,
            "{show}/./{title}",
            SortTemplateError::Outside,
        ),
        (SortKind::Series, "/srv/{show}", SortTemplateError::Outside),
        (
            SortKind::Series,
            "C:/media/{show}",
            SortTemplateError::Outside,
        ),
        (
            SortKind::Series,
            "\\\\server\\share\\{show}",
            SortTemplateError::Outside,
        ),
        (
            SortKind::Series,
            "{show}/Season {season}/episode",
            SortTemplateError::FixedName,
        ),
        (
            SortKind::Series,
            "a/b/c/d/e/f/g/h/{show}",
            SortTemplateError::TooDeep,
        ),
    ];
    for (kind, template, expected) in cases {
        assert_eq!(
            validate_sort_template(*kind, template).as_ref(),
            Err(expected),
            "{template}"
        );
    }
    assert_eq!(
        validate_sort_template(SortKind::Series, &"{show}".repeat(100)),
        Err(SortTemplateError::TooLong)
    );
    for (kind, template) in [
        (SortKind::Series, SERIES),
        (SortKind::Dated, DATED),
        (SortKind::Movie, MOVIE),
        (
            SortKind::Dated,
            "{show}/{year}-{month:00}-{day:00} {title:dots}",
        ),
    ] {
        assert_eq!(validate_sort_template(kind, template), Ok(()), "{template}");
    }
}

fn episode_named(show: &str, title: &str) -> SortMatch {
    SortMatch {
        kind: SortKind::Series,
        show: Some(show.to_owned()),
        season: Some(1),
        episodes: vec![1],
        date: None,
        title: Some(title.to_owned()),
        movie: None,
        year: None,
        resolution: None,
        source: None,
    }
}

fn below_root(target: &crate::SortTarget) -> bool {
    target.directory.starts_with(root())
        && !target
            .directory
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
}

/// The security boundary: whatever a name carries, nothing lands outside the category's folder.
#[test]
fn hostile_values_never_leave_the_root() {
    for show in ["..", "../../etc", "..\\..\\Windows", " .. ", "."] {
        assert_eq!(
            expand_sort_template(&root(), SERIES, &episode_named(show, "x")),
            Err(SortTemplateError::Outside),
            "{show:?}"
        );
    }
    // A separator inside a value is an ordinary character, written as `_` by the sanitiser.
    let cases: &[(&str, &str, &str)] = &[
        ("/etc/passwd", "a/../../b", "_etc_passwd"),
        ("C:\\Windows", "x", "C__Windows"),
        ("\\\\server\\share", "x", "__server_share"),
        ("Show\u{0}Name", "x", "Show_Name"),
    ];
    for (show, title, folder) in cases {
        let target = expand_sort_template(&root(), SERIES, &episode_named(show, title))
            .expect("a sanitised value expands");
        assert!(below_root(&target), "{show:?}: {target:?}");
        assert_eq!(
            target.directory,
            root().join(folder).join("Season 01"),
            "{show:?}"
        );
        assert!(!target.stem.contains(['/', '\\']), "{target:?}");
    }
    // Reserved device names on Windows, as folder and as file name.
    for show in ["CON", "nul", "COM1", "lpt9"] {
        let target = expand_sort_template(&root(), "{show}/{show}", &episode_named(show, "x"))
            .expect("expands");
        let expected = format!("_{show}");
        assert_eq!(target.directory, root().join(&expected));
        assert_eq!(target.stem, expected);
    }
    // Trailing dots and spaces, which Windows strips and so would merge two names.
    let target = expand_sort_template(&root(), SERIES, &episode_named("Show. ", "Title..."))
        .expect("expands");
    assert_eq!(target.directory, root().join("Show").join("Season 01"));
    // The dot inside the name stays; the ones at its end go.
    assert_eq!(target.stem, "Show. - S01E01 - Title");
}

#[test]
fn a_package_is_planned_with_its_companions() {
    let files: Vec<String> = [
        "Breaking.Bad.S05E14.Ozymandias.720p.HDTV.x264-IMMERSE.mkv",
        "Breaking.Bad.S05E14.Ozymandias.720p.HDTV.x264-IMMERSE.en.srt",
        "Breaking.Bad.S05E15.Granite.State.720p.HDTV.x264-IMMERSE.mkv",
        "Breaking.Bad.S05E15.Granite.State.720p.HDTV.x264-IMMERSE.nfo",
        "holiday.mp4",
        "readme.txt",
    ]
    .map(str::to_owned)
    .to_vec();
    let plan = plan_sort(&root(), "Breaking.Bad.S05", &files, &templates());
    let season = root().join("Breaking Bad").join("Season 05");
    let mut moves: Vec<(String, PathBuf)> = plan
        .moves
        .iter()
        .map(|entry| (entry.from.clone(), entry.to.clone()))
        .collect();
    moves.sort();
    let mut expected = vec![
        (
            files[0].clone(),
            season.join("Breaking Bad - S05E14 - Ozymandias.mkv"),
        ),
        (
            files[1].clone(),
            season.join("Breaking Bad - S05E14 - Ozymandias.en.srt"),
        ),
        (
            files[2].clone(),
            season.join("Breaking Bad - S05E15 - Granite State.mkv"),
        ),
        (
            files[3].clone(),
            season.join("Breaking Bad - S05E15 - Granite State.nfo"),
        ),
    ];
    expected.sort();
    assert_eq!(moves, expected);
    // The unrecognised video and the other file stay where they are.
    assert_eq!(plan.unsorted, vec!["holiday.mp4".to_owned()]);
    assert!(plan.refused.is_empty());
}

#[test]
fn an_obfuscated_single_video_is_named_after_its_package() {
    let files: Vec<String> = ["a8f3c2e1.mkv", "Subs/English.srt", "info.nfo"]
        .map(str::to_owned)
        .to_vec();
    let plan = plan_sort(
        &root(),
        "Inception.2010.1080p.BluRay.x264-SPARKS",
        &files,
        &templates(),
    );
    let folder = root().join("Inception (2010)");
    let mut moves: Vec<(String, PathBuf)> = plan
        .moves
        .iter()
        .map(|entry| (entry.from.clone(), entry.to.clone()))
        .collect();
    moves.sort();
    assert_eq!(
        moves,
        vec![
            (
                "Subs/English.srt".to_owned(),
                folder.join("Inception (2010).English.srt")
            ),
            (
                "a8f3c2e1.mkv".to_owned(),
                folder.join("Inception (2010).mkv")
            ),
            ("info.nfo".to_owned(), folder.join("Inception (2010).nfo")),
        ]
    );
}

#[test]
fn a_kind_without_a_template_is_left_alone() {
    let only_films = SortTemplates {
        series: None,
        dated: None,
        movie: Some(MOVIE.to_owned()),
    };
    let files = vec!["Lost.S01E01-E02.Pilot.720p.BluRay.x264-SiNNERS.mkv".to_owned()];
    let plan = plan_sort(&root(), "Lost", &files, &only_films);
    assert!(plan.moves.is_empty());
    assert_eq!(plan.unsorted, files);
}
