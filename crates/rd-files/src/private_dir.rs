//! Directories only the service's own account may enter: the data directory, and below it the
//! database copies and the unencrypted staging of the backup before an update (security review
//! 2026-09-30, findings 2 and 8).
//!
//! The data directory holds the database, the secrets, the local control token and the update's
//! journal; whoever may write it stops the service, plants a journal the next start acts on, or
//! reads every credential. On Unix a directory created here is `0700` from its creation. On
//! Windows a directory inherits its folder's access list, and a portable installation unpacked
//! to `C:\<folder>` inherits "Authenticated Users: Modify" from the drive's root, so the start
//! gives the data directory an access list of its own ([`protect_private_dir`]): the account
//! the service runs as, `SYSTEM` and the Administrators, each with full control, inheritance
//! from above cut. Without `unsafe` code, through the system's own `icacls` and a PowerShell
//! `Get-Acl` that reads the list back in SDDL — account names are translated per system
//! language, SIDs are not.
//!
//! [`private_dir_exposure`] is the question `rdownloader doctor` asks on every platform.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Well-known accounts that stand for other people, as SDDL names them (alias or SID) and as a
/// report names them.
const BROAD_ACCOUNTS: [(&str, &str, &str); 11] = [
    ("WD", "S-1-1-0", "Everyone"),
    ("AU", "S-1-5-11", "Authenticated Users"),
    ("BU", "S-1-5-32-545", "Users"),
    ("IU", "S-1-5-4", "Interactive"),
    ("AN", "S-1-5-7", "Anonymous"),
    ("NU", "S-1-5-2", "Network"),
    ("BG", "S-1-5-32-546", "Guests"),
    ("PU", "S-1-5-32-547", "Power Users"),
    ("RD", "S-1-5-32-555", "Remote Desktop Users"),
    ("DU", "", "Domain Users"),
    ("DG", "", "Domain Guests"),
];

/// Creates `path` and its missing parents; on Unix each one it creates is `0700` from the start
/// (not after a `chmod`, so no other account ever sees it open). One that exists is left as it
/// is — [`private_dir_exposure`] reports it.
///
/// # Errors
///
/// When a directory cannot be created.
pub fn create_private_dir_all(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path)
    }
}

/// Makes an existing directory the owner's alone on Unix (`0700`). Elsewhere nothing: a folder
/// below the data directory inherits [`protect_private_dir`]'s access list.
///
/// # Errors
///
/// When the mode cannot be set.
pub fn restrict_to_owner(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// On Windows: gives `path` an access list of its own — the running account, `SYSTEM` and the
/// Administrators with full control, inherited by everything below, nothing inherited from
/// above — when [`private_dir_exposure`] finds another account in it; left alone otherwise.
/// Elsewhere nothing: [`create_private_dir_all`] created it private.
///
/// # Errors
///
/// When the access list cannot be read or set, or another account is still in it afterwards.
/// The caller warns and starts anyway; `doctor` reports the directory.
pub fn protect_private_dir(path: &Path) -> io::Result<()> {
    if !cfg!(windows) || private_dir_exposure(path)?.is_none() {
        return Ok(());
    }
    let user = format!("*{}", current_user_sid()?);
    let full = |account: &str| format!("{account}:(OI)(CI)F");
    run(Command::new(system_program(r"icacls.exe"))
        .arg(path)
        .args(["/inheritance:r", "/grant:r"])
        .args([full(&user), full("*S-1-5-18"), full("*S-1-5-32-544")]))?;
    // Explicit entries survive `/inheritance:r`; those of the broad groups go as well.
    run(Command::new(system_program(r"icacls.exe"))
        .arg(path)
        .arg("/remove:g")
        .args(
            BROAD_ACCOUNTS
                .iter()
                .filter(|(_, sid, _)| !sid.is_empty())
                .map(|(_, sid, _)| format!("*{sid}")),
        ))?;
    match private_dir_exposure(path)? {
        None => Ok(()),
        Some(why) => Err(io::Error::other(why)),
    }
}

/// Why `path` is not the service account's alone, or `None` when it is: on Unix group or other
/// permission bits, on Windows an allowing entry for another account in its access list.
///
/// # Errors
///
/// When the permissions cannot be read.
pub fn private_dir_exposure(path: &Path) -> io::Result<Option<String>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(path)?.permissions().mode();
        Ok(unix_exposure(mode))
    }
    #[cfg(not(unix))]
    {
        if !cfg!(windows) {
            return Ok(None);
        }
        let user = current_user_sid()?;
        let sddl = access_list_sddl(path)?;
        Ok(windows_exposure(&sddl, &user))
    }
}

