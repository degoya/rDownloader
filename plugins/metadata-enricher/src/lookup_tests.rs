//! Building the addresses and turning the source's answers into fields.

use super::{
    FIELD_GENRE, FIELD_RATING, FIELD_RUNTIME, FIELD_SERIES, FIELD_TITLE, FIELD_YEAR, Kind,
    catalogue_url, from_catalogue, from_meta, merge, meta_url, wants_detail,
};

const CATALOGUE: &str = r#"{"metas":[
        {"id":"tt0083658","type":"movie","name":"Blade Runner","releaseInfo":"1982",
         "imdbRating":"8.1","genres":["Action","Drama","Sci-Fi","Thriller"]},
        {"id":"tt1856101","type":"movie","name":"Blade Runner 2049","releaseInfo":"2017",
         "imdbRating":"8.0","genres":["Action","Drama","Mystery"]}
    ]}"#;

fn value<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(field, _)| field == name)
        .map(|(_, value)| value.as_str())
}

#[test]
fn the_address_carries_the_title_and_can_never_leave_its_path_segment() {
    let url = catalogue_url(Kind::Movie, "Some Film").expect("a query");
    assert_eq!(
        url,
        "https://v3-cinemeta.strem.io/catalog/movie/top/search=Some%20Film.json"
    );
    // A title full of path characters is reduced before it is encoded, so neither a slash
    // nor a dot nor a percent survives into the address.
    let hostile = catalogue_url(Kind::Series, "../../etc/passwd?x=1").expect("a query");
    assert!(hostile.starts_with("https://v3-cinemeta.strem.io/catalog/series/top/search="));
    assert_eq!(hostile.matches('/').count(), 6, "{hostile}");
    assert!(!hostile.contains('%') || hostile.contains("%20"));
    assert!(hostile.ends_with(".json"));
}

#[test]
fn a_title_with_nothing_to_search_for_produces_no_address_at_all() {
    assert_eq!(catalogue_url(Kind::Movie, ""), None);
    assert_eq!(catalogue_url(Kind::Movie, "2019"), None);
    assert_eq!(catalogue_url(Kind::Movie, "-- --"), None);
}

#[test]
fn an_identifier_is_checked_before_it_becomes_part_of_an_address() {
    assert_eq!(
        meta_url(Kind::Movie, "tt1856101").as_deref(),
        Some("https://v3-cinemeta.strem.io/meta/movie/tt1856101.json")
    );
    for hostile in ["tt../../x", "kitsu:1", "tt", "", "tt12345678901234"] {
        assert_eq!(meta_url(Kind::Movie, hostile), None, "{hostile}");
    }
}

#[test]
fn the_year_in_the_name_decides_which_entry_the_row_describes() {
    // Without this the source's own ranking wins and the row says 1982 under a file
    // named 2017, which is worse than saying nothing.
    let found =
        from_catalogue(CATALOGUE, Kind::Movie, "Blade Runner 2049", Some(2017)).expect("a match");
    assert_eq!(value(&found.fields, FIELD_TITLE), Some("Blade Runner 2049"));
    assert_eq!(value(&found.fields, FIELD_YEAR), Some("2017"));
    assert_eq!(value(&found.fields, FIELD_RATING), Some("8.0"));
    assert_eq!(
        value(&found.fields, FIELD_GENRE),
        Some("Action, Drama, Mystery")
    );
    assert_eq!(found.id.as_deref(), Some("tt1856101"));

    let first = from_catalogue(CATALOGUE, Kind::Movie, "Blade Runner", None).expect("a match");
    assert_eq!(value(&first.fields, FIELD_TITLE), Some("Blade Runner"));
}

#[test]
fn a_series_answer_lands_under_the_series_name_rather_than_a_film_title() {
    const BODY: &str = r#"{"metas":[{"id":"tt0903747","type":"series","name":"Breaking Bad",
            "releaseInfo":"2008-2013","imdbRating":"9.5","genres":["Crime","Drama"]}]}"#;
    let found = from_catalogue(BODY, Kind::Series, "Breaking Bad", None).expect("a match");
    assert_eq!(value(&found.fields, FIELD_SERIES), Some("Breaking Bad"));
    assert_eq!(value(&found.fields, FIELD_TITLE), None);
    // `2008-2013` is a run, and the year that belongs on the row is where it started.
    assert_eq!(value(&found.fields, FIELD_YEAR), Some("2008"));
}

