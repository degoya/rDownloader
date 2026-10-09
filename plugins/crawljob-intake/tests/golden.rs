//! The `.crawljob` rDownloader exports (RD-1210-01), read back by this parser.
//!
//! `rd-collector`'s writer produces `golden/rdownloader-export.crawljob` byte for byte; this
//! test is the other half of the round trip: the file is claimed as a crawljob, and every block
//! comes back with its links and its package name.

use rd_plugin_crawljob_intake::parse::{ParsedJob, claims, jobs_in};

#[test]
fn the_exported_crawljob_is_read_back() {
    let root = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
    );
    let text = std::fs::read_to_string(root.join("tests/golden/rdownloader-export.crawljob"))
        .expect("golden file");

    assert!(claims(&text));
    assert_eq!(
        jobs_in(&text),
        vec![
            ParsedJob {
                urls: vec![
                    "https://ddownload.com/abc123/holiday.part1.rar".to_owned(),
                    "https://ddownload.com/def456/holiday.part2.rar".to_owned(),
                ],
                package_name: Some("Holiday 2026".to_owned()),
                file_name: None,
            },
            ParsedJob {
                urls: vec!["http://example.com/file.bin".to_owned()],
                package_name: Some("Second line".to_owned()),
                file_name: None,
            },
        ]
    );
}
