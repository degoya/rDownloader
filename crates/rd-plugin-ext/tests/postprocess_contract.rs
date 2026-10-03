//! The post-processing step contract, exercised against the two bundled checksum plugins.
//!
//! These run the real components over a real directory, because the promises worth checking
//! are exactly the ones a mock would let through: that a plugin reads only the files it was
//! offered, that a package with nothing to do is skipped rather than failed, that a wrong
//! checksum is a permanent failure, and that stopping leaves something to resume from.

use rd_plugin_host::{
    PluginManifest,
    artifact::component,
    extension::{PostprocessPlugin, SourceState, StepOutcome},
};

const SHA256: &str = include_str!("../../../plugins/sha256-postprocess/manifest.toml");
const MD5: &str = include_str!("../../../plugins/md5-postprocess/manifest.toml");

fn manifest(source: &str) -> PluginManifest {
    toml::from_str(source).expect("bundled manifest")
}

/// SHA-256 of `payload`, as `sha256sum` would print it.
const PAYLOAD: &[u8] = b"rdownloader\n";
const PAYLOAD_SHA256: &str = "0a2e9e1e6f70b6a7b1a5c8f2e2b1a7ec5f7d1fef67e8b1d94a5c0d5e6b4a2f31";

fn package(files: &[(&str, &[u8])]) -> (tempfile::TempDir, SourceState) {
    let directory = tempfile::tempdir().expect("tempdir");
    for (name, bytes) in files {
        let path = directory.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("folder");
        }
        std::fs::write(path, bytes).expect("write");
    }
    let names = files.iter().map(|(name, _)| (*name).to_owned()).collect();
    let state = SourceState::new(
        "package-1".to_owned(),
        directory.path().to_path_buf(),
        names,
    );
    (directory, state)
}

#[tokio::test]
async fn a_package_without_a_sidecar_is_skipped_rather_than_failed() {
    let bytes = component("rd-plugin-sha256-postprocess");
    let plugin = PostprocessPlugin::new(manifest(SHA256), &bytes, None).expect("compile");
    // Most packages have no checksum file. A step that failed those would fail every package
    // in a category it was switched on for, which is the opposite of useful.
    let (_directory, source) = package(&[("release.bin", PAYLOAD)]);
    assert_eq!(
        plugin.run(source, Vec::new(), None).await.expect("run"),
        StepOutcome::Skipped
    );
}

#[tokio::test]
async fn a_matching_checksum_completes() {
    let bytes = component("rd-plugin-sha256-postprocess");
    let plugin = PostprocessPlugin::new(manifest(SHA256), &bytes, None).expect("compile");
    let digest = sha256_hex(PAYLOAD);
    let sidecar = format!("{digest}  release.bin\n");
    let (_directory, source) = package(&[
        ("release.bin", PAYLOAD),
        ("release.sha256", sidecar.as_bytes()),
    ]);
    assert_eq!(
        plugin.run(source, Vec::new(), None).await.expect("run"),
        passed()
    );
}