#[test]
fn a_hit_whose_name_is_not_the_one_searched_for_yields_no_field_at_all() {
    // The two answers RD-108-14 was reported with. Cinemeta's search is fuzzy and both of
    // these are what it really hands back; taking the first hit unseen wrote a 1994 cinema
    // release next to a 2024 episode, and another series' name next to an episode of this
    // one. Neither name reads like the one searched for, so there is no field.
    const PARIS: &str = r#"{"metas":[{"id":"tt21371866","type":"series",
            "name":"Paris Has Fallen","releaseInfo":"2024","imdbRating":"6.5",
            "genres":["Action","Drama"],"runtime":"48 min"}]}"#;
    assert!(from_catalogue(PARIS, Kind::Series, "Apollo Has Fallen", None).is_none());

    const BROLY: &str = r#"{"metas":[{"id":"tt0142242","type":"movie",
            "name":"Dragon Ball Z: Broly - Second Coming","releaseInfo":"1994",
            "imdbRating":"6.5","genres":["Animation"],"runtime":"48 min"}]}"#;
    assert!(from_catalogue(BROLY, Kind::Series, "Dragon Ball DAIMA", None).is_none());
    // And not even with the film catalogue's own answer, which is what the release used to
    // be looked up in.
    assert!(from_catalogue(BROLY, Kind::Movie, "Dragon Ball DAIMA", None).is_none());
}

#[test]
fn a_name_the_catalogue_spells_differently_is_still_the_same_release() {
    // The other half of the bar. Too strict and a real hit written with one more word, or
    // with the punctuation the catalogue prefers, loses its row for nothing.
    const LONGER: &str = r#"{"metas":[{"id":"tt9603212","type":"movie",
            "name":"Mission: Impossible - Dead Reckoning Part One","releaseInfo":"2023",
            "imdbRating":"7.7","genres":["Action"],"runtime":"163 min"}]}"#;
    let found = from_catalogue(
        LONGER,
        Kind::Movie,
        "Mission Impossible Dead Reckoning",
        None,
    )
    .expect("the same release");
    assert_eq!(
        value(&found.fields, FIELD_TITLE),
        Some("Mission: Impossible - Dead Reckoning Part One")
    );

    // A title the release name cut short at a marker word. The name alone would not carry
    // it; the year in the name agreeing exactly is what does.
    const DUNE: &str = r#"{"metas":[{"id":"tt15239678","type":"movie","name":"Dune: Part Two",
            "releaseInfo":"2024","imdbRating":"8.5","genres":["Sci-Fi"]}]}"#;
    assert!(from_catalogue(DUNE, Kind::Movie, "Dune", None).is_none());
    let found = from_catalogue(DUNE, Kind::Movie, "Dune", Some(2024)).expect("the same film");
    assert_eq!(value(&found.fields, FIELD_TITLE), Some("Dune: Part Two"));
}

#[test]
fn a_title_the_catalogue_writes_properly_is_the_one_the_release_spells_in_ascii() {
    // Release names are ASCII by convention and catalogues are not, so without folding the
    // two share no word at all and a perfectly good hit is thrown away.
    const SHOGUN: &str = r#"{"metas":[{"id":"tt2788316","type":"series",
            "name":"Sh\u00f4gun","releaseInfo":"2024","imdbRating":"8.6",
            "genres":["Drama"]}]}"#;
    let found = from_catalogue(SHOGUN, Kind::Series, "Shogun", None).expect("the same series");
    assert_eq!(value(&found.fields, FIELD_SERIES), Some("Sh\u{f4}gun"));

    const POKEMON: &str = r#"{"metas":[{"id":"tt0168366","type":"series",
            "name":"Pok\u00e9mon","releaseInfo":"1997","imdbRating":"7.5",
            "genres":["Animation"]}]}"#;
    let found = from_catalogue(POKEMON, Kind::Series, "Pokemon", None).expect("the same series");
    assert_eq!(value(&found.fields, FIELD_SERIES), Some("Pok\u{e9}mon"));
}

