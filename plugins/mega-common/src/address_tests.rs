use super::*;

#[test]
fn a_current_file_address_is_a_file() {
    let target =
        Target::parse("https://mega.nz/file/yuZ0QJ6J#jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc");
    assert_eq!(
        target,
        Some(Target::File {
            handle: "yuZ0QJ6J".to_owned(),
            key: "jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc".to_owned(),
        })
    );
}

#[test]
fn a_current_folder_address_is_a_folder() {
    assert_eq!(
        Target::parse("https://mega.nz/folder/e4diDZ7T#iJnegBO_m6OXBQp27lHCrg"),
        Some(Target::Folder {
            handle: "e4diDZ7T".to_owned(),
            key: "iJnegBO_m6OXBQp27lHCrg".to_owned(),
        })
    );
}

#[test]
fn a_file_named_inside_a_folder_keeps_both_handles() {
    assert_eq!(
        Target::parse("https://mega.nz/folder/e4diDZ7T#iJnegBO_m6OXBQp27lHCrg/file/KlVgwR4B"),
        Some(Target::FolderChild {
            folder: "e4diDZ7T".to_owned(),
            key: "iJnegBO_m6OXBQp27lHCrg".to_owned(),
            node: "KlVgwR4B".to_owned(),
        })
    );
    assert_eq!(
        Target::child_url("e4diDZ7T", "iJnegBO_m6OXBQp27lHCrg", "KlVgwR4B"),
        "https://mega.nz/folder/e4diDZ7T#iJnegBO_m6OXBQp27lHCrg/file/KlVgwR4B"
    );
}

#[test]
fn the_legacy_forms_are_read_too() {
    assert_eq!(
        Target::parse("https://mega.nz/#!yuZ0QJ6J!jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc"),
        Some(Target::File {
            handle: "yuZ0QJ6J".to_owned(),
            key: "jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc".to_owned(),
        })
    );
    assert_eq!(
        Target::parse("https://mega.co.nz/#F!e4diDZ7T!iJnegBO_m6OXBQp27lHCrg"),
        Some(Target::Folder {
            handle: "e4diDZ7T".to_owned(),
            key: "iJnegBO_m6OXBQp27lHCrg".to_owned(),
        })
    );
}

#[test]
fn an_address_without_a_key_is_not_claimed() {
    // Nothing here can be downloaded without the fragment, and claiming it would end the
    // link with a refusal instead of leaving it to whatever else might know the address.
    assert_eq!(Target::parse("https://mega.nz/file/yuZ0QJ6J"), None);
    assert_eq!(Target::parse("https://mega.nz/folder/e4diDZ7T"), None);
    assert_eq!(Target::parse("https://mega.nz/file/yuZ0QJ6J#short"), None);
}

#[test]
fn other_hosts_and_other_paths_are_not_ours() {
    assert_eq!(
        Target::parse("https://notmega.nz/file/yuZ0QJ6J#iJnegBO_m6OXBQp27lHCrg"),
        None
    );
    assert_eq!(
        Target::parse("https://mega.nz.example.com/file/yuZ0QJ6J#iJnegBO_m6OXBQp27lHCrg"),
        None
    );
    assert_eq!(Target::parse("https://mega.nz/register"), None);
    assert_eq!(Target::parse("ftp://mega.nz/file/x#y"), None);
}

#[test]
fn www_and_the_old_host_are_the_same_addresses() {
    assert!(matches!(
        Target::parse(
            "https://www.mega.nz/file/yuZ0QJ6J#jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc"
        ),
        Some(Target::File { .. })
    ));
    assert!(matches!(
        Target::parse(
            "https://mega.co.nz/file/yuZ0QJ6J#jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc"
        ),
        Some(Target::File { .. })
    ));
}

#[test]
fn a_folder_address_naming_something_other_than_a_file_is_refused() {
    assert_eq!(
        Target::parse("https://mega.nz/folder/e4diDZ7T#iJnegBO_m6OXBQp27lHCrg/file/!!"),
        None
    );
}

#[test]
fn an_account_address_names_a_node_and_carries_no_key() {
    // RD-120-30. The key of an account's node is under the master key, which only the host
    // holds, so neither form has a fragment -- and one that does is nobody's address.
    assert_eq!(
        Target::parse("https://mega.nz/fm/G5NikTgR"),
        Some(Target::AccountNode {
            handle: "G5NikTgR".to_owned(),
        })
    );
    assert_eq!(
        Target::parse("https://mega.nz/fm/file/KlVgwR4B"),
        Some(Target::AccountFile {
            handle: "KlVgwR4B".to_owned(),
        })
    );
    assert_eq!(
        Target::account_file_url("KlVgwR4B"),
        "https://mega.nz/fm/file/KlVgwR4B"
    );
    assert_eq!(
        Target::parse(&Target::account_file_url("KlVgwR4B")),
        Some(Target::AccountFile {
            handle: "KlVgwR4B".to_owned(),
        })
    );
    for refused in [
        "https://mega.nz/fm/G5NikTgR#iJnegBO_m6OXBQp27lHCrg",
        "https://mega.nz/fm/file",
        "https://mega.nz/fm/",
        "https://mega.nz/fm/G5NikTgR/KlVgwR4B",
        "https://mega.nz/fm/bad!handle",
    ] {
        assert_eq!(Target::parse(refused), None, "{refused}");
    }
}
