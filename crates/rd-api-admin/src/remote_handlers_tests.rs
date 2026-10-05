use rd_core::{RemoteAuthMode, RemoteProtocol};

use super::{parse_host, validate_auth, validate_port};

#[test]
fn hosts_normalise_the_way_links_are_matched() {
    assert_eq!(
        parse_host("Files.EXAMPLE.com.").expect("host"),
        "files.example.com"
    );
    assert_eq!(
        parse_host("ftp://files.example.com/pub").expect("host"),
        "files.example.com"
    );
    // Unicode has to become punycode here, or a link would never match the entry.
    assert_eq!(
        parse_host("\u{e9}xample.fr").expect("host"),
        "xn--xample-9ua.fr"
    );
    assert!(parse_host("").is_err());
    assert!(parse_host("   ").is_err());
}

#[test]
fn keys_and_agents_are_refused_for_ftp() {
    for mode in [RemoteAuthMode::PrivateKey, RemoteAuthMode::Agent] {
        assert!(
            validate_auth(
                RemoteProtocol::Ftp,
                mode,
                Some("bob"),
                None,
                Some("k"),
                true
            )
            .is_err()
        );
    }
    assert!(
        validate_auth(
            RemoteProtocol::Sftp,
            RemoteAuthMode::PrivateKey,
            Some("bob"),
            None,
            Some("k"),
            true
        )
        .is_ok()
    );
}

#[test]
fn a_mode_without_its_credential_is_refused_on_create() {
    assert!(
        validate_auth(
            RemoteProtocol::Ftp,
            RemoteAuthMode::Password,
            Some("bob"),
            None,
            None,
            true
        )
        .is_err()
    );
    // On update the stored credential stands in for the missing field.
    assert!(
        validate_auth(
            RemoteProtocol::Ftp,
            RemoteAuthMode::Password,
            Some("bob"),
            Some("kept"),
            None,
            false
        )
        .is_ok()
    );
}

#[test]
fn anonymous_needs_nothing_at_all() {
    assert!(
        validate_auth(
            RemoteProtocol::Ftp,
            RemoteAuthMode::Anonymous,
            None,
            None,
            None,
            true
        )
        .is_ok()
    );
}

#[test]
fn port_zero_is_refused() {
    assert!(validate_port(0).is_err());
    assert!(validate_port(21).is_ok());
}
