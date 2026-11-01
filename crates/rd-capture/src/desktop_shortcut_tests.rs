use std::path::{Path, PathBuf};

use super::{ARGUMENTS, DESCRIPTION, Place, link};

/// The shortcut starts `open`, after which Windows puts the dropped files, under the names
/// Windows shows.
#[test]
fn each_shortcut_has_its_name_and_starts_open() {
    let started = <crate::cli::Cli as clap::Parser>::try_parse_from([
        "rdownloader-capture",
        ARGUMENTS,
        "dropped.nzb",
    ])
    .expect("the shortcut's arguments parse");
    assert!(
        matches!(started.command, Some(crate::cli::Command::Open(_))),
        "the shortcut starts `open`"
    );
    assert_eq!(Place::Desktop.file_name(), "Add to rDownloader.lnk");
    assert_eq!(Place::SendTo.file_name(), "rDownloader.lnk");
}

/// The agent, in its own folder, with its own icon, minimised; a path with a quote or a `$` is
/// data, not code, now that no script is written.
#[test]
fn the_link_points_at_the_agent_in_its_folder() {
    let executable = Path::new("/Users/O'Brien/$Tools/rdownloader-capture.exe");
    let link = link(executable);
    assert_eq!(link.target, executable);
    assert_eq!(link.arguments, ARGUMENTS);
    assert_eq!(
        link.working_directory,
        PathBuf::from("/Users/O'Brien/$Tools")
    );
    assert_eq!(link.icon, executable);
    assert_eq!(link.description, DESCRIPTION);
    assert!(link.minimized);
}

/// Elsewhere the commands say where they exist instead of pretending to work.
#[cfg(not(windows))]
#[test]
fn the_shortcuts_exist_on_windows_only() {
    let error = super::install(Place::SendTo, Path::new("/opt/rd/capture")).expect_err("refused");
    assert!(error.to_string().contains("Windows only"), "{error}");
    assert!(super::remove(Place::Desktop).is_err());
}

/// Removing takes the one file it names, and none that is not there.
#[test]
fn removing_takes_only_the_shortcut_and_tolerates_none() {
    let folder = std::env::temp_dir().join(format!("rd-shortcut-rm-{}", std::process::id()));
    std::fs::create_dir_all(&folder).expect("folder");
    let link = folder.join(Place::Desktop.file_name());
    let other = folder.join("keep.txt");
    std::fs::write(&link, b"lnk").expect("link");
    std::fs::write(&other, b"keep").expect("other");
    for _ in 0..2 {
        super::remove_in(&folder, Place::Desktop).expect("removed");
    }
    assert!(!link.exists());
    assert!(other.exists());
    std::fs::remove_dir_all(&folder).expect("clean up");
}

/// The real thing, in a folder of the test's own (Windows CI): written through the shell link
/// object, read back by Windows' own `WScript.Shell` (the test's PowerShell, not the agent's),
/// pointing at the agent with `open`, and gone without a trace after `remove` -- also twice.
#[cfg(windows)]
#[test]
fn a_shortcut_is_written_and_removed_without_a_trace() {
    let folder = std::env::temp_dir().join(format!("rd-shortcut-{}", std::process::id()));
    std::fs::create_dir_all(&folder).expect("folder");
    let executable = std::env::current_exe().expect("this test's executable");
    super::write_in(&folder, Place::SendTo, &executable).expect("written");
    let link = folder.join(Place::SendTo.file_name());
    assert!(link.exists(), "{}", link.display());

    let read = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$l = (New-Object -ComObject WScript.Shell).CreateShortcut($env:RD_LINK); \
             Write-Output $l.TargetPath; Write-Output $l.Arguments; Write-Output $l.WindowStyle",
        ])
        .env("RD_LINK", &link)
        .output()
        .expect("read back");
    let text = String::from_utf8_lossy(&read.stdout);
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some(executable.to_string_lossy().as_ref()));
    assert_eq!(lines.next(), Some("open"));
    assert_eq!(lines.next(), Some("7"), "minimised");

    for _ in 0..2 {
        super::remove_in(&folder, Place::SendTo).expect("removed");
    }
    assert!(!link.exists());
    assert_eq!(std::fs::read_dir(&folder).expect("folder").count(), 0);
    std::fs::remove_dir(&folder).expect("clean up");
}