#[tokio::test]
async fn a_wrong_checksum_fails_permanently_and_names_the_file() {
    let bytes = component("rd-plugin-sha256-postprocess");
    let plugin = PostprocessPlugin::new(manifest(SHA256), &bytes, None).expect("compile");
    let sidecar = format!("{PAYLOAD_SHA256}  release.bin\n");
    let (_directory, source) = package(&[
        ("release.bin", PAYLOAD),
        ("release.sha256", sidecar.as_bytes()),
    ]);
    match plugin.run(source, Vec::new(), None).await.expect("run") {
        StepOutcome::Failed { message } => {
            assert!(message.contains("release.bin"), "{message}");
        }
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[tokio::test]
async fn a_cancelled_step_stops_with_something_to_resume_from() {
    let bytes = component("rd-plugin-sha256-postprocess");
    let plugin = PostprocessPlugin::new(manifest(SHA256), &bytes, None).expect("compile");
    let digest = sha256_hex(PAYLOAD);
    let sidecar = format!("{digest}  release.bin\n");
    let (_directory, source) = package(&[
        ("release.bin", PAYLOAD),
        ("release.sha256", sidecar.as_bytes()),
    ]);
    // Cancelled before it starts, so it stops at the first entry rather than finishing.
    source
        .cancellation()
        .store(true, std::sync::atomic::Ordering::SeqCst);
    match plugin.run(source, Vec::new(), None).await.expect("run") {
        StepOutcome::Stopped { checkpoint } => assert_eq!(checkpoint, 0_u32.to_le_bytes()),
        other => panic!("expected a stop, got {other:?}"),
    }
}

#[tokio::test]
async fn md5_reads_its_own_sidecar_and_ignores_the_other_one() {
    let bytes = component("rd-plugin-md5-postprocess");
    let plugin = PostprocessPlugin::new(manifest(MD5), &bytes, None).expect("compile");
    // Both formats present. Each plugin verifies its own, which is the point of them being
    // two plugins: switching one off leaves the other doing its job.
    let sha = format!("{}  release.bin\n", sha256_hex(PAYLOAD));
    let md5 = format!("{}  release.bin\n", md5_hex(PAYLOAD));
    let (_directory, source) = package(&[
        ("release.bin", PAYLOAD),
        ("release.sha256", sha.as_bytes()),
        ("release.md5", md5.as_bytes()),
    ]);
    assert_eq!(
        plugin.run(source, Vec::new(), None).await.expect("run"),
        passed()
    );
}

#[tokio::test]
async fn a_sidecar_in_a_subfolder_checks_the_files_beside_it() {
    // An archive unpacked into a folder of its own brings its sidecar along (RD-170-16). The
    // host lists `Film/film.mkv`; the sidecar says `film.mkv`, relative to its own folder.
    for (source, name, sidecar) in [
        (SHA256, "rd-plugin-sha256-postprocess", "Film/film.sha256"),
        (MD5, "rd-plugin-md5-postprocess", "Film/film.md5"),
    ] {
        let bytes = component(name);
        let plugin = PostprocessPlugin::new(manifest(source), &bytes, None).expect("compile");
        let digest = if source == MD5 {
            md5_hex(PAYLOAD)
        } else {
            sha256_hex(PAYLOAD)
        };
        let text = format!("{digest}  film.mkv\n");
        let (_directory, state) =
            package(&[("Film/film.mkv", PAYLOAD), (sidecar, text.as_bytes())]);
        assert_eq!(
            plugin.run(state, Vec::new(), None).await.expect("run"),
            passed(),
            "{name}"
        );
    }
}

#[tokio::test]
async fn a_wrong_checksum_in_a_subfolder_fails_like_one_at_the_top() {
    let bytes = component("rd-plugin-sha256-postprocess");
    let plugin = PostprocessPlugin::new(manifest(SHA256), &bytes, None).expect("compile");
    let sidecar = format!("{PAYLOAD_SHA256}  film.mkv\n");
    let (_directory, source) = package(&[
        ("Film/film.mkv", PAYLOAD),
        ("Film/film.sha256", sidecar.as_bytes()),
    ]);
    match plugin.run(source, Vec::new(), None).await.expect("run") {
        StepOutcome::Failed { message } => {
            assert!(message.contains("Film/film.mkv"), "{message}");
        }
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[tokio::test]
async fn a_listed_file_the_package_lacks_is_a_warning_when_others_matched() {
    // A release split across two packages: the part that is here verifies, the missing one is
    // named in a warning the host shows on the step (RD-190-06; before, only in the log).
    for (source, name, slug, ext) in [
        (
            SHA256,
            "rd-plugin-sha256-postprocess",
            "sha256_postprocess",
            "sha256",
        ),
        (MD5, "rd-plugin-md5-postprocess", "md5_postprocess", "md5"),
    ] {
        let bytes = component(name);
        let plugin = PostprocessPlugin::new(manifest(source), &bytes, None).expect("compile");
        let digest = if source == MD5 {
            md5_hex(PAYLOAD)
        } else {
            sha256_hex(PAYLOAD)
        };
        let sidecar = format!("{digest}  release.bin\n{digest}  other.bin\n");
        let sidecar_name = format!("release.{ext}");
        let (_directory, state) = package(&[
            ("release.bin", PAYLOAD),
            (sidecar_name.as_str(), sidecar.as_bytes()),
        ]);
        match plugin.run(state, Vec::new(), None).await.expect("run") {
            StepOutcome::Complete {
                checkpoint: None,
                warnings,
            } => {
                assert_eq!(warnings.len(), 1, "{name}: {warnings:?}");
                let warning = &warnings[0];
                assert_eq!(warning.code, format!("{slug}.unchecked"));
                assert!(
                    warning
                        .params
                        .contains(&("count".to_owned(), "1".to_owned())),
                    "{warning:?}"
                );
                assert!(warning.message.contains("other.bin"), "{warning:?}");
            }
            other => panic!("{name}: expected a pass with a warning, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_sidecar_none_of_whose_files_is_there_fails_rather_than_passing() {
    // Before RD-190-06 a sidecar whose files all went unfound reported nothing wrong. With not
    // one file checked, "fine" is not an answer.
    let bytes = component("rd-plugin-sha256-postprocess");
    let plugin = PostprocessPlugin::new(manifest(SHA256), &bytes, None).expect("compile");
    let sidecar = format!("{}  other.bin\n", sha256_hex(PAYLOAD));
    let (_directory, source) = package(&[
        ("release.bin", PAYLOAD),
        ("release.sha256", sidecar.as_bytes()),
    ]);
    match plugin.run(source, Vec::new(), None).await.expect("run") {
        StepOutcome::Failed { message } => {
            assert!(message.contains("other.bin"), "{message}");
        }
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[tokio::test]
async fn volumes_the_unpack_already_removed_are_skipped_not_failed() {
    // At `+Delete` the archive volumes are gone once the unpack succeeded, and this step runs
    // only after that. The host names them as removed (RD-190-06); a sidecar over the volumes
    // has nothing left to check.
    let bytes = component("rd-plugin-md5-postprocess");
    let plugin = PostprocessPlugin::new(manifest(MD5), &bytes, None).expect("compile");
    let sidecar = format!(
        "{0}  release.part1.rar\n{0}  release.part2.rar\n",
        md5_hex(PAYLOAD)
    );
    let package_files = [
        ("release/film.mkv", PAYLOAD),
        ("release.md5", sidecar.as_bytes()),
    ];
    let removed = vec![
        "release.part1.rar".to_owned(),
        "release.part2.rar".to_owned(),
    ];
    let (_directory, source) = package(&package_files);
    assert_eq!(
        plugin.run(source, removed, None).await.expect("run"),
        StepOutcome::Skipped
    );
    // Without the host's word the plugin no longer guesses from the extension: volumes the
    // package lacks for no known reason leave nothing verified, which fails.
    let (_directory, source) = package(&package_files);
    assert!(matches!(
        plugin.run(source, Vec::new(), None).await.expect("run"),
        StepOutcome::Failed { .. }
    ));
}

#[tokio::test]
async fn neither_plugin_asks_for_a_capability() {
    // Reading the package is what a post-processing step *is*, not something it requests, so
    // both manifests are empty. A step that could also reach the network would be a different
    // kind of thing wearing this type's name.
    for source in [SHA256, MD5] {
        let manifest = manifest(source);
        assert!(manifest.capabilities.net_http.is_none());
        assert!(manifest.capabilities.net_stream.is_none());
        assert!(!manifest.capabilities.cookies);
        assert!(!manifest.capabilities.captcha);
        assert!(manifest.capabilities.secrets.is_empty());
    }
}

/// A plain pass: no checkpoint to keep, nothing to warn about.
fn passed() -> StepOutcome {
    StepOutcome::Complete {
        checkpoint: None,
        warnings: Vec::new(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex(&hasher.finalize())
}

fn md5_hex(bytes: &[u8]) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(bytes);
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
