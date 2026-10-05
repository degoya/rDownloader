use super::SourceState;

fn state(directory: &std::path::Path) -> SourceState {
    SourceState::new(
        "pkg-1".to_owned(),
        directory.to_path_buf(),
        vec!["one.bin".to_owned(), "sub/two.bin".to_owned()],
    )
}

#[test]
fn only_the_offered_files_of_the_given_package_resolve() {
    let directory = tempfile::tempdir().expect("tempdir");
    let source = state(directory.path());
    assert!(source.resolve("pkg-1", "one.bin").is_ok());
    assert!(source.resolve("pkg-1", "sub/two.bin").is_ok());
    // A file that exists but was not offered, a package that was not given, and a
    // traversal are all the same refusal: the plugin does not name files.
    assert!(source.resolve("pkg-1", "secret.txt").is_err());
    assert!(source.resolve("pkg-2", "one.bin").is_err());
    assert!(source.resolve("pkg-1", "../../etc/passwd").is_err());
}

#[test]
fn a_read_returns_only_what_is_there() {
    let directory = tempfile::tempdir().expect("tempdir");
    std::fs::write(directory.path().join("one.bin"), b"0123456789").expect("write");
    let source = state(directory.path());
    let path = source.resolve("pkg-1", "one.bin").expect("resolve");
    let mut file = std::fs::File::open(path).expect("open");
    assert_eq!(super::read_slice(&mut file, 0, 4).expect("read"), b"0123");
    assert_eq!(super::read_slice(&mut file, 8, 100).expect("read"), b"89");
    assert!(
        super::read_slice(&mut file, 100, 4)
            .expect("read")
            .is_empty()
    );
    // The same handle serves a slice before the last one: every read seeks.
    assert_eq!(super::read_slice(&mut file, 2, 3).expect("read"), b"234");
}

/// PLUG-01: the reads pay for a checksum's work, up to the package's passes and no
/// further, so a plugin that reads in a loop still runs out of fuel.
#[tokio::test]
async fn reads_credit_fuel_up_to_the_packages_passes_and_no_further() {
    let directory = tempfile::tempdir().expect("tempdir");
    std::fs::write(directory.path().join("one.bin"), [0_u8; 10]).expect("write");
    std::fs::create_dir(directory.path().join("sub")).expect("folder");
    std::fs::write(directory.path().join("sub").join("two.bin"), [0_u8; 6]).expect("write");
    let sandbox = crate::SandboxEngine::new(crate::PluginLimits::default()).expect("sandbox");
    let mut store = sandbox
        .create_source_store(
            Vec::new(),
            None,
            rd_plugin_api::ClientIdentity {
                account_id: None,
                proxy_profile_id: None,
                tls_revision: 0,
            },
            None,
            false,
            state(directory.path()),
        )
        .expect("store");

    let bytes = super::read_at(store.data_mut(), "pkg-1", "one.bin", 0, 10)
        .await
        .expect("read");
    assert_eq!(bytes.len(), 10);
    let source = store.data_mut().source.as_mut().expect("source");
    // Two offered files: three passes.
    let ceiling = 16 * super::FUEL_PER_READ_BYTE * super::credited_passes(2);
    assert_eq!(
        source.read_credit,
        Some(ceiling),
        "the first read measures both offered files"
    );
    let mut credited = 0;
    for _ in 0..10 {
        credited += source.take_read_credit(10);
    }
    assert_eq!(
        credited, ceiling,
        "a loop of reads earns the ceiling and nothing past it"
    );
    assert_eq!(source.take_read_credit(10), 0);
}

