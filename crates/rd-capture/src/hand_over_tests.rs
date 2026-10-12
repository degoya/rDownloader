use std::path::Path;

use super::{FileKind, HANDED_OVER, Outcome, UNSUPPORTED, display_name, kind_of};

/// Every container the service reads by name, the NZB and the torrent, whatever the case of the
/// extension; each goes to the route that reads it.
#[test]
fn each_registered_type_goes_to_its_route() {
    for (name, kind) in [
        ("release.nzb", FileKind::Nzb),
        ("release.dlc", FileKind::Container),
        ("Release.CCF", FileKind::Container),
        ("links.rsdf", FileKind::Container),
        ("holiday.rdlinks", FileKind::Container),
        ("job.crawljob", FileKind::Container),
        ("ubuntu.iso.torrent", FileKind::Torrent),
    ] {
        assert_eq!(kind_of(Path::new(name)).ok(), Some(kind), "{name}");
    }
}

/// The list names exactly the containers of the job, with the service's own names for them,
/// and leaves out what is too general to claim or has no file route.
#[test]
fn the_list_is_the_jobs_and_the_services() {
    let containers: Vec<&str> = HANDED_OVER
        .iter()
        .filter(|(_, kind)| *kind == FileKind::Container)
        .map(|(extension, _)| *extension)
        .collect();
    assert_eq!(
        containers,
        [".dlc", ".ccf", ".rsdf", ".rdlinks", ".crawljob"]
    );
    for extension in &containers {
        assert!(
            rd_collector::ContainerFormat::from_file_name(&format!("x{extension}")).is_some(),
            "{extension}: the service would not know it"
        );
    }
    for extension in [".txt", ".metalink", ".meta4"] {
        assert!(
            HANDED_OVER.iter().all(|(known, _)| *known != extension),
            "{extension}"
        );
    }
}

/// Another type is refused with the stable code, naming the extension.
#[test]
fn an_unknown_extension_is_refused_by_name() {
    let refused = kind_of(Path::new("/home/me/notes.MD")).expect_err("refused");
    assert_eq!(refused.extension, "md");
    let message = refused.to_string();
    assert!(message.starts_with(UNSUPPORTED), "{message}");
    assert!(message.contains(".md"), "{message}");

    let bare = kind_of(Path::new("/home/me/README")).expect_err("refused");
    assert_eq!(bare.extension, "");
    assert!(bare.to_string().contains("without an extension"));
    // A text file is a container to the service, but not one the desktop hands over.
    assert!(kind_of(Path::new("links.txt")).is_err());
}

/// Several files are one notice with the count; one that arrived alone is not announced twice.
#[test]
fn several_files_are_one_notice_with_the_count() {
    let alone = Outcome {
        handed: vec!["a.dlc".to_owned()],
        refused: Vec::new(),
    };
    assert!(!alone.tells());
    assert_eq!(alone.summary(), "Handed to rDownloader: a.dlc");

    let three = Outcome {
        handed: vec!["a.dlc".to_owned(), "b.nzb".to_owned(), "c.ccf".to_owned()],
        refused: Vec::new(),
    };
    assert!(three.tells());
    assert_eq!(three.summary(), "3 files handed to rDownloader");
}

/// A refusal is always told, with each refused file's name and code.
#[test]
fn a_refusal_names_the_file_and_its_code() {
    let mixed = Outcome {
        handed: vec!["a.dlc".to_owned(), "b.nzb".to_owned()],
        refused: vec![("notes.md".to_owned(), UNSUPPORTED.to_owned())],
    };
    assert!(mixed.tells());
    assert_eq!(
        mixed.summary(),
        "2 of 3 files handed to rDownloader; not taken: notes.md (capture.file_type_unsupported)"
    );

    let none = Outcome {
        handed: Vec::new(),
        refused: vec![("x.dlc".to_owned(), "dlc.service_disabled".to_owned())],
    };
    assert!(none.tells());
    assert_eq!(
        none.summary(),
        "0 of 1 files handed to rDownloader; not taken: x.dlc (dlc.service_disabled)"
    );
}

#[test]
fn a_file_is_reported_by_its_own_name() {
    assert_eq!(
        display_name(Path::new("/home/me/Downloads/release.dlc")),
        "release.dlc"
    );
}
