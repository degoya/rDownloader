//! The sidecar reader's tests, against the MD5 plugin's [`Algorithm`].

use std::collections::BTreeSet;

use super::{Algorithm, Entry, Plan, Unverifiable, resolve};

/// The MD5 plugin's shape; the tests are the ones it carried before the two plugins shared this.
const MD5: Algorithm = Algorithm {
    extension: ".md5",
    digest_hex_len: 32,
    slug: "md5_postprocess",
    label: "MD5",
};

/// MD5 of the empty input, used only for its shape.
const DIGEST: &str = "d41d8cd98f00b204e9800998ecf8427e";

fn is_sidecar(name: &str) -> bool {
    MD5.is_sidecar(name)
}

fn parse(text: &str) -> Vec<Entry> {
    MD5.parse(text)
}

fn wanted(
    files: &BTreeSet<&str>,
    removed: &BTreeSet<&str>,
    sidecar: &str,
    text: &str,
) -> Result<Plan, Unverifiable> {
    MD5.wanted(files, removed, sidecar, text)
}

#[test]
fn a_sidecar_is_recognised_by_its_extension() {
    assert!(is_sidecar("release.md5"));
    assert!(is_sidecar("RELEASE.MD5"));
    assert!(!is_sidecar("release.sfv"));
    assert!(!is_sidecar("md5"));
}

#[test]
fn the_usual_two_space_form_is_read() {
    assert_eq!(
        parse(&format!("{DIGEST}  release.bin\n")),
        vec![Entry {
            digest: DIGEST.to_owned(),
            file: "release.bin".to_owned(),
        }]
    );
}

#[test]
fn binary_mode_and_upper_case_digests_are_accepted() {
    let entries = parse(&format!("{}  *release.bin", DIGEST.to_uppercase()));
    assert_eq!(entries[0].digest, DIGEST);
    assert_eq!(entries[0].file, "release.bin");
}

#[test]
fn a_malformed_line_costs_only_itself() {
    let text = format!("# a comment\n\nnothex  release.bin\n{DIGEST}  good.bin\n");
    let entries = parse(&text);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].file, "good.bin");
}

fn package<'a>(files: &[&'a str]) -> BTreeSet<&'a str> {
    files.iter().copied().collect()
}

/// Nothing removed before the step.
fn none() -> BTreeSet<&'static str> {
    BTreeSet::new()
}

#[test]
fn a_top_level_entry_keeps_its_name() {
    assert_eq!(
        resolve("release.md5", "release.bin").as_deref(),
        Some("release.bin")
    );
    assert_eq!(
        resolve("release.md5", "CD1/a.bin").as_deref(),
        Some("CD1/a.bin")
    );
}

#[test]
fn an_entry_in_a_subfolder_is_read_from_the_sidecars_folder() {
    assert_eq!(
        resolve("Film/film.md5", "film.mkv").as_deref(),
        Some("Film/film.mkv")
    );
    assert_eq!(
        resolve("A/B/x.md5", "./c\\d.bin").as_deref(),
        Some("A/B/c/d.bin")
    );
}

#[test]
fn an_entry_that_could_leave_its_folder_is_not_resolved() {
    assert_eq!(resolve("Film/film.md5", "../other.bin"), None);
    assert_eq!(resolve("Film/film.md5", "a/../../b.bin"), None);
    assert_eq!(resolve("Film/film.md5", "/etc/passwd"), None);
    assert_eq!(resolve("Film/film.md5", "\\server\\x.bin"), None);
    assert_eq!(resolve("Film/film.md5", "C:\\film.mkv"), None);
}

#[test]
fn a_sidecar_in_a_subfolder_checks_the_files_beside_it() {
    let files = package(&["Film/film.mkv", "Film/film.md5", "film.mkv"]);
    let entries = wanted(
        &files,
        &none(),
        "Film/film.md5",
        &format!("{DIGEST}  film.mkv\n"),
    );
    assert_eq!(
        entries,
        Ok(Plan {
            entries: vec![Entry {
                digest: DIGEST.to_owned(),
                file: "Film/film.mkv".to_owned(),
            }],
            unchecked: Vec::new(),
        })
    );
}

