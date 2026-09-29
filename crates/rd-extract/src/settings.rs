use std::{path::PathBuf, time::Duration};

use anyhow::{Result, bail};
use rd_core::PostprocessSettings;
use rd_db::Database;
use rd_postprocess::{ArchiveLimits, ExternalRarTool, RarToolKind};

/// Reads the shared postprocessing settings from `service.settings`.
///
/// Refuses a malformed blob rather than running on defaults: silently defaulted archive
/// limits and tool paths decide what gets unpacked and with which binary.
pub async fn load_postprocess_settings(database: &Database) -> Result<PostprocessSettings> {
    database.service_settings().await
}

pub(crate) fn archive_limits(settings: &PostprocessSettings) -> ArchiveLimits {
    ArchiveLimits {
        max_files: u64::from(settings.archive_max_files),
        max_uncompressed_bytes: settings.archive_max_uncompressed_bytes.get(),
    }
}

/// The ffmpeg binary, looked up the same way every other external tool is (RD-080-09).
///
/// The managed store first, then the vendor folders, then `PATH`, so a portable install that
/// dropped ffmpeg next to the executable is found without anybody typing a path. The lease
/// comes back with it: hold it for as long as ffmpeg runs, so a managed version cannot be
/// removed mid-remux (RD-102-02).
pub(crate) fn ffmpeg_tool(
    settings: &PostprocessSettings,
) -> Option<(PathBuf, Option<rd_core::ToolLease>)> {
    rd_core::locate_tool_leased(None, settings.vendor_directory.as_deref(), "ffmpeg")
        .map(|(tool, lease)| (tool.path, lease))
}

/// What the settings resolved to, plus the reason there is no tool when there is none.
#[derive(Clone, Debug, Default)]
pub(crate) struct RarToolChoice {
    pub tool: Option<ExternalRarTool>,
    /// `rar_executable` names a binary of a different kind than `rar_tool` claims.
    ///
    /// The two settings used to be taken on trust, so unrar syntax could be sent to 7-Zip. The
    /// tool then rejects the command line and, before RD-107-11, that was reported as a wrong
    /// password. Nothing is run at all now, and the contradiction is what the step says.
    pub conflict: Option<String>,
    /// The tool that was found is below its security floor, so it is not used to extract.
    pub outdated: Option<OutdatedTool>,
}

/// An archive tool refused for its version (security review 2026-09-28, finding 5).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutdatedTool {
    /// `unrar` or `7z`.
    pub tool: &'static str,
    /// The version the tool reported.
    pub found: String,
    /// The oldest version extraction runs.
    pub minimum: String,
}

/// Takes the tool out of `choice` when its version is below the floor in
/// `rd_tools::compat::ARCHIVE_TOOL_FLOORS`.
///
/// unrar before 6.12 and 7-Zip before 25.00 write through an archive's links before
/// `validate_tree` can look at the result, so the only brake is not to start them. A version
/// that cannot be read is not refused: `unknown` is not `old` (the rule of `rd_tools::compat`),
/// a renamed or wrapped binary would otherwise stop every RAR unpack, and the person sees the
/// unreadable version in the tool status and in `doctor`. It is logged as a warning here.
pub(crate) async fn refuse_outdated(choice: RarToolChoice) -> RarToolChoice {
    let Some(tool) = choice.tool.as_ref() else {
        return choice;
    };
    let name = kind_name(tool.kind);
    let assessment = rd_tools::compat::assess_archive_tool(name, &tool.executable).await;
    match assessment.verdict {
        rd_tools::Verdict::TooOld | rd_tools::Verdict::KnownBad => {
            let outdated = OutdatedTool {
                tool: name,
                found: assessment.version.unwrap_or_else(|| "unknown".to_owned()),
                minimum: assessment.min_version.unwrap_or_default(),
            };
            tracing::warn!(
                tool = %tool.executable.display(),
                found = %outdated.found,
                minimum = %outdated.minimum,
                "archive tool below its security floor; extraction refused"
            );
            RarToolChoice {
                tool: None,
                conflict: choice.conflict,
                outdated: Some(outdated),
            }
        }
        rd_tools::Verdict::Unknown => {
            tracing::warn!(
                tool = %tool.executable.display(),
                version = assessment.version.as_deref().unwrap_or("unreadable"),
                "archive tool version could not be read; its security floor is not checked"
            );
            choice
        }
        rd_tools::Verdict::Supported => choice,
    }
}

