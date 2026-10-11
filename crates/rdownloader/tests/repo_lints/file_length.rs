//! At most 500 lines per file (owner, 2026-10-08, DOC-13; RD-1190-07), for every Rust source of
//! the repository: the crates, the plugins, the SDK templates.
//!
//! A production file over the limit is split -- a module of its own, re-exported where the
//! old path was public -- unless it is one of `EXCEPTIONS`, each with the reason it stays whole.
//! A test file over the limit on 2026-10-08 is in `BASELINE` with its count then: a ratchet, so
//! the file may shrink and never grow past that count, and an entry whose file is back at the
//! limit, or gone, leaves the list. A test file not on the list stays at the limit.
//!
//! Lines are counted as `wc -l` counts them. The web interface and the extension have their own
//! ratchet (`web/src/fileLength.test.ts`), the scripts theirs (`scripts/tests/file-length.sh`).

use std::path::{Path, PathBuf};

use crate::workspace_root;

const LIMIT: usize = 500;

/// Production files above the limit on purpose. No count: each grows by a row or a field with
/// the feature that needs one, and splitting it would scatter what is read as one table.
const EXCEPTIONS: &[(&str, &str)] = &[
    (
        "crates/rd-api-core/src/dto/settings.rs",
        "one struct, the settings DTO; split, its fields would become serde-flattened \
         sub-structs, which changes the OpenAPI document and every struct literal",
    ),
    (
        "crates/rd-api-core/src/scope_policy/table.rs",
        "the scope table, one row per route and method",
    ),
    (
        "crates/rd-api/src/mcp_coverage.rs",
        "the MCP coverage table, one row per route",
    ),
];

/// Test files above the limit on 2026-10-08 and their line counts then; may only go down.
#[rustfmt::skip]
const BASELINE: &[(&str, usize)] = &[
    ("crates/rd-api-core/src/link_check_cache_tests.rs", 625),
    ("crates/rd-api-core/src/remote_job_service/tests.rs", 1242),
    ("crates/rd-api/tests/access/api_tokens.rs", 612),
    ("crates/rd-api/tests/access/audit.rs", 816),
    ("crates/rd-api/tests/access/auth_profiles.rs", 549),
    ("crates/rd-api/tests/access/oidc.rs", 742),
    ("crates/rd-api/tests/admin/automations.rs", 520),
    ("crates/rd-api/tests/admin/metrics.rs", 510),
    ("crates/rd-api/tests/admin/plugin_auto_updates.rs", 728),
    ("crates/rd-api/tests/admin/plugin_versions.rs", 524),
    ("crates/rd-api/tests/admin/settings_backup.rs", 698),
    ("crates/rd-api/tests/admin/updates.rs", 736),
    ("crates/rd-api/tests/intake/captcha.rs", 618),
    ("crates/rd-api/tests/intake/torrent.rs", 1457),
    ("crates/rd-api/tests/mcp/mcp.rs", 1687),
    ("crates/rd-api/tests/queue/category_move_and_reset.rs", 557),
    ("crates/rd-api/tests/sources/compat_qbittorrent.rs", 1016),
    ("crates/rd-api/tests/sources/indexer_search.rs", 564),
    ("crates/rd-api/tests/sources/indexers.rs", 993),
    ("crates/rd-api/tests/sources/subscriptions.rs", 1651),
    ("crates/rd-captcha/tests/broker.rs", 524),
    ("crates/rd-db/src/archive_password_tests.rs", 592),
    ("crates/rd-db/src/tests/auth.rs", 509),
    ("crates/rd-db/src/tests/collector.rs", 875),
    ("crates/rd-db/src/tests/downloads.rs", 943),
    ("crates/rd-db/src/tests/mirrors.rs", 823),
    ("crates/rd-db/src/tests/packages.rs", 649),
    ("crates/rd-db/src/tests/subscriptions.rs", 755),
    ("crates/rd-db/tests/database/settings_backup.rs", 507),
    ("crates/rd-extract/src/direct_unpack_tests.rs", 530),
    ("crates/rd-files/src/sort_tests.rs", 513),
    ("crates/rd-http/src/multisource_tests.rs", 536),
    ("crates/rd-http/tests/crash_restart.rs", 849),
    ("crates/rd-media/tests/fake_ytdlp.rs", 591),
    ("crates/rd-object-storage/src/tests.rs", 980),
    ("crates/rd-object-storage/tests/s3.rs", 658),
    ("crates/rd-plugin-ext/src/crawler_tests.rs", 531),
    ("crates/rd-plugin-ext/src/remote_job_tests.rs", 675),
    ("crates/rd-plugin-ext/tests/contract/box_contract.rs", 1211),
    ("crates/rd-plugin-ext/tests/contract/directory_index_crawler_contract.rs", 794),
    ("crates/rd-plugin-ext/tests/contract/dropbox_contract.rs", 1165),
    ("crates/rd-plugin-ext/tests/contract/google_drive_contract.rs", 1096),
    ("crates/rd-plugin-ext/tests/contract/mega_account_contract.rs", 573),
    ("crates/rd-plugin-ext/tests/contract/mega_auth_contract.rs", 502),
    ("crates/rd-plugin-ext/tests/contract/mega_contract.rs", 776),
    ("crates/rd-plugin-ext/tests/contract/nextcloud_crawler_contract.rs", 1037),
    ("crates/rd-plugin-ext/tests/contract/oauth_contract.rs", 930),
    ("crates/rd-plugin-ext/tests/contract/offcloud_cloud_contract.rs", 913),
    ("crates/rd-plugin-ext/tests/contract/onedrive_contract.rs", 1284),
    ("crates/rd-plugin-ext/tests/contract/pcloud_contract.rs", 1062),
    ("crates/rd-plugin-ext/tests/contract/premiumize_transfers_contract.rs", 1104),
    ("crates/rd-plugin-ext/tests/contract/putio_contract.rs", 729),
    ("crates/rd-plugin-ext/tests/contract/putio_remote_job_contract.rs", 907),
    ("crates/rd-plugin-ext/tests/contract/realdebrid_contract.rs", 890),
    ("crates/rd-plugin-ext/tests/contract/remote_job_contract.rs", 781),
    ("crates/rd-plugin-ext/tests/contract/seedr_remote_job_contract.rs", 892),
    ("crates/rd-plugin-ext/tests/contract/torbox_remote_job_contract.rs", 1300),
    ("crates/rd-plugin-host/src/component_tests.rs", 561),
    ("crates/rd-plugin-host/src/manifest_tests.rs", 957),
    ("crates/rd-plugin-host/src/native/expand/tests.rs", 557),
    ("crates/rd-plugin-host/src/native/host_tests.rs", 1051),
    ("crates/rd-plugin-host/src/repository_tests.rs", 594),
    ("crates/rd-plugin-host/src/repository_trust_tests.rs", 509),
    ("crates/rd-plugin-host/tests/krakenfiles_parity.rs", 521),
    ("crates/rd-plugin-host/tests/turbobit_hitfile_contract.rs", 587),
    ("crates/rd-scheduler/src/control_tests.rs", 678),
    ("crates/rd-scheduler/tests/crash_restart.rs", 552),
    ("crates/rd-scheduler/tests/provider_transfer_credential.rs", 571),
    ("crates/rd-sftp/tests/transfer.rs", 572),
    ("crates/rd-tools/tests/managed_tools.rs", 804),
    ("crates/rd-usenet/src/hopeless_tests.rs", 511),
    ("crates/rd-usenet/src/test_support.rs", 737),
    ("crates/rd-usenet/src/worker_tests.rs", 896),
];

