use super::{
    check_settings, destination_reach, reaches_supplied_address, resolve_settings,
    settings_from_config,
};
use crate::SettingManifest;

fn priority() -> Vec<SettingManifest> {
    vec![
        SettingManifest {
            name: "priority_info".to_owned(),
            choices: ["1", "2", "3", "4", "5"].map(str::to_owned).to_vec(),
            default: Some("2".to_owned()),
        },
        SettingManifest {
            name: "priority_fixed".to_owned(),
            choices: ["1", "2", "3", "4", "5"].map(str::to_owned).to_vec(),
            default: None,
        },
    ]
}

fn pairs(values: &[(&str, &str)]) -> Vec<(String, String)> {
    values
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

/// RD-170-09: a target saves only names the destination declares and values it offers.
#[test]
fn a_target_is_saved_only_with_settings_its_destination_offers() {
    check_settings(&priority(), &[]).expect("nothing chosen");
    check_settings(&priority(), &pairs(&[("priority_info", "5")])).expect("offered");
    check_settings(&priority(), &pairs(&[("priority_fixed", "")])).expect("left unset");
    let code = |chosen: &[(&str, &str)]| {
        check_settings(&priority(), &pairs(chosen))
            .expect_err("refused")
            .code
            .unwrap_or_default()
    };
    assert_eq!(code(&[("priority_info", "7")]), "plugin.setting_invalid");
    assert_eq!(code(&[("priority_info", "high")]), "plugin.setting_invalid");
    assert_eq!(code(&[("volume", "3")]), "plugin.setting_unknown");
}

/// The plugin hears the stored value, else the default, and nothing it did not declare.
#[test]
fn a_delivery_resolves_settings_against_the_manifest() {
    assert_eq!(
        resolve_settings(&priority(), &[]),
        pairs(&[("priority_info", "2")])
    );
    assert_eq!(
        resolve_settings(
            &priority(),
            &pairs(&[
                ("priority_info", "4"),
                ("priority_fixed", "5"),
                ("volume", "3")
            ])
        ),
        pairs(&[("priority_info", "4"), ("priority_fixed", "5")])
    );
    // Unset on purpose, or a value the manifest no longer offers: the default, or nothing.
    assert_eq!(
        resolve_settings(
            &priority(),
            &pairs(&[("priority_info", "9"), ("priority_fixed", "")])
        ),
        pairs(&[("priority_info", "2")])
    );
}

#[test]
fn stored_settings_are_a_name_to_text_object() {
    let read = |config: serde_json::Value| settings_from_config(&config);
    assert!(
        read(serde_json::json!({"plugin_id": "x"}))
            .expect("none")
            .is_empty()
    );
    assert!(
        read(serde_json::json!({"settings": null}))
            .expect("null")
            .is_empty()
    );
    assert_eq!(
        read(serde_json::json!({"settings": {"priority_info": "4"}})).expect("object"),
        pairs(&[("priority_info", "4")])
    );
    for config in [
        serde_json::json!({"settings": {"priority_info": 4}}),
        serde_json::json!({"settings": ["priority_info"]}),
    ] {
        assert_eq!(
            read(config).expect_err("refused").code.as_deref(),
            Some("plugin.setting_invalid")
        );
    }
}

fn ntfy() -> Vec<String> {
    vec!["ntfy.sh".to_owned(), "*".to_owned()]
}

/// The service as if it listened on port 8710.
fn own() -> crate::OwnEndpoints {
    crate::OwnEndpoints::new(Some("0.0.0.0:8710".parse().expect("address")))
}

fn reach(destination: &str) -> Vec<String> {
    destination_reach(&ntfy(), destination, &own())
        .unwrap_or_else(|failure| panic!("{destination}: {failure}"))
}

fn refusal(destination: &str) -> String {
    destination_reach(&ntfy(), destination, &own())
        .expect_err(destination)
        .code
        .unwrap_or_default()
}

#[test]
fn a_bare_topic_reaches_the_named_service_and_never_the_catch_all() {
    assert_eq!(reach("downloads"), ["ntfy.sh"]);
    assert_eq!(reach("/downloads"), ["ntfy.sh"]);
    // Something that looks like a scheme but is not a web address is a topic like any
    // other, and gets the named service rather than the wildcard.
    assert_eq!(reach("ftp://files.example.org/x"), ["ntfy.sh"]);
}

#[test]
fn an_address_reaches_its_own_host_and_nothing_else() {
    assert_eq!(
        reach("https://ntfy.example.org/alerts"),
        ["ntfy.example.org"]
    );
    assert_eq!(
        reach(" HTTPS://Ntfy.Example.ORG:8443/alerts "),
        ["ntfy.example.org"]
    );
    // Even the public service, written out, is narrowed to itself.
    assert_eq!(reach("https://ntfy.sh/downloads"), ["ntfy.sh"]);
}

#[test]
fn plain_http_is_for_the_own_network_only() {
    for inside in [
        "http://192.168.1.20/alerts",
        "http://10.0.0.5:8080/alerts",
        "http://172.16.4.1/alerts",
        "http://[fd12:3456::1]/alerts",
        // This machine, on a port none of ours (owner, 2026-10-04).
        "http://127.0.0.1:2586/alerts",
        "http://[::1]:2586/alerts",
        "http://localhost:2586/alerts",
        // A single label is a service name in a Docker network or on the LAN.
        "http://ntfy:2586/alerts",
        "http://ntfy.lan/alerts",
        "http://pi.local/alerts",
    ] {
        assert_eq!(reach(inside).len(), 1, "{inside}");
    }
    for outside in [
        "http://ntfy.sh/alerts",
        "http://ntfy.example.org/alerts",
        "http://172.32.0.1/alerts",
        "http://8.8.8.8/alerts",
        // A public IPv6 address; 2001:db8::/32 is documentation and refused as not routable.
        "http://[2606:4700:4700::1111]/alerts",
        // A suffix that merely ends in the letters is not the suffix.
        "http://example.planlan/alerts",
    ] {
        assert_eq!(
            refusal(outside),
            "plugin.destination_not_encrypted",
            "{outside}"
        );
    }
}

/// RA-HOST-01: only an address the person entered may reach their own network; a bare
/// topic goes to the named public service, and a manifest without `*` never narrows.
#[test]
fn only_an_entered_address_reaches_the_own_network() {
    assert!(reaches_supplied_address(
        &ntfy(),
        "http://192.168.1.20/alerts"
    ));
    assert!(reaches_supplied_address(
        &ntfy(),
        " HTTPS://ntfy.lan/alerts"
    ));
    assert!(!reaches_supplied_address(&ntfy(), "downloads"));
    let named = vec!["api.telegram.org".to_owned()];
    assert!(!reaches_supplied_address(&named, "https://192.168.1.20/"));
}

/// RA-HOST-01, owner 2026-10-04: a destination on this machine is fine, one of the
/// service's own ports is not — its API and Click'n'Load — and link-local never is.
#[test]
fn our_own_ports_and_link_local_are_refused_when_the_target_is_saved() {
    for ours in [
        "http://127.0.0.1:8710/alerts",
        "https://localhost:8710/alerts",
        "http://[::1]:8710/alerts",
        "http://localhost:9666/alerts",
        "http://ntfy.localhost:9666/alerts",
    ] {
        assert_eq!(refusal(ours), "plugin.http_own_service", "{ours}");
    }
    for local in ["http://169.254.10.10/alerts", "http://[fe80::1]/alerts"] {
        assert_eq!(refusal(local), "plugin.http_local_target", "{local}");
    }
    assert_eq!(reach("https://localhost:2586/alerts"), ["localhost"]);
}

#[test]
fn an_address_without_a_host_is_refused() {
    assert_eq!(refusal("https://"), "plugin.destination_invalid");
}

/// RD-1240-12: a destination whose manifest names nothing but `*` (Plex, Jellyfin, Emby) can
/// only reach an address; anything else is refused, and an address still reaches its own host.
#[test]
fn a_manifest_of_only_the_catch_all_needs_an_address() {
    let only = vec!["*".to_owned()];
    for bare in ["plex", "192.168.1.10:32400", "", "ftp://plex.example.org"] {
        assert_eq!(
            destination_reach(&only, bare, &own())
                .expect_err(bare)
                .code
                .as_deref(),
            Some("plugin.destination_invalid"),
            "{bare:?}"
        );
    }
    assert_eq!(
        destination_reach(&only, "http://192.168.1.10:32400", &own()).expect("address"),
        ["192.168.1.10"]
    );
    assert_eq!(
        destination_reach(&only, "https://jellyfin.example.org/jf", &own()).expect("address"),
        ["jellyfin.example.org"]
    );
}

#[test]
fn a_manifest_without_the_catch_all_keeps_its_list_whatever_the_destination() {
    let telegram = vec!["api.telegram.org".to_owned()];
    assert_eq!(
        destination_reach(&telegram, "https://elsewhere.example/", &own()).expect("unchanged"),
        telegram
    );
    assert_eq!(
        destination_reach(&telegram, "-100123456", &own()).expect("unchanged"),
        telegram
    );
}