#[test]
fn a_top_level_sidecar_reads_as_before() {
    let files = package(&["release.bin", "release.md5"]);
    let entries = wanted(
        &files,
        &none(),
        "release.md5",
        &format!("{DIGEST}  release.bin\n"),
    );
    assert_eq!(
        entries.map(|plan| plan.entries[0].file.clone()),
        Ok("release.bin".to_owned())
    );
}

#[test]
fn a_listed_file_the_package_lacks_is_named_when_others_verify() {
    // A release split across two packages: this one holds the first part only.
    let files = package(&["release.md5", "CD1/a.bin"]);
    let text = format!("{DIGEST}  CD1/a.bin\n{DIGEST}  CD2/b.bin\n{DIGEST}  ../c.bin\n");
    let plan = wanted(&files, &none(), "release.md5", &text).expect("plan");
    assert_eq!(plan.entries.len(), 1);
    assert_eq!(
        plan.unchecked,
        vec!["CD2/b.bin".to_owned(), "../c.bin".to_owned()]
    );
}

#[test]
fn a_sidecar_none_of_whose_files_is_there_fails() {
    // `film.mkv` exists, but at the top, not beside the sidecar that names it.
    let files = package(&["Film/film.md5", "film.mkv"]);
    let text = format!("{DIGEST}  film.mkv\n{DIGEST}  ../film.mkv\n");
    assert_eq!(
        wanted(&files, &none(), "Film/film.md5", &text),
        Err(Unverifiable::Missing("film.mkv".to_owned()))
    );
}

#[test]
fn files_the_host_says_were_removed_are_not_missing() {
    let files = package(&["release.md5", "release/film.mkv"]);
    let removed = package(&[
        "release.part1.rar",
        "release.vol0+1.par2",
        "release/film.nfo",
    ]);
    let text = format!(
        "{DIGEST}  release.part1.rar\n{DIGEST}  release.vol0+1.par2\n\
         {DIGEST}  release/film.nfo\n"
    );
    assert_eq!(
        wanted(&files, &removed, "release.md5", &text),
        Ok(Plan::default())
    );
}

#[test]
fn a_removed_file_is_found_from_a_sidecar_in_a_subfolder() {
    // The cleanup deleted `Film/film.nfo`; the sidecar beside it says `film.nfo`.
    let files = package(&["Film/film.md5", "Film/film.mkv"]);
    let removed = package(&["Film/film.nfo"]);
    let text = format!("{DIGEST}  film.mkv\n{DIGEST}  film.nfo\n");
    let plan = wanted(&files, &removed, "Film/film.md5", &text).expect("plan");
    assert_eq!(plan.entries.len(), 1);
    assert!(plan.unchecked.is_empty(), "{plan:?}");
}

#[test]
fn a_volume_nobody_removed_is_not_waved_through_by_its_extension() {
    // Before RD-190-06 a `.rar` entry was skipped for its extension alone. A volume the
    // package lacks without the pipeline having removed it is a gap like any other file.
    let files = package(&["release.md5", "release.bin"]);
    let text = format!("{DIGEST}  release.bin\n{DIGEST}  release.part2.rar\n");
    let plan = wanted(&files, &none(), "release.md5", &text).expect("plan");
    assert_eq!(plan.unchecked, vec!["release.part2.rar".to_owned()]);
}

#[test]
fn a_sidecar_without_a_readable_line_is_not_a_pass() {
    let files = package(&["release.md5"]);
    assert_eq!(
        wanted(&files, &none(), "release.md5", "# nothing here\n"),
        Err(Unverifiable::Empty)
    );
}

#[test]
fn codes_carry_the_plugins_slug() {
    assert_eq!(MD5.code("mismatch"), "md5_postprocess.mismatch");
}

#[test]
fn a_digest_of_another_algorithms_length_is_not_an_entry() {
    let sha256 = Algorithm {
        extension: ".sha256",
        digest_hex_len: 64,
        slug: "sha256_postprocess",
        label: "SHA-256",
    };
    assert!(sha256.is_sidecar("release.SHA256"));
    assert!(!sha256.is_sidecar("release.md5"));
    assert!(sha256.parse(&format!("{DIGEST}  release.bin\n")).is_empty());
}
