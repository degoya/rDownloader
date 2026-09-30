//! Every installation kind from a path fixture, and what each one is told to do.

use std::path::{Path, PathBuf};

use super::*;

/// Creates `relative` under `root` as an empty executable and returns its path.
fn executable(root: &Path, relative: &[&str]) -> PathBuf {
    let path = relative
        .iter()
        .fold(root.to_owned(), |path, part| path.join(part));
    std::fs::create_dir_all(path.parent().expect("parent")).expect("layout");
    std::fs::write(&path, b"").expect("executable");
    path
}

fn marker(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("layout");
    std::fs::write(path, content).expect("marker");
}

#[test]
fn the_environment_names_the_kind_before_anything_else() {
    let root = tempfile::tempdir().expect("tempdir");
    let exe = executable(
        root.path(),
        &["scoop", "apps", "rdownloader", "1.8.0", "rdownloader.exe"],
    );
    assert_eq!(
        detect_from(Some(&exe), Some("docker"), false),
        InstallKind::Docker
    );
    assert_eq!(detect_from(Some(&exe), Some("MSI"), true), InstallKind::Msi);
    // A value that names nothing is ignored rather than trusted.
    assert_eq!(
        detect_from(Some(&exe), Some("nonsense"), false),
        InstallKind::Scoop
    );
}

#[test]
fn a_container_is_docker() {
    let root = tempfile::tempdir().expect("tempdir");
    let exe = executable(root.path(), &["usr", "local", "bin", "rdownloader"]);
    assert_eq!(detect_from(Some(&exe), None, true), InstallKind::Docker);
}

/// Scoop and Homebrew install the portable archive, marker and all: the layout wins.
#[test]
fn a_package_manager_layout_wins_over_the_archives_marker() {
    let root = tempfile::tempdir().expect("tempdir");
    let scoop = executable(
        root.path(),
        &[
            "Users",
            "me",
            "scoop",
            "apps",
            "rdownloader",
            "1.8.0",
            "rdownloader.exe",
        ],
    );
    marker(&scoop.with_file_name(INSTALL_KIND_FILE), "portable\n");
    assert_eq!(detect_from(Some(&scoop), None, false), InstallKind::Scoop);

    let brew = executable(
        root.path(),
        &[
            "opt",
            "homebrew",
            "Cellar",
            "rdownloader",
            "1.8.0",
            "libexec",
            "rdownloader",
        ],
    );
    marker(&brew.with_file_name(INSTALL_KIND_FILE), "portable\n");
    assert_eq!(detect_from(Some(&brew), None, false), InstallKind::Homebrew);

    let linuxbrew = executable(
        root.path(),
        &[
            "home",
            "linuxbrew",
            ".linuxbrew",
            "Cellar",
            "rdownloader",
            "1.8.0",
            "bin",
            "rdownloader",
        ],
    );
    assert_eq!(
        detect_from(Some(&linuxbrew), None, false),
        InstallKind::Homebrew
    );

    let winget = executable(
        root.path(),
        &[
            "Users",
            "me",
            "AppData",
            "Local",
            "Microsoft",
            "WinGet",
            "Packages",
            "degoya.rDownloader_Microsoft.Winget.Source_8wekyb3d8bbwe",
            "rdownloader.exe",
        ],
    );
    assert_eq!(detect_from(Some(&winget), None, false), InstallKind::Winget);
}

/// A folder of one's own called `apps` is not Scoop.
#[test]
fn an_apps_folder_outside_scoop_is_read_by_its_marker() {
    let root = tempfile::tempdir().expect("tempdir");
    let exe = executable(
        root.path(),
        &["Tools", "apps", "rdownloader", "current", "rdownloader.exe"],
    );
    marker(&exe.with_file_name(INSTALL_KIND_FILE), "portable\n");
    assert_eq!(detect_from(Some(&exe), None, false), InstallKind::Portable);
}

#[test]
fn the_marker_beside_the_executable_names_the_installer() {
    let root = tempfile::tempdir().expect("tempdir");
    for (content, expected) in [
        ("portable\n", InstallKind::Portable),
        ("msi", InstallKind::Msi),
        ("# written by the MSI\n\n  msi  \n", InstallKind::Msi),
        ("Winget\n", InstallKind::Winget),
    ] {
        let exe = executable(
            root.path(),
            &["Program Files", "rDownloader", "rdownloader.exe"],
        );
        marker(&exe.with_file_name(INSTALL_KIND_FILE), content);
        assert_eq!(
            detect_from(Some(&exe), None, false),
            expected,
            "{content:?}"
        );
    }
}

/// A distribution package keeps `/usr/bin` clean: its marker is `/usr/lib/rdownloader/`.
#[test]
fn a_distribution_package_is_named_by_its_marker_under_lib() {
    for (content, expected) in [
        ("deb\n", InstallKind::Deb),
        ("rpm\n", InstallKind::Rpm),
        ("aur\n", InstallKind::Aur),
    ] {
        let root = tempfile::tempdir().expect("tempdir");
        let exe = executable(root.path(), &["usr", "bin", "rdownloader"]);
        marker(
            &root
                .path()
                .join("usr")
                .join("lib")
                .join("rdownloader")
                .join(INSTALL_KIND_FILE),
            content,
        );
        assert_eq!(
            detect_from(Some(&exe), None, false),
            expected,
            "{content:?}"
        );
    }
}

