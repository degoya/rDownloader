use std::path::Path;

use super::{
    MAX_PATH_UTF16_UNITS, extraction_subfolder, extraction_subfolders, package_directory,
    package_name_from_file_name, renamed_package_directory, sanitize_file_name,
    sanitize_file_name_within,
};

#[test]
fn strips_known_extensions_from_package_names() {
    assert_eq!(
        package_name_from_file_name("Great Video.mp4"),
        "Great Video"
    );
    assert_eq!(package_name_from_file_name("backup.tar.gz"), "backup");
    assert_eq!(package_name_from_file_name("Show.2023"), "Show.2023");
    assert_eq!(
        package_name_from_file_name("Show.S01.1080p.WEB-DL"),
        "Show.S01.1080p.WEB-DL"
    );
    assert_eq!(package_name_from_file_name(".mp4"), ".mp4");
    assert_eq!(package_name_from_file_name("watch"), "watch");
}

#[test]
fn sanitizes_windows_names() {
    assert_eq!(sanitize_file_name("CON.txt"), "_CON.txt");
    assert_eq!(sanitize_file_name("bad<name>. "), "bad_name_");
    assert_eq!(sanitize_file_name("   "), "download");
}

/// Audit 2026-10-05, S3: a base of four bytes was sliced at byte three, which panicked
/// inside any multi-byte character there.
#[test]
fn a_multi_byte_name_of_four_bytes_is_kept_without_a_panic() {
    for name in [
        "\u{1f600}.jpg",
        "42\u{b0}.txt",
        "\u{e4}\u{f6}.mkv",
        "CO\u{e9}.bin",
        "COM\u{20ac}.txt",
        "\u{1f600}",
    ] {
        assert_eq!(sanitize_file_name(name), name);
    }
}

/// Audit 2026-10-05, S20: the console devices and the superscript ports are reserved on
/// Windows as well, and `COM0`/`LPT0` since Microsoft's list names them.
#[test]
fn every_windows_device_name_is_prefixed() {
    for name in [
        "CONIN$",
        "conout$.txt",
        "COM0.log",
        "COM9",
        "COM\u{b9}.txt",
        "lpt\u{b2}",
        "LPT\u{b3}.tar.gz",
        "nul .txt",
    ] {
        assert_eq!(sanitize_file_name(name), format!("_{name}"));
    }
    for name in [
        "COM10.txt",
        "LPT",
        "COMA.txt",
        "CONIN.txt",
        "COM\u{b9}x.txt",
    ] {
        assert_eq!(sanitize_file_name(name), name);
    }
}

#[test]
fn replaces_broken_decoding_markers() {
    assert_eq!(
        sanitize_file_name("251K views \u{FFFD} 562"),
        "251K views _ 562"
    );
}

#[test]
fn keeps_the_whole_path_inside_the_budget() {
    // The title that made yt-dlp fail with "unable to open for writing" on Windows.
    let title = "251K views \u{FFFD} 562 reactions _ Seit Jahren reisen Menschen nach \
                 Altschauerberg, um dort ein ganz besonderes Haus zu sehen und heute \
                 zeigen wir euch warum das so ist und was es damit auf sich hat";
    let directory = Path::new("C:\\Tools\\rdownloader\\downloads\\facebook.com");
    let name = sanitize_file_name_within(directory, title, 20);
    let full = directory
        .join(&name)
        .to_string_lossy()
        .encode_utf16()
        .count();
    assert!(
        full + 20 <= MAX_PATH_UTF16_UNITS,
        "path {full} + reserve exceeds the budget: {name}"
    );
    assert!(!name.is_empty());
    assert!(!name.contains('\u{FFFD}'));
}

#[test]
fn a_long_package_name_leaves_room_for_its_files() {
    let base = Path::new("C:\\Tools\\rdownloader\\downloads");
    let directory = package_directory(base, &"very long package name ".repeat(20));
    let used = directory.to_string_lossy().encode_utf16().count();
    assert!(
        MAX_PATH_UTF16_UNITS - used >= 24,
        "package folder leaves only {} units for file names",
        MAX_PATH_UTF16_UNITS - used
    );
}

#[test]
fn a_short_name_is_left_alone() {
    let directory = Path::new("/downloads/youtube.com");
    assert_eq!(
        sanitize_file_name_within(directory, "clip.mp4", 20),
        "clip.mp4"
    );
}