fn rust_files(directory: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if kind.is_dir() {
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                rust_files(&path, out);
            }
        } else if kind.is_file() && name.ends_with(".rs") {
            out.push(path);
        }
    }
}

/// A test file: below a `tests` directory, or named as one (`*_tests.rs`, `tests.rs`,
/// `test_support.rs`).
fn is_test(relative: &str) -> bool {
    let name = relative.rsplit('/').next().unwrap_or(relative);
    relative.split('/').any(|part| part == "tests")
        || name.ends_with("_tests.rs")
        || name == "tests.rs"
        || name == "test_support.rs"
}

fn line_count(path: &Path) -> usize {
    std::fs::read(path)
        .map(|bytes| bytes.iter().filter(|byte| **byte == b'\n').count())
        .unwrap_or(0)
}

/// Every Rust source over the limit, relative to the root with `/`, and its count.
fn over_limit() -> Vec<(String, usize)> {
    let root = workspace_root();
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    assert!(!files.is_empty(), "no Rust sources were found at all");
    let mut over: Vec<(String, usize)> = files
        .iter()
        .map(|path| (path, line_count(path)))
        .filter(|(_, lines)| *lines > LIMIT)
        .map(|(path, lines)| {
            let relative = path.strip_prefix(&root).unwrap_or(path);
            (relative.to_string_lossy().replace('\\', "/"), lines)
        })
        .collect();
    over.sort();
    over
}

#[test]
fn no_rust_source_outgrows_the_limit() {
    let mut offenders = Vec::new();
    for (path, lines) in over_limit() {
        if EXCEPTIONS.iter().any(|(listed, _)| *listed == path) {
            continue;
        }
        match BASELINE.iter().find(|(listed, _)| *listed == path) {
            Some((_, baseline)) if lines <= *baseline => {}
            Some((_, baseline)) => {
                offenders.push(format!("{path}: {lines} lines, grown past its {baseline}"));
            }
            None if is_test(&path) => offenders.push(format!("{path}: {lines} lines")),
            None => offenders.push(format!("{path}: {lines} lines of production code")),
        }
    }
    assert!(
        offenders.is_empty(),
        "these Rust sources are over {LIMIT} lines (AGENTS.md, Conventions): split them into \
         modules of their own, re-exported where the old path was public; a test file on \
         BASELINE may shrink, never grow\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn the_lists_name_only_files_still_over_the_limit() {
    let over = over_limit();
    let mut stale = Vec::new();
    let listed = EXCEPTIONS
        .iter()
        .map(|(path, _)| *path)
        .chain(BASELINE.iter().map(|(path, _)| *path));
    for path in listed {
        if !over.iter().any(|(file, _)| file == path) {
            stale.push(format!("{path}: at or under {LIMIT} lines, or gone"));
        }
    }
    for (path, _) in BASELINE {
        if !is_test(path) {
            stale.push(format!(
                "{path}: production code; split it, or name it in EXCEPTIONS with its reason"
            ));
        }
    }
    assert!(
        stale.is_empty(),
        "remove these entries from file_length.rs, the list only shrinks:\n  {}",
        stale.join("\n  ")
    );
}
