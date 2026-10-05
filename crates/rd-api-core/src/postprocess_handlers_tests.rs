use super::validate_script_name;

#[test]
fn rejects_paths_and_hidden_files() {
    assert!(validate_script_name(Some("../evil.sh".to_owned())).is_err());
    assert!(validate_script_name(Some("dir/run.sh".to_owned())).is_err());
    assert!(validate_script_name(Some(".hidden".to_owned())).is_err());
    assert_eq!(
        validate_script_name(Some(" rename-files.py ".to_owned())).expect("valid"),
        Some("rename-files.py".to_owned())
    );
    assert_eq!(
        validate_script_name(Some("  ".to_owned())).expect("empty"),
        None
    );
}

fn templates() -> rd_core::SortTemplates {
    rd_core::SortTemplates {
        series: Some(
            "{show}/Season {season:00}/{show} - S{season:00}E{episode:00} - {title}".to_owned(),
        ),
        dated: None,
        movie: Some("{movie} ({year})/{movie} ({year})".to_owned()),
    }
}

#[tokio::test]
async fn the_preview_shows_where_each_name_would_land() {
    let axum::Json(answer) =
        super::preview_category_sorting(axum::Json(crate::dto::SortPreviewRequest {
            sorting: templates(),
            names: vec![
                "Lost.S01E01-E02.Pilot.720p.BluRay.x264-SiNNERS.mkv".to_owned(),
                "Inception.2010.1080p.BluRay.x264-SPARKS".to_owned(),
                "The.Daily.Show.2024.03.15.Guest.720p.WEB.h264-EDITH.mkv".to_owned(),
                "holiday.mp4".to_owned(),
            ],
        }))
        .await
        .expect("preview");
    let paths: Vec<Option<&str>> = answer
        .entries
        .iter()
        .map(|entry| entry.path.as_deref())
        .collect();
    assert_eq!(
        paths,
        vec![
            Some("Lost/Season 01/Lost - S01E01-E02 - Pilot.mkv"),
            Some("Inception (2010)/Inception (2010)"),
            None,
            None,
        ]
    );
    assert_eq!(answer.entries[2].code.as_deref(), Some("sort.no_template"));
    assert_eq!(answer.entries[3].kind, None);
    assert_eq!(
        answer.entries[0].fields.get("episode").map(String::as_str),
        Some("1-2")
    );
    assert!(answer.fields["movie"].contains(&"movie".to_owned()));
}

#[tokio::test]
async fn the_preview_refuses_a_template_that_leaves_the_folder() {
    let mut sorting = templates();
    sorting.series = Some("../{show}/{title}".to_owned());
    let refused = super::preview_category_sorting(axum::Json(crate::dto::SortPreviewRequest {
        sorting,
        names: vec!["Lost.S01E01.Pilot.mkv".to_owned()],
    }))
    .await
    .err()
    .expect("refused");
    assert_eq!(refused.code(), "sort.template_outside");
}

#[test]
fn blank_sort_templates_are_no_sorting() {
    let blank = rd_core::SortTemplates {
        series: Some(" ".to_owned()),
        dated: None,
        movie: None,
    };
    assert_eq!(super::validate_sorting(Some(blank)).expect("valid"), None);
}
