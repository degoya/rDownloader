use std::path::Path;

use super::{final_path_line, output_template, progressive_format, timed_out};

/// RA-TR-03: a deadline is a failure with a retry, not a stop.
#[test]
fn a_run_past_its_deadline_is_a_retryable_failure() {
    let failure = timed_out("");
    assert_eq!(failure.code.as_deref(), Some("media.ytdlp_failed"));
    assert!(failure.category.is_retryable());
    assert!(
        failure.message.contains("time limit"),
        "{}",
        failure.message
    );
    assert!(
        timed_out("WARNING: slow\nERROR: stalled\n")
            .message
            .contains("ERROR: stalled")
    );
}

#[test]
fn keeps_only_the_alternative_that_needs_no_merge() {
    assert_eq!(
        progressive_format("bv*[height<=1080]+ba/b[height<=1080]"),
        "b[height<=1080]"
    );
    assert_eq!(progressive_format("bv*+ba/b"), "b");
    // Nothing pre-muxed on offer: fall back to yt-dlp's own "best single file".
    assert_eq!(progressive_format("bv*+ba"), "b");
    assert_eq!(progressive_format("b"), "b");
}

#[test]
fn without_a_template_the_output_is_the_plain_file_name() {
    let values = rd_files::TemplateValues::new();
    assert_eq!(
        output_template(Path::new("/downloads/pkg"), "clip", None, &values),
        Path::new("/downloads/pkg/clip.%(ext)s")
    );
    // An empty pattern is the same as none, not an empty path.
    assert_eq!(
        output_template(Path::new("/downloads/pkg"), "clip", Some("   "), &values),
        Path::new("/downloads/pkg/clip.%(ext)s")
    );
}

#[test]
fn a_template_produces_a_literal_path_with_only_ytdlps_extension_placeholder() {
    let mut values = rd_files::TemplateValues::new();
    values.insert("title".to_owned(), "Trailer".to_owned());
    values.insert("uploader".to_owned(), "Studio".to_owned());
    let path = output_template(
        Path::new("/downloads/pkg"),
        "clip",
        Some("{uploader}/{title}"),
        &values,
    );
    assert_eq!(path, Path::new("/downloads/pkg/Studio/Trailer.%(ext)s"));
    // Nothing but the extension placeholder survives into the argument.
    let rendered = path.to_string_lossy();
    assert_eq!(rendered.matches('%').count(), 1);
}

#[test]
fn a_per_cent_in_a_name_is_escaped_so_yt_dlp_cannot_read_it_as_a_field() {
    // `sanitize_file_name` leaves `%` alone, so without the doubling this title reaches
    // yt-dlp as a real output template and the file lands under a name nobody chose.
    let values = rd_files::TemplateValues::new();
    let path = output_template(
        Path::new("/downloads/pkg"),
        "50%(title)s off",
        None,
        &values,
    );
    assert_eq!(path, Path::new("/downloads/pkg/50%%(title)s off.%(ext)s"));

    let mut values = rd_files::TemplateValues::new();
    values.insert("title".to_owned(), "100% Wolf".to_owned());
    let path = output_template(
        Path::new("/downloads/pkg"),
        "clip",
        Some("{title}"),
        &values,
    );
    assert_eq!(path, Path::new("/downloads/pkg/100%% Wolf.%(ext)s"));
    // The only unescaped placeholder left is the extension yt-dlp fills in.
    assert_eq!(path.to_string_lossy().matches("%(").count(), 1);
}

#[test]
fn only_the_marked_line_is_taken_as_the_output_path() {
    assert_eq!(
        final_path_line("rdownloader-final-path:/downloads/pkg/clip.mp4"),
        Some("/downloads/pkg/clip.mp4")
    );
    // The lines that used to overwrite the path: anything unprefixed on stdout.
    assert_eq!(final_path_line("/downloads/pkg/wrong.mp4"), None);
    assert_eq!(final_path_line("WARNING: generic extractor"), None);
    assert_eq!(
        final_path_line("[download] Destination: clip.f137.mp4"),
        None
    );
    assert_eq!(final_path_line(""), None);
    assert_eq!(final_path_line("   "), None);
    // A marker with nothing behind it is not an answer either.
    assert_eq!(final_path_line("rdownloader-final-path:"), None);
    assert_eq!(final_path_line("rdownloader-final-path:   "), None);
}

#[test]
fn a_template_that_cannot_be_expanded_falls_back_instead_of_failing_the_download() {
    // The template was validated when it was saved; a title of `..` is a problem with
    // one page, not with the configuration, so the file still lands somewhere sane.
    let mut values = rd_files::TemplateValues::new();
    values.insert("title".to_owned(), "..".to_owned());
    assert_eq!(
        output_template(
            Path::new("/downloads/pkg"),
            "clip",
            Some("{title}"),
            &values
        ),
        Path::new("/downloads/pkg/clip.%(ext)s")
    );
}