/// The verdict on a Unix mode: any bit for group or others is access for other accounts.
#[must_use]
pub fn unix_exposure(mode: u32) -> Option<String> {
    (mode & 0o077 != 0).then(|| {
        format!(
            "other accounts have access (mode {:o}); `chmod 700` it",
            mode & 0o777
        )
    })
}

/// The verdict on a Windows access list in SDDL: every allowing entry of its DACL for an account
/// other than `user`, `SYSTEM`, the Administrators and the owner placeholders. Inherit-only
/// entries count — they are what every file created below gets.
#[must_use]
pub fn windows_exposure(sddl: &str, user: &str) -> Option<String> {
    let Some(dacl) = sddl.split_once("D:").map(|(_, rest)| rest) else {
        return Some("the access list could not be read".to_owned());
    };
    // The SACL, when present, follows the DACL's entries.
    let dacl = dacl.split("S:").next().unwrap_or(dacl);
    if dacl.starts_with("NO_ACCESS_CONTROL") {
        return Some("the folder has no access list: everyone has full control".to_owned());
    }
    let mut others: Vec<String> = Vec::new();
    for entry in dacl.split('(').skip(1) {
        let entry = entry.split(')').next().unwrap_or_default();
        let fields: Vec<&str> = entry.split(';').collect();
        let (Some(kind), Some(account)) = (fields.first(), fields.get(5)) else {
            continue;
        };
        if !matches!(*kind, "A" | "OA" | "XA" | "ZA") {
            continue;
        }
        let account = account.trim();
        let trusted = account.eq_ignore_ascii_case(user)
            || [
                "SY",
                "S-1-5-18",
                "BA",
                "S-1-5-32-544",
                "CO",
                "S-1-3-0",
                "OW",
                "S-1-3-4",
            ]
            .iter()
            .any(|own| account.eq_ignore_ascii_case(own));
        if trusted {
            continue;
        }
        let name = BROAD_ACCOUNTS
            .iter()
            .find(|(alias, sid, _)| {
                account.eq_ignore_ascii_case(alias)
                    || (!sid.is_empty() && account.eq_ignore_ascii_case(sid))
            })
            .map_or_else(
                || "another account".to_owned(),
                |(_, _, name)| (*name).to_owned(),
            );
        if !others.contains(&name) {
            others.push(name);
        }
    }
    (!others.is_empty()).then(|| {
        format!(
            "the access list lets other accounts in: {}",
            others.join(", ")
        )
    })
}

/// The SID in the output of `whoami /user /fo csv /nh`: `"machine\name","S-1-5-21-..."`.
#[must_use]
pub fn sid_from_whoami(output: &str) -> Option<String> {
    let sid = output.trim().rsplit(',').next()?.trim().trim_matches('"');
    (sid.starts_with("S-1-")
        && sid
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'-' || byte == b'S'))
    .then(|| sid.to_owned())
}

fn current_user_sid() -> io::Result<String> {
    let output = Command::new(system_program("whoami.exe"))
        .args(["/user", "/fo", "csv", "/nh"])
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("whoami could not name the account"));
    }
    sid_from_whoami(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| io::Error::other("whoami named no SID"))
}

/// The access list of `path` in SDDL. The path goes through the environment, so no quoting of
/// it can end up as PowerShell code.
#[cfg(not(unix))]
fn access_list_sddl(path: &Path) -> io::Result<String> {
    let output = Command::new(system_program(r"WindowsPowerShell\v1.0\powershell.exe"))
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$ErrorActionPreference = 'Stop'; (Get-Acl -LiteralPath $env:RD_PRIVATE_DIR).Sddl",
        ])
        .env("RD_PRIVATE_DIR", path)
        .stdin(Stdio::null())
        .output()?;
    let sddl = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || sddl.is_empty() {
        return Err(io::Error::other(format!(
            "the access list of {} could not be read",
            path.display()
        )));
    }
    Ok(sddl)
}

fn run(command: &mut Command) -> io::Result<()> {
    let status = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("icacls ended with {status}")))
    }
}

