/// The helper the release builds declares the scheme, and the applet hands a link to the
/// agent's `handle` command; both are only exercised on a Mac, so they are pinned here.
#[test]
fn the_macos_helper_declares_the_scheme_and_forwards_links() {
    let info = include_str!("../../../resources/macos/capture-helper-Info.plist");
    assert!(super::declares_url_scheme(info));
    assert!(
        info.contains("<string>org.rdownloader.nzb</string>"),
        "{info}"
    );
    let applet = include_str!("../../../resources/macos/rdownloader-capture.applescript");
    assert!(applet.contains("on open location "), "{applet}");
    assert!(applet.contains("\" handle \" & quoted form of"), "{applet}");
    // A helper from before 1.8, or one whose schemes name something else.
    assert!(!super::declares_url_scheme(
        &info.replace("<key>CFBundleURLSchemes</key>", "<key>Other</key>")
    ));
    assert!(!super::declares_url_scheme(&info.replace(
        "<string>rdownloader</string>",
        "<string>other</string>"
    )));
}

#[test]
fn the_windows_scheme_entries_declare_a_protocol_and_quote_the_command() {
    let entries = super::windows_scheme_entries(r"C:\Tools\rdownloader-capture.exe");
    let rendered: Vec<String> = entries
        .iter()
        .map(|entry| {
            format!(
                "{}|{}|{}",
                entry.key,
                entry.name.unwrap_or_default(),
                entry.value
            )
        })
        .collect();
    let joined = rendered.join("\n");
    // Without the empty `URL Protocol` value Windows does not treat the key as a
    // scheme at all, and the handler is simply never invoked.
    assert!(joined.contains("URL Protocol"), "{joined}");
    // The command has to quote both the executable and `%1`: an address with a space
    // would otherwise arrive split across arguments.
    assert!(
        joined.contains("\"C:\\Tools\\rdownloader-capture.exe\" handle \"%1\""),
        "{joined}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn the_scheme_desktop_entry_claims_the_handler_without_showing_a_second_app() {
    let rendered = super::render_linux_scheme_desktop(std::path::Path::new("/opt/rd/capture"))
        .expect("render");
    assert!(
        rendered.contains("MimeType=x-scheme-handler/rdownloader;"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Exec=\"/opt/rd/capture\" handle %u"),
        "{rendered}"
    );
    // Two entries named rDownloader in the application menu is a worse outcome than
    // one; this one exists only for the URL dispatcher.
    assert!(rendered.contains("NoDisplay=true"), "{rendered}");
}

#[cfg(target_os = "linux")]
#[test]
fn desktop_and_mime_outputs_cover_nzb_and_reserved_paths() {
    let desktop = super::render_linux_desktop(std::path::Path::new(
        "/tmp/Portable 100%/$Tools/\"capture\"",
    ))
    .expect("valid desktop path");
    assert!(desktop.contains(r#"Exec="/tmp/Portable 100%%/\$Tools/\"capture\"" open %f"#));
    assert!(desktop.contains("Terminal=false"));
    assert!(desktop.contains("NoDisplay=false"));
    assert!(desktop.contains("Comment=Import NZB into rDownloader"));
    assert!(desktop.contains("Categories=Network;"));
    assert!(desktop.contains("MimeType=application/x-nzb;"));
    assert!(super::LINUX_NZB_MIME_XML.contains("<glob pattern=\"*.nzb\"/>"));
}

#[test]
fn windows_registry_entries_include_import_verb_with_exact_quoting() {
    let executable = r"C:\Program Files\rDownloader\rdownloader-capture.exe";
    let entries = super::windows_registry_entries(executable);

    assert_eq!(
        entries.len(),
        8,
        "expected 4 file-association entries, 3 Import verb entries and the notification \
             AppUserModelID"
    );
    assert!(
        entries.iter().any(|entry| entry.key
            == format!(
                r"HKCU\Software\Classes\AppUserModelId\{}",
                super::WINDOWS_APP_ID
            )
            && entry.name == Some("DisplayName")
            && entry.value == "rDownloader Capture"),
        "desktop notifications need a registered sender"
    );

    let import_key = r"HKCU\Software\Classes\SystemFileAssociations\.nzb\shell\rDownloader.Import";
    let import_command_key =
        r"HKCU\Software\Classes\SystemFileAssociations\.nzb\shell\rDownloader.Import\command";

    assert!(entries.iter().any(|entry| entry.key == import_key
        && entry.name.is_none()
        && entry.value == "Import into rDownloader"));

    assert!(entries.iter().any(|entry| entry.key == import_key
        && entry.name == Some("Icon")
        && entry.value == r#""C:\Program Files\rDownloader\rdownloader-capture.exe",0"#));

    assert!(entries.iter().any(|entry| entry.key == import_command_key
        && entry.name.is_none()
        && entry.value == r#""C:\Program Files\rDownloader\rdownloader-capture.exe" open "%1""#));
}

/// Install and remove move over one list, so a key cannot end up on only one side. What can
/// still go wrong is an entry whose removal key does not actually cover it, and that is what
/// this pins -- including a deliberately mismatched entry, to show the check bites
/// (RD-109-15).
#[test]
fn every_entry_is_covered_by_the_key_that_removes_it() {
    let executable = r"C:\Tools\rdownloader-capture.exe";
    for kind in [super::Kind::Association, super::Kind::Scheme] {
        let entries = super::windows_entries(kind, executable);
        assert!(!entries.is_empty());
        for entry in &entries {
            assert!(
                entry.key.starts_with(&entry.removes),
                "{} is written but `remove` would never reach it: it deletes {}",
                entry.key,
                entry.removes
            );
        }
        // Every removal key belongs to an entry, and every entry to a removal key.
        let removal = super::windows_removal_keys(&entries);
        for key in &removal {
            assert!(
                entries.iter().any(|entry| &entry.removes == key),
                "{key} is deleted but nothing writes under it"
            );
        }
        for entry in &entries {
            assert!(
                removal.contains(&entry.removes),
                "{} is written and never deleted",
                entry.key
            );
        }
    }

    // The same check against an entry that was added on the writing side only, in the way
    // the AppUserModelID once was: it names a branch no removal key reaches.
    let stray = super::RegistryEntry {
        key: r"HKCU\Software\Classes\rDownloader.Something".to_owned(),
        name: None,
        value: "left behind".to_owned(),
        removes: r"HKCU\Software\Classes\.nzb".to_owned(),
    };
    assert!(
        !stray.key.starts_with(&stray.removes),
        "the assertion above is what turns such an entry red"
    );
}

/// The notification sender is the key that was written and never removed.
#[test]
fn association_remove_takes_the_notification_sender_with_it() {
    let keys = super::windows_removal_keys(&super::windows_registry_entries("x"));
    assert!(
        keys.contains(&format!(
            r"HKCU\Software\Classes\AppUserModelId\{}",
            super::WINDOWS_APP_ID
        )),
        "{keys:?}"
    );
}

/// The deliberate restraint: rDownloader's own verb goes, the shared parent stays.
#[test]
fn the_shared_file_association_parent_is_never_deleted() {
    let parent = r"HKCU\Software\Classes\SystemFileAssociations\.nzb";
    let keys = super::windows_removal_keys(&super::windows_registry_entries("x"));
    for key in &keys {
        assert!(
            !parent.starts_with(key.as_str()),
            "{key} would take {parent} with it, and other software registers verbs there"
        );
    }
    assert!(
        keys.iter().any(|key| key
            == r"HKCU\Software\Classes\SystemFileAssociations\.nzb\shell\rDownloader.Import"),
        "rDownloader's own verb is still removed: {keys:?}"
    );
}
