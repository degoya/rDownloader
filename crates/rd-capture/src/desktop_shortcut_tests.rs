use std::path::Path;

use super::{ARGUMENTS, CREATE_SCRIPT, Place, REMOVE_SCRIPT, environment};

/// The shortcut starts `open`, after which Windows puts the dropped files, and lies where
/// Windows looks for it.
#[test]
fn each_shortcut_lies_in_its_folder_and_starts_open() {
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
    assert_eq!(Place::Desktop.folder(), "DesktopDirectory");
    assert_eq!(Place::SendTo.folder(), "SendTo");
    assert_eq!(Place::Desktop.file_name(), "Add to rDownloader.lnk");
    assert_eq!(Place::SendTo.file_name(), "rDownloader.lnk");
}

/// No value is spliced into a script: a path with a quote or a `$` reaches PowerShell as data.
#[test]
fn every_value_reaches_the_scripts_through_the_environment() {
    let executable = Path::new(r"C:\Users\O'Brien\$Tools\rdownloader-capture.exe");
    let environment = environment(Place::Desktop, "DesktopDirectory", executable);
    for (name, _) in &environment {
        assert!(
            CREATE_SCRIPT.contains(&format!("$env:{name}")),
            "{name} is set but never read"
        );
    }
    for name in ["RD_SHORTCUT_FOLDER", "RD_SHORTCUT_NAME"] {
        assert!(REMOVE_SCRIPT.contains(&format!("$env:{name}")), "{name}");
    }
    assert!(
        environment
            .iter()
            .any(|(name, value)| *name == "RD_SHORTCUT_TARGET"
                && value == r"C:\Users\O'Brien\$Tools\rdownloader-capture.exe")
    );
    for script in [CREATE_SCRIPT, REMOVE_SCRIPT] {
        assert!(script.starts_with("$ErrorActionPreference = 'Stop'"));
    }
    // Removing only ever takes the one file it names, and none that is not there.
    assert!(REMOVE_SCRIPT.contains("Test-Path -LiteralPath $path"));
    assert!(REMOVE_SCRIPT.contains("Remove-Item -LiteralPath $path"));
}

/// Elsewhere the commands say where they exist instead of pretending to work.
#[cfg(not(windows))]
#[test]
fn the_shortcuts_exist_on_windows_only() {
    let error = super::install(Place::SendTo, Path::new("/opt/rd/capture")).expect_err("refused");
    assert!(error.to_string().contains("Windows only"), "{error}");
    assert!(super::remove(Place::Desktop).is_err());
}

/// The real thing, in a folder of the test's own (Windows CI): written, pointing at the agent with
/// `open`, and gone without a trace after `remove` -- also when removed twice.
#[cfg(windows)]
#[test]
fn a_shortcut_is_written_and_removed_without_a_trace() {
    let folder = std::env::temp_dir().join(format!("rd-shortcut-{}", std::process::id()));
    std::fs::create_dir_all(&folder).expect("folder");
    let folder_text = folder.to_string_lossy().into_owned();
    let executable = std::env::current_exe().expect("this test's executable");
    super::run(
        CREATE_SCRIPT,
        "write",
        Place::SendTo,
        &folder_text,
        &executable,
    )
    .expect("written");
    let link = folder.join(Place::SendTo.file_name());
    assert!(link.exists(), "{}", link.display());

    let read = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$l = (New-Object -ComObject WScript.Shell).CreateShortcut($env:RD_LINK); \
             Write-Output $l.TargetPath; Write-Output $l.Arguments",
        ])
        .env("RD_LINK", &link)
        .output()
        .expect("read back");
    let text = String::from_utf8_lossy(&read.stdout);
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some(executable.to_string_lossy().as_ref()));
    assert_eq!(lines.next(), Some("open"));

    for _ in 0..2 {
        super::run(
            REMOVE_SCRIPT,
            "remove",
            Place::SendTo,
            &folder_text,
            Path::new(""),
        )
        .expect("removed");
    }
    assert!(!link.exists());
    assert_eq!(std::fs::read_dir(&folder).expect("folder").count(), 0);
    std::fs::remove_dir(&folder).expect("clean up");
}
