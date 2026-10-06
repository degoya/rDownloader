//! RD-1120-18 (RD-120-59): an archive password with a `"` or a trailing `\`, measured through a
//! real 7-Zip on the argument path the service uses (`rar_arguments`, `apply_to`).
//!
//! `seven_zip_args_tests.rs` checks the quoting against a port of 7-Zip's parser; this one runs
//! the program. The archive is a split ZIP (`secret.zip.001`, `.002`), AES-256 with the password,
//! because a split ZIP goes to the external 7-Zip like a RAR set does and the `zip` crate can
//! write one here: no binary in the tree, no RAR encoder on the build machine. Every password is
//! tried after a wrong one, so the run also shows that the password decides.
//!
//! Outside Windows the arguments travel as a vector and every password has to extract. Under
//! Windows the command line is re-parsed by 7-Zip's own parser, which has no way to receive a
//! `"`: that password is refused before the tool starts (`PasswordHasQuote`, security review
//! 2026-09-28), and the trailing backslash has to arrive whole.
//!
//! It needs 7-Zip 25.00 or later, the service's floor (`rd_tools::compat::ARCHIVE_TOOL_FLOORS`):
//! `RD_TEST_7Z` names it, otherwise `7z` on `PATH`. Without one the test says why and passes.

use std::{
    io::{Cursor, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use zip::write::SimpleFileOptions;

use crate::{
    ArchiveLimits, ExternalRarTool, ExtractRequest, ExtractionError, RarToolKind,
    extract_with_passwords, group_archive_sets,
};

const MEMBER: &str = "secret.txt";
const CONTENT: &[u8] = b"measured\n";
const WRONG: &str = "not-the-password";

/// The passwords the measurement hands over, with what the log calls them.
const PASSWORDS: [(&str, &str); 3] = [
    ("plain", "plain-pass-1120"),
    ("with a quote", r#"pa"ss"#),
    ("ending in a backslash", r"pass\"),
];

/// `major.minor` of a version token such as `25.01`.
fn version_of(token: &str) -> Option<(u32, u32)> {
    let (major, minor) = token.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// The 7-Zip to measure and the version its banner states, or `None` with the reason printed.
fn seven_zip() -> Option<(PathBuf, String)> {
    let explicit = std::env::var("RD_TEST_7Z").ok();
    let Some(tool) = rd_core::locate_tool(explicit.as_deref(), None, "7z") else {
        eprintln!("skipped: no 7z (set RD_TEST_7Z)");
        return None;
    };
    let banner = match std::process::Command::new(&tool.path).arg("i").output() {
        Ok(output) => String::from_utf8_lossy(&output.stdout).into_owned(),
        Err(error) => {
            eprintln!("skipped: {} did not start: {error}", tool.path.display());
            return None;
        }
    };
    let Some(version) = banner
        .lines()
        .filter(|line| line.contains("7-Zip"))
        .flat_map(str::split_whitespace)
        .find(|token| version_of(token).is_some())
    else {
        eprintln!(
            "skipped: {} states no version in its banner",
            tool.path.display()
        );
        return None;
    };
    if version_of(version).is_some_and(|found| found < (25, 0)) {
        eprintln!(
            "skipped: {} is 7-Zip {version}, below the 25.00 floor (set RD_TEST_7Z)",
            tool.path.display()
        );
        return None;
    }
    Some((tool.path, version.to_owned()))
}

/// Writes `secret.zip.001` and `.002` into `folder`: one ZIP holding [`MEMBER`], AES-256 with
/// `password`, cut in two.
fn split_zip(folder: &Path, password: &str) -> Vec<PathBuf> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(
        MEMBER,
        SimpleFileOptions::default().with_aes_encryption(zip::AesMode::Aes256, password),
    )
    .expect("encrypted member");
    zip.write_all(CONTENT).expect("member content");
    let bytes = zip.finish().expect("finish ZIP").into_inner();
    let (first, second) = bytes.split_at(bytes.len() / 2);
    let volumes = vec![folder.join("secret.zip.001"), folder.join("secret.zip.002")];
    std::fs::write(&volumes[0], first).expect("first volume");
    std::fs::write(&volumes[1], second).expect("second volume");
    volumes
}

#[tokio::test]
async fn a_real_seven_zip_gets_a_password_with_a_quote_or_a_trailing_backslash() {
    let Some((executable, version)) = seven_zip() else {
        return;
    };
    let platform = std::env::consts::OS;
    for (label, password) in PASSWORDS {
        let temp = tempfile::tempdir().expect("temporary directory");
        let package = temp.path().join("measured package");
        std::fs::create_dir_all(&package).expect("package");
        let sets = group_archive_sets(&split_zip(&package, password));
        assert_eq!(sets.len(), 1, "the two volumes are one set");
        assert!(
            sets[0].is_multipart(),
            "a split ZIP goes to the external tool"
        );
        let destination = package.join("out dir");
        let request = ExtractRequest {
            set: &sets[0],
            destination: destination.clone(),
            limits: ArchiveLimits::default(),
            rar_tool: Some(ExternalRarTool {
                executable: executable.clone(),
                kind: RarToolKind::SevenZip,
                timeout: Duration::from_secs(60),
            }),
            merge: false,
            progress: None,
        };
        let refused_on_windows = cfg!(windows) && password.contains('"');
        // Alone, so the refusal is the answer and not a wrong password tried before it.
        let candidates = if refused_on_windows {
            vec![Some(password.to_owned())]
        } else {
            vec![Some(WRONG.to_owned()), Some(password.to_owned())]
        };
        let result = extract_with_passwords(request, &candidates).await;
        if refused_on_windows {
            assert!(
                matches!(result, Err(ExtractionError::PasswordHasQuote)),
                "{label} on {platform} with 7-Zip {version}: {result:?}"
            );
            eprintln!(
                "measured: {label}: refused before 7-Zip {version} started ({platform}, {})",
                executable.display()
            );
            continue;
        }
        let (report, used) = result.unwrap_or_else(|error| {
            panic!("{label} on {platform} with 7-Zip {version} did not extract: {error}")
        });
        assert_eq!(
            used.as_deref(),
            Some(password),
            "{label}: the wrong one won"
        );
        assert_eq!(report.files, 1, "{label}");
        assert_eq!(
            std::fs::read(destination.join(MEMBER)).expect("extracted member"),
            CONTENT,
            "{label}"
        );
        eprintln!(
            "measured: {label}: extracted by 7-Zip {version} ({platform}, {})",
            executable.display()
        );
    }
}