/// Renaming is the one thing on this interface that writes, so the guards matter more here
/// than anywhere else: only offered files, only plain names, never over an existing file.
#[tokio::test]
async fn rename_stays_inside_the_package_and_keeps_the_offered_list_in_step() {
    let directory = tempfile::tempdir().expect("tempdir");
    std::fs::write(directory.path().join("Big Buck Bunny.mkv"), b"x").expect("write");
    std::fs::write(directory.path().join("taken.mkv"), b"y").expect("write");
    let mut state = super::SourceState::new(
        "handle".to_owned(),
        directory.path().to_path_buf(),
        vec!["Big Buck Bunny.mkv".to_owned(), "taken.mkv".to_owned()],
    );

    state
        .rename("handle", "Big Buck Bunny.mkv", "Big.Buck.Bunny.mkv")
        .await
        .expect("a plain new name is allowed");
    assert!(directory.path().join("Big.Buck.Bunny.mkv").exists());
    assert!(
        state.files.iter().any(|file| file == "Big.Buck.Bunny.mkv"),
        "a later read addresses the file by its new name"
    );

    assert!(
        state
            .rename("handle", "nothing.mkv", "x.mkv")
            .await
            .is_err(),
        "a file that was never offered cannot be renamed"
    );
    assert!(
        state
            .rename("other", "Big.Buck.Bunny.mkv", "x.mkv")
            .await
            .is_err(),
        "another invocation's handle is refused"
    );
    for name in ["../escape.mkv", "sub/escape.mkv", "..", "", "."] {
        assert!(
            state
                .rename("handle", "Big.Buck.Bunny.mkv", name)
                .await
                .is_err(),
            "{name:?} must not be accepted as a new name"
        );
    }
    assert!(
        state
            .rename("handle", "Big.Buck.Bunny.mkv", "taken.mkv")
            .await
            .is_err(),
        "an existing file is never overwritten"
    );
    assert_eq!(
        std::fs::read(directory.path().join("taken.mkv")).expect("read"),
        b"y",
        "and it still holds its own content"
    );
}

/// Audit 2026-10-05, S22: a new name is one the host would give a file itself. A Windows
/// device name, a colon (an alternate data stream on NTFS) or a trailing dot is refused, and
/// the file keeps its name.
#[tokio::test]
async fn rename_refuses_a_name_the_host_would_not_give_a_file() {
    let directory = tempfile::tempdir().expect("tempdir");
    std::fs::write(directory.path().join("film.mkv"), b"x").expect("write");
    let mut state = super::SourceState::new(
        "handle".to_owned(),
        directory.path().to_path_buf(),
        vec!["film.mkv".to_owned()],
    );
    for name in [
        "CON.mkv",
        "conin$",
        "COM1.mkv",
        "film.mkv:stream",
        "film<1>.mkv",
        "film.",
        "film.mkv ",
        "line\nbreak.mkv",
    ] {
        assert!(
            state.rename("handle", "film.mkv", name).await.is_err(),
            "{name:?} must not be accepted as a new name"
        );
    }
    assert!(directory.path().join("film.mkv").exists());
    assert_eq!(state.files, vec!["film.mkv".to_owned()]);

    state
        .rename("handle", "film.mkv", "Film (2024).mkv")
        .await
        .expect("a name the sanitiser leaves alone is allowed");
    assert!(directory.path().join("Film (2024).mkv").exists());
}

/// A file in a folder of the package is renamed where it is, not moved to the top.
#[tokio::test]
async fn a_nested_file_is_renamed_inside_its_own_folder() {
    let directory = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(directory.path().join("Film")).expect("folder");
    std::fs::write(
        directory.path().join("Film").join("Big Buck Bunny.mkv"),
        b"x",
    )
    .expect("write");
    let mut state = super::SourceState::new(
        "handle".to_owned(),
        directory.path().to_path_buf(),
        vec!["Film/Big Buck Bunny.mkv".to_owned()],
    );

    state
        .rename("handle", "Film/Big Buck Bunny.mkv", "Big.Buck.Bunny.mkv")
        .await
        .expect("renamed");

    assert!(
        directory
            .path()
            .join("Film")
            .join("Big.Buck.Bunny.mkv")
            .is_file()
    );
    assert!(!directory.path().join("Big.Buck.Bunny.mkv").exists());
    assert_eq!(state.files, ["Film/Big.Buck.Bunny.mkv".to_owned()]);
    let path = state
        .resolve("handle", "Film/Big.Buck.Bunny.mkv")
        .expect("a later read finds it under its new name");
    assert!(path.is_file());
}

/// RA-HOST-05: one pass per offered file plus the pass over the sidecars, so three
/// sidecars over one large file are paid for; never fewer than two passes, never more than
/// eight.
#[test]
fn every_offered_file_may_be_one_more_pass_up_to_a_ceiling() {
    assert_eq!(super::credited_passes(0), super::MIN_CREDITED_PASSES);
    assert_eq!(super::credited_passes(1), 2);
    assert_eq!(super::credited_passes(2), 3);
    // `film.mkv` with `film.md5`, `release.md5` and `film.sfv`: three passes over the film
    // and one over the sidecars.
    assert_eq!(super::credited_passes(4), 5);
    assert_eq!(super::credited_passes(1000), super::MAX_CREDITED_PASSES);
    assert_eq!(
        super::credited_passes(usize::MAX),
        super::MAX_CREDITED_PASSES
    );
}
