//! The account's SID and a folder's access list on a real folder (RD-1420-05): what
//! `rd_files::protect_private_dir` relies on — a protected list replaces the inherited one, what
//! is created below inherits it, and an SDDL without an access list changes nothing.

use std::path::{Path, PathBuf};

use crate::{current_user_sid, dacl_sddl, set_protected_dacl};

/// A fresh folder under the temporary directory, removed when dropped.
struct Folder(PathBuf);

impl Folder {
    fn new(test: &str) -> Self {
        let path = std::env::temp_dir().join(format!("rd-win32-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).expect("create the folder");
        Self(path)
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The entries of an SDDL access list as their six fields: type, flags, rights, object type,
/// inherited object type, trustee.
fn entries(sddl: &str) -> Vec<Vec<String>> {
    let (_, list) = sddl.split_once("D:").expect("an access list");
    list.split('(')
        .skip(1)
        .map(|entry| {
            entry
                .trim_end_matches(')')
                .split(';')
                .map(str::to_owned)
                .collect()
        })
        .collect()
}

fn protect(path: &Path) -> String {
    let user = current_user_sid().expect("the current user");
    set_protected_dacl(path, &format!("D:P(A;OICI;FA;;;{user})(A;OICI;FA;;;SY)"))
        .expect("set the access list");
    dacl_sddl(path).expect("read the access list back")
}

#[test]
fn the_current_user_is_a_stable_sid() {
    let sid = current_user_sid().expect("the current user");
    let parts = sid.strip_prefix("S-1-5-").expect("an NT authority SID");
    assert!(
        parts.split('-').all(|part| part.parse::<u64>().is_ok()),
        "numbers only: {sid}"
    );
    assert_eq!(current_user_sid().expect("again"), sid);
}

#[test]
fn a_new_folder_carries_an_inherited_list() {
    let folder = Folder::new("inherited");
    let sddl = dacl_sddl(&folder.0).expect("read the access list");
    assert!(
        sddl.starts_with("D:") && !sddl.starts_with("D:P"),
        "not protected: {sddl}"
    );
    assert!(!entries(&sddl).is_empty(), "entries: {sddl}");
}

#[test]
fn a_protected_list_replaces_the_inherited_one() {
    let folder = Folder::new("protected");
    let sddl = protect(&folder.0);
    assert!(sddl.starts_with("D:P"), "protected from above: {sddl}");
    let entries = entries(&sddl);
    assert_eq!(
        entries.len(),
        2,
        "the two entries set, nothing inherited: {sddl}"
    );
    assert!(entries.iter().all(|entry| entry[1] == "OICI"), "{sddl}");
    assert!(entries.iter().any(|entry| entry[5] == "SY"), "{sddl}");
}

#[test]
fn what_is_created_below_inherits_the_protected_list() {
    let folder = Folder::new("below");
    protect(&folder.0);
    let file = folder.0.join("inside.txt");
    std::fs::write(&file, b"x").expect("create a file below");
    let sddl = dacl_sddl(&file).expect("read the file's access list");
    let entries = entries(&sddl);
    assert_eq!(entries.len(), 2, "{sddl}");
    assert!(
        entries.iter().all(|entry| entry[1].contains("ID")),
        "inherited from the folder: {sddl}"
    );
}

#[test]
fn an_sddl_without_an_access_list_changes_nothing() {
    let folder = Folder::new("refused");
    let before = dacl_sddl(&folder.0).expect("read the access list");
    assert!(set_protected_dacl(&folder.0, "O:SY").is_err());
    assert!(set_protected_dacl(&folder.0, "not an SDDL").is_err());
    assert_eq!(dacl_sddl(&folder.0).expect("read it again"), before);
}

#[test]
fn a_missing_path_is_an_error() {
    let folder = Folder::new("missing");
    let missing = folder.0.join("not-there");
    assert!(dacl_sddl(&missing).is_err());
    assert!(set_protected_dacl(&missing, "D:P(A;OICI;FA;;;SY)").is_err());
}
