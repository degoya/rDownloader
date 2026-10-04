//! The post-processing step contract, exercised against the bundled rename step
//! (RD-191-07, PLUG-23).
//!
//! `rename-postprocess` had unit tests of its naming rules and nothing that ran the component.
//! What only the component can show: that it renames through the host's `source.rename` and
//! nowhere else, that a file in a folder keeps its folder, that a package with nothing to tidy
//! is skipped, that a name already taken leaves the file alone without failing the package, and
//! that stopping leaves a count to resume from.

use std::path::Path;

use rd_plugin_host::{
    PluginManifest,
    artifact::component,
    extension::{PostprocessPlugin, SourceState, StepOutcome},
};

const RENAME: &str = include_str!("../../../plugins/rename-postprocess/manifest.toml");

fn plugin() -> PostprocessPlugin {
    let manifest: PluginManifest = toml::from_str(RENAME).expect("bundled manifest");
    let bytes = component("rd-plugin-rename-postprocess");
    PostprocessPlugin::new(manifest, &bytes, None).expect("compile")
}

fn package(files: &[&str]) -> (tempfile::TempDir, SourceState) {
    let directory = tempfile::tempdir().expect("tempdir");
    for name in files {
        let path = directory.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("folder");
        }
        std::fs::write(&path, name.as_bytes()).expect("write");
    }
    let names = files.iter().map(|name| (*name).to_owned()).collect();
    let state = SourceState::new(
        "package-1".to_owned(),
        directory.path().to_path_buf(),
        names,
    );
    (directory, state)
}

fn exists(root: &Path, name: &str) -> bool {
    root.join(name).is_file()
}

#[tokio::test]
async fn untidy_names_are_renamed_in_place_and_the_extension_is_kept() {
    let (directory, source) = package(&["Big Buck Bunny.MKV", "Season 1/Ep  01.mkv"]);
    match plugin().run(source, Vec::new(), None).await.expect("run") {
        StepOutcome::Complete { warnings, .. } => assert!(warnings.is_empty(), "{warnings:?}"),
        other => panic!("expected the step to complete, got {other:?}"),
    }
    let root = directory.path();
    assert!(exists(root, "Big.Buck.Bunny.MKV"));
    assert!(!exists(root, "Big Buck Bunny.MKV"));
    // A file in a folder of the package keeps its folder; only its own name is tidied, and the
    // folder's name is not the step's to change.
    assert!(exists(root, "Season 1/Ep.01.mkv"));
    assert!(!exists(root, "Season 1/Ep  01.mkv"));
    // The contents travel with the name: the step renames, it never rewrites.
    assert_eq!(
        std::fs::read(root.join("Big.Buck.Bunny.MKV")).expect("read"),
        b"Big Buck Bunny.MKV"
    );
}

#[tokio::test]
async fn a_package_that_is_already_tidy_is_skipped() {
    // Most packages need nothing; reporting a success for them would be noise in every
    // package's history.
    let (directory, source) = package(&["Big.Buck.Bunny.mkv", "readme.txt"]);
    assert_eq!(
        plugin().run(source, Vec::new(), None).await.expect("run"),
        StepOutcome::Skipped
    );
    assert!(exists(directory.path(), "Big.Buck.Bunny.mkv"));
    assert!(exists(directory.path(), "readme.txt"));
}

#[tokio::test]
async fn a_name_already_taken_leaves_the_file_as_it_was_without_failing() {
    // The tidy name of the first file is the second file's name. The host refuses the rename,
    // and the step carries on rather than failing a package whose files are all still there.
    let (directory, source) = package(&["Big Buck Bunny.mkv", "Big.Buck.Bunny.mkv"]);
    assert_eq!(
        plugin().run(source, Vec::new(), None).await.expect("run"),
        StepOutcome::Skipped
    );
    let root = directory.path();
    assert!(exists(root, "Big Buck Bunny.mkv"));
    assert_eq!(
        std::fs::read(root.join("Big.Buck.Bunny.mkv")).expect("read"),
        b"Big.Buck.Bunny.mkv",
        "the file already carrying the name is not overwritten"
    );
}

#[tokio::test]
async fn a_cancelled_step_stops_with_the_count_to_resume_from() {
    let (directory, source) = package(&["Big Buck Bunny.mkv"]);
    source
        .cancellation()
        .store(true, std::sync::atomic::Ordering::SeqCst);
    match plugin().run(source, Vec::new(), None).await.expect("run") {
        // Nothing renamed yet, and the name the step stopped at (RD-191-06, PLUG-05).
        StepOutcome::Stopped { checkpoint } => assert_eq!(
            checkpoint,
            [0_u32.to_le_bytes().as_slice(), b"Big Buck Bunny.mkv"].concat()
        ),
        other => panic!("expected a stop, got {other:?}"),
    }
    assert!(exists(directory.path(), "Big Buck Bunny.mkv"));
}

#[tokio::test]
async fn a_resumed_step_starts_after_the_files_it_already_did() {
    // The checkpoint names the file to resume at (RD-191-06, PLUG-05): the files that sort
    // before it were looked at already, so the first one is left alone although it is untidy.
    let (directory, source) = package(&["First File.mkv", "Second File.mkv"]);
    let checkpoint = [0_u32.to_le_bytes().as_slice(), b"Second File.mkv"].concat();
    let outcome = plugin()
        .run(source, Vec::new(), Some(checkpoint))
        .await
        .expect("run");
    assert!(
        matches!(outcome, StepOutcome::Complete { .. }),
        "{outcome:?}"
    );
    let root = directory.path();
    assert!(exists(root, "First File.mkv"));
    assert!(exists(root, "Second.File.mkv"));
}

#[test]
fn the_step_asks_for_no_capability() {
    // Renaming inside the package is what a step is handed a source for; nothing else is
    // needed, so nothing else is granted.
    let manifest: PluginManifest = toml::from_str(RENAME).expect("bundled manifest");
    assert!(manifest.capabilities.net_http.is_none());
    assert!(manifest.capabilities.net_stream.is_none());
    assert!(!manifest.capabilities.cookies);
    assert!(!manifest.capabilities.captcha);
}