/// The kind a binary's name gives away, or `None` for a name that says nothing.
///
/// Only the names the two projects actually ship are recognised; a renamed or wrapped binary
/// keeps the benefit of the doubt and the configured kind.
///
/// Both separators are split by hand: a Windows path configured on a Linux host (or read from a
/// settings backup) must still be recognised, and `Path` only knows the host's separator.
fn kind_from_name(executable: &str) -> Option<RarToolKind> {
    let name = executable.rsplit(['/', '\\']).next()?;
    let stem = name
        .rsplit_once('.')
        .map_or(name, |(stem, _)| stem)
        .to_ascii_lowercase();
    match stem.as_str() {
        "unrar" | "rar" => Some(RarToolKind::Unrar),
        "7z" | "7za" | "7zr" | "7zz" | "7zzs" | "p7zip" => Some(RarToolKind::SevenZip),
        _ => None,
    }
}

const fn kind_name(kind: RarToolKind) -> &'static str {
    match kind {
        RarToolKind::Unrar => "unrar",
        RarToolKind::SevenZip => "7z",
    }
}

/// The configured RAR tool, or the first one found in the vendor folders / on `PATH`.
///
/// Without a fallback, RAR extraction stayed silently unavailable until someone typed an
/// absolute path into the settings, even with unrar installed.
pub(crate) fn rar_tool(settings: &PostprocessSettings, timeout: Duration) -> Result<RarToolChoice> {
    let preferred = match settings.rar_tool.as_str() {
        "unrar" => RarToolKind::Unrar,
        "7z" => RarToolKind::SevenZip,
        value => bail!("unsupported configured RAR tool {value:?}"),
    };
    if let Some(executable) = settings.rar_executable.as_ref() {
        if let Some(observed) = kind_from_name(executable)
            && observed != preferred
        {
            return Ok(RarToolChoice {
                tool: None,
                conflict: Some(format!(
                    "rar_tool={} but rar_executable looks like {}",
                    kind_name(preferred),
                    kind_name(observed)
                )),
                outdated: None,
            });
        }
        return Ok(RarToolChoice {
            tool: Some(ExternalRarTool {
                executable: PathBuf::from(executable),
                kind: preferred,
                timeout,
            }),
            conflict: None,
            outdated: None,
        });
    }
    let vendor = settings.vendor_directory.as_deref();
    let candidates = match preferred {
        RarToolKind::Unrar => [(RarToolKind::Unrar, "unrar"), (RarToolKind::SevenZip, "7z")],
        RarToolKind::SevenZip => [(RarToolKind::SevenZip, "7z"), (RarToolKind::Unrar, "unrar")],
    };
    Ok(RarToolChoice {
        tool: candidates.into_iter().find_map(|(kind, name)| {
            let tool = rd_core::locate_tool(None, vendor, name)?;
            Some(ExternalRarTool {
                executable: tool.path,
                kind,
                timeout,
            })
        }),
        conflict: None,
        outdated: None,
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rd_core::PostprocessSettings;
    use rd_postprocess::RarToolKind;

    use super::{kind_from_name, rar_tool};

    #[test]
    fn a_binary_name_identifies_the_tool_or_says_nothing() {
        assert_eq!(kind_from_name("/usr/bin/unrar"), Some(RarToolKind::Unrar));
        assert_eq!(
            kind_from_name("C:\\tools\\UnRAR.exe"),
            Some(RarToolKind::Unrar)
        );
        assert_eq!(kind_from_name("7zz"), Some(RarToolKind::SevenZip));
        assert_eq!(
            kind_from_name("/opt/vendor/7z.exe"),
            Some(RarToolKind::SevenZip)
        );
        assert_eq!(kind_from_name("/opt/vendor/my-unpacker"), None);
    }

    #[test]
    fn a_contradiction_between_the_two_settings_yields_no_tool_and_a_reason() {
        // RD-107-11: unrar syntax sent to 7-Zip used to surface as a wrong password.
        let settings = PostprocessSettings {
            rar_tool: "unrar".to_owned(),
            rar_executable: Some("/usr/bin/7z".to_owned()),
            ..PostprocessSettings::default()
        };
        let choice = rar_tool(&settings, Duration::from_secs(1)).expect("resolve");
        assert!(choice.tool.is_none());
        let conflict = choice.conflict.expect("conflict");
        assert!(conflict.contains("rar_tool=unrar"), "{conflict}");
        assert!(conflict.contains("7z"), "{conflict}");
    }

    #[test]
    fn a_matching_pair_and_an_unrecognised_name_are_both_accepted() {
        let mut settings = PostprocessSettings {
            rar_tool: "unrar".to_owned(),
            rar_executable: Some("/usr/bin/unrar".to_owned()),
            ..PostprocessSettings::default()
        };
        let matching = rar_tool(&settings, Duration::from_secs(1)).expect("resolve");
        assert_eq!(matching.tool.expect("tool").kind, RarToolKind::Unrar);
        settings.rar_executable = Some("/opt/wrapper".to_owned());
        let wrapped = rar_tool(&settings, Duration::from_secs(1)).expect("resolve");
        assert!(wrapped.conflict.is_none());
        assert_eq!(wrapped.tool.expect("tool").kind, RarToolKind::Unrar);
    }

    /// Security review 2026-09-28, finding 5, against a real process: a tool below the floor
    /// is taken out with its version, one at or above it stays, and one whose version cannot
    /// be read stays too (unknown is not old).
    #[cfg(unix)]
    #[tokio::test]
    async fn an_archive_tool_below_its_security_floor_is_not_used() {
        use super::refuse_outdated;
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("tempdir");
        let fake = |name: &str, banner: &str| {
            let path = directory.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\necho '{banner}'\nexit 7\n"))
                .expect("fake tool");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            path
        };
        let settings = |executable: &std::path::Path, kind: &str| PostprocessSettings {
            rar_tool: kind.to_owned(),
            rar_executable: Some(executable.to_string_lossy().into_owned()),
            ..PostprocessSettings::default()
        };
        let choose = |settings: PostprocessSettings| async move {
            refuse_outdated(rar_tool(&settings, Duration::from_secs(1)).expect("resolve")).await
        };

        let old = fake("unrar", "UNRAR 6.11 freeware      Copyright (c) 1993-2022");
        let refused = choose(settings(&old, "unrar")).await;
        assert!(refused.tool.is_none(), "unrar 6.11 must not extract");
        let outdated = refused.outdated.expect("the refusal names the version");
        assert_eq!(
            (
                outdated.tool,
                outdated.found.as_str(),
                outdated.minimum.as_str()
            ),
            ("unrar", "6.11", "6.12")
        );

        let p7zip = fake(
            "7z",
            "7-Zip [64] 16.02 : Copyright (c) 1999-2016 Igor Pavlov",
        );
        let refused = choose(settings(&p7zip, "7z")).await;
        assert!(refused.tool.is_none(), "p7zip 16.02 must not extract");
        assert_eq!(refused.outdated.expect("outdated").minimum, "25.00");

        let current = fake("7zz", "7-Zip (z) 25.01 (x64) : Copyright (c) 1999-2025");
        let kept = choose(settings(&current, "7z")).await;
        assert!(kept.tool.is_some() && kept.outdated.is_none());

        let silent = fake("my-unpacker", "");
        let unknown = choose(settings(&silent, "unrar")).await;
        assert!(unknown.tool.is_some() && unknown.outdated.is_none());
    }
}