#[test]
fn a_bracketed_qualifier_does_not_cost_a_one_word_title_its_row() {
    // The bar bites hardest on short titles: one word against two is 66 whatever the words
    // are, and a series has no year in its name to be rescued by. The qualifier a catalogue
    // adds to tell two like-named entries apart is not part of the title.
    const SKINS: &str = r#"{"metas":[{"id":"tt0840196","type":"series","name":"Skins (UK)",
            "releaseInfo":"2007-2013","imdbRating":"8.0","genres":["Drama"]}]}"#;
    let found = from_catalogue(SKINS, Kind::Series, "Skins", None).expect("the same series");
    assert_eq!(value(&found.fields, FIELD_SERIES), Some("Skins (UK)"));

    // And it is only the bracket that is forgiven, not the name in front of it.
    const OTHER: &str = r#"{"metas":[{"id":"tt1586680","type":"series",
            "name":"Shameless (UK)","releaseInfo":"2004-2013","imdbRating":"8.3",
            "genres":["Comedy"]}]}"#;
    assert!(from_catalogue(OTHER, Kind::Series, "Skins", None).is_none());
}

#[test]
fn a_word_that_would_fold_onto_an_article_is_not_thrown_away() {
    // "Loss" folds to "los" and "Dies" to "die", which are a Spanish and a German article.
    // Dropping the articles first and folding the plural afterwards is what keeps such a
    // title from normalising to nothing and failing against its own name.
    const LOSS: &str = r#"{"metas":[{"id":"tt1234567","type":"movie","name":"Loss",
            "releaseInfo":"2008","imdbRating":"6.9","genres":["Drama"]}]}"#;
    let found = from_catalogue(LOSS, Kind::Movie, "Loss", None).expect("its own name");
    assert_eq!(value(&found.fields, FIELD_TITLE), Some("Loss"));
    // And the word still has to be there to be compared with, not merely rescued by the
    // whole-string fallback: an article in front of it and the two must still meet.
    let found = from_catalogue(LOSS, Kind::Movie, "The Loss", None).expect("its own name");
    assert_eq!(value(&found.fields, FIELD_TITLE), Some("Loss"));
}

#[test]
fn nonsense_from_the_source_leaves_the_row_exactly_as_it_was() {
    for body in [
        "",
        "<html>502 Bad Gateway</html>",
        "{}",
        r#"{"metas":[]}"#,
        r#"{"metas":"soon"}"#,
        "null",
    ] {
        assert!(
            from_catalogue(body, Kind::Movie, "Blade Runner", None).is_none(),
            "{body} must yield nothing"
        );
        assert!(from_meta(body).is_empty(), "{body} must yield nothing");
    }
}

#[test]
fn a_value_that_is_not_what_it_claims_to_be_is_left_out_rather_than_shown() {
    const BODY: &str = r#"{"metas":[{"id":"x","type":"movie","name":"Odd",
            "releaseInfo":"soon","imdbRating":"excellent","genres":[1,2],"runtime":"  "}]}"#;
    let found = from_catalogue(BODY, Kind::Movie, "Odd", None).expect("an entry");
    assert_eq!(value(&found.fields, FIELD_TITLE), Some("Odd"));
    assert_eq!(value(&found.fields, FIELD_YEAR), None);
    assert_eq!(value(&found.fields, FIELD_RATING), None);
    assert_eq!(value(&found.fields, FIELD_GENRE), None);
    assert_eq!(value(&found.fields, FIELD_RUNTIME), None);
    // An identifier that is not the source's own shape is not kept, so no second request
    // can be built from it.
    assert_eq!(found.id, None);
}

#[test]
fn the_detail_fills_what_the_catalogue_left_out_and_overwrites_nothing() {
    let mut found =
        from_catalogue(CATALOGUE, Kind::Movie, "Blade Runner 2049", Some(2017)).expect("a match");
    assert!(wants_detail(&found.fields), "runtime is still missing");
    const DETAIL: &str = r#"{"meta":{"id":"tt1856101","type":"movie","name":"Blade Runner 2049",
            "releaseInfo":"2017","imdbRating":"7.9","runtime":"164 min",
            "genres":["Sci-Fi"]}}"#;
    merge(&mut found.fields, from_meta(DETAIL));
    assert_eq!(value(&found.fields, FIELD_RUNTIME), Some("164 min"));
    // The catalogue said 8.0 first, and a second answer does not get to move it.
    assert_eq!(value(&found.fields, FIELD_RATING), Some("8.0"));
    assert!(!wants_detail(&found.fields));
}
