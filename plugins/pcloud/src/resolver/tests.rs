use pcloud_common::metadata::Metadata;

use super::{MAX_TREE_DEPTH, find_file, matches};

#[test]
fn only_pcloud_file_addresses_are_claimed() {
    assert!(matches(
        "https://my.pcloud.com/#/filemanager?folder=42&fileid=123"
    ));
    assert!(matches(
        "https://e.pcloud.link/publink/show?code=XZabc&fileid=7"
    ));
    // A folder and a bare public link are the crawler's, and a stranger's host is nobody's.
    assert!(!matches("https://my.pcloud.com/#/filemanager?folder=42"));
    assert!(!matches("https://e.pcloud.link/publink/show?code=XZabc"));
    assert!(!matches("https://ddownload.com/f/abc"));
}

fn folder(folder_id: u64, contents: Vec<Metadata>) -> Metadata {
    Metadata {
        name: Some(format!("d{folder_id}")),
        isfolder: true,
        folderid: Some(folder_id),
        contents,
        ..Metadata::default()
    }
}

fn file(file_id: u64) -> Metadata {
    Metadata {
        name: Some(format!("f{file_id}.bin")),
        isfolder: false,
        fileid: Some(file_id),
        size: Some(file_id),
        ..Metadata::default()
    }
}

/// `showpublink` answers a folder link with its whole tree, so the file the address named
/// is found in it rather than fetched a second time.
#[test]
fn the_file_a_public_address_names_is_found_in_the_tree_the_link_answered_with() {
    let tree = folder(1, vec![file(10), folder(2, vec![file(20), file(21)])]);
    assert_eq!(
        find_file(&tree, 21, 0).and_then(|found| found.fileid),
        Some(21)
    );
    assert!(find_file(&tree, 99, 0).is_none());
    // A link that *is* one file answers with that file, not with a tree.
    assert_eq!(find_file(&file(5), 5, 0).and_then(|f| f.fileid), Some(5));
}

/// A tree a stranger built is not a reason to recurse without a floor.
#[test]
fn a_tree_deeper_than_the_floor_is_not_followed_for_ever() {
    let mut node = file(7);
    for level in 0..MAX_TREE_DEPTH + 5 {
        node = folder(u64::from(level) + 100, vec![node]);
    }
    assert!(find_file(&node, 7, 0).is_none());
    let shallow = folder(1, vec![folder(2, vec![file(7)])]);
    assert!(find_file(&shallow, 7, 0).is_some());
}