#[test]
fn without_a_marker_or_with_a_wrong_one_the_kind_is_unknown() {
    let root = tempfile::tempdir().expect("tempdir");
    let exe = executable(
        root.path(),
        &["src", "rDownloader", "target", "release", "rdownloader"],
    );
    assert_eq!(detect_from(Some(&exe), None, false), InstallKind::Unknown);
    marker(&exe.with_file_name(INSTALL_KIND_FILE), "flatpak\n");
    assert_eq!(detect_from(Some(&exe), None, false), InstallKind::Unknown);
    assert_eq!(detect_from(None, None, false), InstallKind::Unknown);
}

/// The archive carries `VERSION.txt` and no marker; a build from source carries neither.
#[test]
fn an_archive_without_a_marker_is_portable_by_its_version_file() {
    let root = tempfile::tempdir().expect("tempdir");
    let exe = executable(root.path(), &["Tools", "rDownloader", "rdownloader.exe"]);
    marker(&exe.with_file_name("VERSION.txt"), "version=1.8.0\n");
    assert_eq!(detect_from(Some(&exe), None, false), InstallKind::Portable);
}

/// The installers' layout (RD-180-05): the marker beside the real executable, which
/// `current_exe` reaches through the `/usr/bin` symlink.
#[test]
fn an_installer_marker_beside_the_real_executable_wins_over_the_version_file() {
    let root = tempfile::tempdir().expect("tempdir");
    let exe = executable(root.path(), &["usr", "lib", "rdownloader", "rdownloader"]);
    marker(&exe.with_file_name("VERSION.txt"), "version=1.8.0\n");
    marker(&exe.with_file_name(INSTALL_KIND_FILE), "deb\n");
    assert_eq!(detect_from(Some(&exe), None, false), InstallKind::Deb);
}

#[test]
fn every_kind_round_trips_through_its_name() {
    for kind in [
        InstallKind::Portable,
        InstallKind::Msi,
        InstallKind::Deb,
        InstallKind::Rpm,
        InstallKind::Homebrew,
        InstallKind::Scoop,
        InstallKind::Winget,
        InstallKind::Aur,
        InstallKind::Docker,
    ] {
        assert_eq!(InstallKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(InstallKind::parse("unknown"), None);
}

#[test]
fn each_kind_gets_its_own_action() {
    let command = |kind: InstallKind, version: &str| match kind.action(version) {
        UpdateAction::Command { command, .. } => command,
        UpdateAction::Download => "download".to_owned(),
        UpdateAction::Install => "install".to_owned(),
    };
    for kind in [InstallKind::Portable, InstallKind::Msi] {
        assert_eq!(kind.action("1.8.0"), UpdateAction::Install, "{kind:?}");
        assert!(kind.installs_itself(), "{kind:?}");
    }
    assert_eq!(InstallKind::Unknown.action("1.8.0"), UpdateAction::Download);
    for kind in [
        InstallKind::Deb,
        InstallKind::Rpm,
        InstallKind::Homebrew,
        InstallKind::Scoop,
        InstallKind::Winget,
        InstallKind::Aur,
        InstallKind::Docker,
        InstallKind::Unknown,
    ] {
        assert!(!kind.installs_itself(), "{kind:?}");
    }
    assert_eq!(
        command(InstallKind::Deb, "1.8.0"),
        "sudo apt update && sudo apt install --only-upgrade rdownloader"
    );
    assert_eq!(
        command(InstallKind::Rpm, "1.8.0"),
        "sudo dnf upgrade --refresh rdownloader"
    );
    assert_eq!(
        command(InstallKind::Homebrew, "1.8.0"),
        "brew upgrade rdownloader"
    );
    assert_eq!(
        command(InstallKind::Scoop, "1.8.0"),
        "scoop update rdownloader"
    );
    assert_eq!(
        command(InstallKind::Winget, "1.8.0"),
        "winget upgrade degoya.rDownloader"
    );
    assert_eq!(command(InstallKind::Aur, "1.8.0"), "yay -S rdownloader-bin");
    assert_eq!(
        command(InstallKind::Docker, "1.8.0"),
        "docker pull ghcr.io/degoya/rdownloader:latest"
    );
    assert_eq!(
        command(InstallKind::Docker, "1.8.0-beta.2"),
        "docker pull ghcr.io/degoya/rdownloader:v1.8.0-beta.2"
    );
}

#[test]
fn only_the_channels_that_publish_betas_are_offered_them() {
    assert!(InstallKind::Portable.receives_betas());
    assert!(InstallKind::Msi.receives_betas());
    assert!(InstallKind::Docker.receives_betas());
    assert!(!InstallKind::Homebrew.receives_betas());
    assert!(!InstallKind::Scoop.receives_betas());
    assert!(!InstallKind::Winget.receives_betas());
    assert!(!InstallKind::Aur.receives_betas());
}

#[test]
fn the_windows_verbatim_prefix_is_dropped() {
    assert_eq!(
        strip_verbatim(Path::new(
            r"\\?\C:\scoop\apps\rdownloader\1.8.0\rdownloader.exe"
        )),
        PathBuf::from(r"C:\scoop\apps\rdownloader\1.8.0\rdownloader.exe")
    );
}