#[test]
fn a_renamed_package_folder_stays_beside_the_one_it_replaces() {
    let current = Path::new("/downloads/movies/Old Name");
    assert_eq!(
        renamed_package_directory(current, "New Name"),
        Some(Path::new("/downloads/movies/New Name").to_path_buf())
    );
}

/// The rename runs the requested name through the very same rules a new folder gets, so a
/// separator in it can never walk the package out of its category directory.
#[test]
fn a_rename_cannot_escape_the_category_directory() {
    let current = Path::new("/downloads/movies/Old Name");
    let renamed = renamed_package_directory(current, "../../etc/Escaped").expect("renamed");
    assert_eq!(renamed.parent(), Some(Path::new("/downloads/movies")));
    assert_eq!(
        renamed.file_name().and_then(|value| value.to_str()),
        Some(".._.._etc_Escaped")
    );
}

#[test]
fn a_root_directory_has_nothing_to_rename_inside() {
    assert_eq!(renamed_package_directory(Path::new("/"), "New Name"), None);
}

#[test]
fn an_extraction_subfolder_is_named_after_its_archive() {
    let package = tempfile::tempdir().expect("tempdir");
    assert_eq!(
        extraction_subfolder(package.path(), "Film"),
        package.path().join("Film")
    );
    // A separator in a downloaded name cannot walk out of the package folder.
    assert_eq!(
        extraction_subfolder(package.path(), "../../etc/x"),
        package.path().join(".._.._etc_x")
    );
    // Nothing usable left: the fixed fallback name, never the package folder itself.
    assert_eq!(
        extraction_subfolder(package.path(), " .. "),
        package.path().join("download")
    );
}

#[test]
fn an_existing_extraction_subfolder_is_reused_and_a_file_is_stepped_around() {
    let package = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(package.path().join("Film")).expect("folder");
    std::fs::write(package.path().join("Extras"), b"not a folder").expect("file");
    // A rerun after a crash merges into what the first run created.
    assert_eq!(
        extraction_subfolder(package.path(), "Film"),
        package.path().join("Film")
    );
    let stepped = extraction_subfolder(package.path(), "Extras");
    assert_eq!(stepped, package.path().join("Extras (1)"));
    // Deterministic: once that folder exists, the same name leads to it again.
    std::fs::create_dir(&stepped).expect("folder");
    assert_eq!(extraction_subfolder(package.path(), "Extras"), stepped);
}

#[test]
fn two_sets_with_one_base_get_two_folders_and_keep_them() {
    let package = tempfile::tempdir().expect("tempdir");
    let directory = package.path();
    // `Film.zip` and `Film.rar`, and a third whose name differs only in case and in what
    // sanitising drops: one folder on Windows, so one folder each everywhere.
    let bases = ["Film", "Film", "FILM ", "Extras"];
    let folders = extraction_subfolders(directory, &bases);
    assert_eq!(
        folders,
        vec![
            directory.join("Film"),
            directory.join("Film (1)"),
            directory.join("FILM (2)"),
            directory.join("Extras"),
        ]
    );
    // A rerun finds the folders the first run created and hands them out the same way.
    for folder in &folders {
        std::fs::create_dir(folder).expect("folder");
    }
    assert_eq!(extraction_subfolders(directory, &bases), folders);
}

#[test]
fn a_file_in_the_way_and_a_taken_name_are_stepped_around_together() {
    let package = tempfile::tempdir().expect("tempdir");
    let directory = package.path();
    std::fs::write(directory.join("Film (1)"), b"not a folder").expect("file");
    assert_eq!(
        extraction_subfolders(directory, &["Film", "Film"]),
        vec![directory.join("Film"), directory.join("Film (2)")]
    );
}

/// Audit 2026-10-08, CORE-04: a package without a name gets an English folder, as every other
/// name the service makes up does.
#[test]
fn a_package_without_a_name_gets_the_package_folder() {
    let base = Path::new("/library");
    assert_eq!(package_directory(base, "  "), base.join("package"));
    assert_eq!(package_directory(base, "download"), base.join("download"));
}

/// Audit 2026-10-08, CORE-03: a dangling symlink is in the way. `exists()` followed it and
/// called the name free, and the download would have been written through it.
#[cfg(unix)]
#[test]
fn a_dangling_symlink_is_stepped_around() {
    let package = tempfile::tempdir().expect("tempdir");
    let target = package.path().join("elsewhere.bin");
    std::os::unix::fs::symlink(&target, package.path().join("video.mp4")).expect("link");
    assert_eq!(
        super::collision_free_path(package.path(), "video.mp4"),
        package.path().join("video (1).mp4")
    );
}