/// A program of the Windows directory's `System32`, by its full path: never looked up by name,
/// which starts in the folder of the running executable.
fn system_program(relative: &str) -> PathBuf {
    windows_directory(std::env::var_os("SystemRoot"))
        .join("System32")
        .join(relative)
}

fn windows_directory(root: Option<OsString>) -> PathBuf {
    root.filter(|root| !root.is_empty())
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str = "S-1-5-21-1111111111-2222222222-3333333333-1001";

    #[test]
    fn group_or_other_bits_are_access_for_other_accounts() {
        assert_eq!(unix_exposure(0o40700), None);
        assert_eq!(unix_exposure(0o700), None);
        for open in [0o755, 0o750, 0o705, 0o770, 0o777, 0o710] {
            let why = unix_exposure(open).expect("exposed");
            assert!(why.contains(&format!("{open:o}")), "{why}");
        }
    }

    /// What a portable folder under `C:\` inherits, and what `protect_private_dir` leaves.
    #[test]
    fn an_inherited_authenticated_users_entry_is_found_and_a_protected_list_is_clean() {
        let inherited = format!(
            "O:{USER}G:{USER}D:AI(A;OICIID;FA;;;BA)(A;OICIID;FA;;;SY)\
             (A;OICIIOID;SDGXGWGR;;;AU)(A;ID;0x1301bf;;;AU)(A;OICIID;0x1200a9;;;BU)"
        );
        let why = windows_exposure(&inherited, USER).expect("exposed");
        assert!(why.contains("Authenticated Users"), "{why}");
        assert!(why.contains("Users"), "{why}");

        let protected =
            format!("O:{USER}G:{USER}D:PAI(A;OICI;FA;;;{USER})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
        assert_eq!(windows_exposure(&protected, USER), None);
    }

    #[test]
    fn another_account_everyone_and_a_missing_dacl_are_exposure() {
        let other = format!("D:P(A;OICI;FA;;;{USER})(A;;FR;;;S-1-5-21-1-2-3-1002)");
        assert!(
            windows_exposure(&other, USER)
                .expect("exposed")
                .contains("another account")
        );
        let everyone = format!("D:P(A;OICI;FA;;;{USER})(A;;0x1200a9;;;S-1-1-0)");
        assert!(
            windows_exposure(&everyone, USER)
                .expect("exposed")
                .contains("Everyone")
        );
        // A deny entry keeps nobody out that is not already let in.
        let denied = format!("D:P(A;OICI;FA;;;{USER})(D;;FA;;;WD)");
        assert_eq!(windows_exposure(&denied, USER), None);
        assert!(windows_exposure(&format!("O:{USER}G:{USER}"), USER).is_some());
        assert!(windows_exposure("D:NO_ACCESS_CONTROL", USER).is_some());
        // An empty protected list lets nobody in.
        assert_eq!(windows_exposure("O:BAG:DUD:P", USER), None);
    }

    #[test]
    fn the_sid_is_read_from_whoami_whatever_the_account_is_called() {
        assert_eq!(
            sid_from_whoami("\"desktop-7\\j\u{fc}rgen m\u{fc}ller\",\"S-1-5-21-1-2-3-1001\"\r\n")
                .as_deref(),
            Some("S-1-5-21-1-2-3-1001")
        );
        assert_eq!(sid_from_whoami("Access is denied."), None);
        assert_eq!(sid_from_whoami(""), None);
    }

    #[test]
    fn system_programs_are_named_by_their_full_path() {
        let root = windows_directory(Some(r"D:\Win".into()));
        assert!(root.join("System32").starts_with(r"D:\Win"));
        for unknown in [None, Some(OsString::new())] {
            assert_eq!(windows_directory(unknown), PathBuf::from(r"C:\Windows"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_created_directory_is_private_from_the_start_and_an_existing_one_is_left() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().expect("tempdir");
        let data = root.path().join("new").join("data");
        create_private_dir_all(&data).expect("create");
        for created in [&data, &root.path().join("new")] {
            let mode = std::fs::metadata(created)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700, "{}", created.display());
        }
        assert_eq!(private_dir_exposure(&data).expect("read"), None);

        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).expect("create");
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        create_private_dir_all(&shared).expect("exists");
        assert!(private_dir_exposure(&shared).expect("read").is_some());
        restrict_to_owner(&shared).expect("restrict");
        assert_eq!(private_dir_exposure(&shared).expect("read"), None);
        protect_private_dir(&shared).expect("nothing to do off Windows");
    }
}
