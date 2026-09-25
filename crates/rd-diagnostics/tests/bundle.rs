//! The diagnostic bundle is inventoried deterministically and holds no secret (RD-110-02).

use std::io::Read;

use chrono::{TimeZone, Utc};
use rd_core::LogLevel;
use rd_db::LogRecord;
use rd_diagnostics::{
    BundleInput, Check, CheckStatus,
    bundle::{self, is_file_name, scrub_configuration},
};

fn input() -> BundleInput {
    BundleInput {
        application_version: "1.1.0".to_owned(),
        os: "linux".to_owned(),
        arch: "x86_64".to_owned(),
        plugins: vec![
            ("zippyshare".to_owned(), "0.3.0".to_owned()),
            ("ddownload".to_owned(), "0.3.1".to_owned()),
        ],
        configuration: serde_json::json!({
            "max_active_files": 3,
            "custom_ca_pem": "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----",
            "managed_tools_manifest_url": "https://tools.example/manifest.json?token=sk-live-4242",
            "completion_script": "after.sh",
            "external_url": null,
            "admin_password": "hunter2",
            "nested": { "api_key": "AKIAIOSFODNN7EXAMPLE", "keep": true }
        }),
        checks: vec![
            Check::new(
                "download helpers",
                "yt-dlp",
                CheckStatus::Ok,
                "/usr/bin/yt-dlp (Path)",
            )
            .note("version: 2026.01.01 - compatible"),
            Check::new(
                "Reverse proxy",
                "external URL",
                CheckStatus::Info,
                "not set",
            ),
        ],
        recent_errors: vec![LogRecord {
            id: 7,
            recorded_at: Utc
                .with_ymd_and_hms(2026, 9, 20, 10, 0, 0)
                .single()
                .expect("time"),
            level: LogLevel::Error,
            component: "rd_http::engine".to_owned(),
            code: Some("http.status".to_owned()),
            correlation_id: Some("dl-1".to_owned()),
            message: "failed for https://h.example/f?token=[redacted]".to_owned(),
            fields: Default::default(),
        }],
        log_records_total: 1234,
    }
}

fn all_ids(input: &BundleInput) -> Vec<String> {
    bundle::inventory(input)
        .expect("inventory")
        .entries
        .into_iter()
        .map(|entry| entry.id)
        .collect()
}

#[test]
fn the_same_state_gives_the_same_inventory_and_digest() {
    let first = bundle::inventory(&input()).expect("inventory");
    let second = bundle::inventory(&input()).expect("inventory");
    assert_eq!(first, second);
    assert_eq!(
        first
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        [
            "versions",
            "configuration",
            "system-checks",
            "doctor",
            "recent-errors"
        ]
    );
    assert_eq!(first.digest.len(), 64);
    assert!(
        !first.excluded.is_empty(),
        "the preview says what never goes in"
    );

    // A record logged between preview and approval changes a count, never the digest.
    let mut busier = input();
    busier.log_records_total += 1;
    busier.recent_errors.push(busier.recent_errors[0].clone());
    let third = bundle::inventory(&busier).expect("inventory");
    assert_eq!(third.digest, first.digest);
    assert_ne!(third.entries[4].items, first.entries[4].items);
}

#[test]
fn the_same_state_and_clock_give_byte_identical_archives() {
    let now = Utc
        .with_ymd_and_hms(2026, 9, 20, 12, 30, 0)
        .single()
        .expect("time");
    let ids = all_ids(&input());
    let first = bundle::build(&input(), &ids, now).expect("bundle");
    let second = bundle::build(&input(), &ids, now).expect("bundle");
    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.manifest, second.manifest);
    assert_eq!(first.manifest.entries.len(), 5);
    assert!(first.manifest.omitted.is_empty());
    assert_eq!(first.manifest.created_at, "2026-09-20T12:30:00Z");
    assert_eq!(
        bundle::file_name(now),
        "rdownloader-diagnostics-20260920T123000Z.zip"
    );
}

#[test]
fn a_deselected_entry_is_left_out_and_named_in_the_manifest() {
    let now = Utc
        .with_ymd_and_hms(2026, 9, 20, 12, 30, 0)
        .single()
        .expect("time");
    let built = bundle::build(&input(), &["versions".to_owned(), "doctor".to_owned()], now)
        .expect("bundle");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(built.bytes)).expect("zip");
    let mut names: Vec<String> = (0..archive.len())
        .map(|index| archive.by_index(index).expect("entry").name().to_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["doctor.txt", "manifest.json", "versions.json"]);
    assert_eq!(
        built.manifest.omitted,
        ["configuration", "system-checks", "recent-errors"]
    );
    let mut manifest = String::new();
    archive
        .by_name("manifest.json")
        .expect("manifest")
        .read_to_string(&mut manifest)
        .expect("read");
    let parsed: rd_diagnostics::Manifest = serde_json::from_str(&manifest).expect("json");
    assert_eq!(parsed, built.manifest);
    assert!(
        bundle::build(&input(), &["everything".to_owned()], now).is_err(),
        "an unknown id refuses rather than producing a different bundle than approved"
    );
}

#[test]
fn no_entry_of_the_archive_carries_a_secret() {
    let now = Utc::now();
    let built = bundle::build(&input(), &all_ids(&input()), now).expect("bundle");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(built.bytes)).expect("zip");
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).expect("entry");
        let mut text = String::new();
        entry.read_to_string(&mut text).expect("read");
        for canary in ["hunter2", "sk-live-4242", "AKIAIOSFODNN7EXAMPLE", "MIIB"] {
            assert!(
                !text.contains(canary),
                "{} carries {canary}: {text}",
                entry.name()
            );
        }
    }
}

#[test]
fn the_configuration_is_scrubbed_by_key_and_by_value() {
    let (scrubbed, replaced) = scrub_configuration(&input().configuration);
    assert_eq!(scrubbed["max_active_files"], 3);
    assert_eq!(scrubbed["completion_script"], "after.sh");
    assert_eq!(scrubbed["custom_ca_pem"], "[present]");
    assert_eq!(scrubbed["admin_password"], "[redacted]");
    assert_eq!(scrubbed["nested"]["api_key"], "[redacted]");
    assert_eq!(scrubbed["nested"]["keep"], true);
    assert_eq!(
        scrubbed["managed_tools_manifest_url"],
        "https://tools.example/manifest.json?token=%5Bredacted%5D"
    );
    assert_eq!(replaced, ["admin_password", "api_key", "custom_ca_pem"]);
}

#[test]
fn only_a_generated_name_is_a_bundle_file_name() {
    assert!(is_file_name("rdownloader-diagnostics-20260920T123000Z.zip"));
    assert!(!is_file_name("rdownloader-diagnostics-2026092T123000Z.zip"));
    assert!(!is_file_name(
        "../rdownloader-diagnostics-20260920T123000Z.zip"
    ));
    assert!(!is_file_name(
        "rdownloader-diagnostics-20260920T123000Z.zip/.."
    ));
    assert!(!is_file_name("rdownloader.sqlite3"));
}
