//! RD-1280-19: one negative case per type. Each judge is handed the answer a deliberately wrong
//! component gives -- a delivery reported to nowhere, an account authorized without credentials,
//! a trap -- and has to fail it; the answers a correct plugin gives pass. The real components
//! answer the same questions in CI's conformance run over every bundled package.

use rd_core::{Failure, FailureKind};

use super::*;
use crate::extension::StepWarning;

fn trap() -> anyhow::Error {
    anyhow::Error::new(wasmtime::Trap::UnreachableCodeReached)
}

fn refusal() -> anyhow::Error {
    anyhow::Error::new(Failure::coded(
        FailureKind::Permanent,
        "demo.refused",
        "refused",
    ))
}

fn proposal(url: &str) -> IntakeProposal {
    IntakeProposal {
        url: url.to_owned(),
        file_name: None,
        size: None,
        package_hint: None,
    }
}

#[test]
fn a_notifier_that_delivers_to_an_empty_destination_fails() {
    assert!(judge_refusal(Err(refusal())).is_ok());
    let wrong = judge_refusal(Ok(())).expect_err("delivered to nowhere");
    assert!(wrong.contains("empty destination"), "{wrong}");
    assert!(
        judge_refusal(Err(trap()))
            .expect_err("a trap")
            .contains("trapped")
    );
}

#[test]
fn a_sign_in_that_authorizes_without_credentials_fails() {
    assert!(judge_no_credentials(Err(refusal())).is_ok());
    assert!(
        judge_no_credentials(Ok(AuthProgress::Failed {
            message: "no credentials".to_owned()
        }))
        .is_ok()
    );
    assert!(
        judge_no_credentials(Ok(AuthProgress::Pending {
            retry_after_seconds: 5
        }))
        .is_ok()
    );
    let wrong = judge_no_credentials(Ok(AuthProgress::Authorized)).expect_err("authorized");
    assert!(wrong.contains("without any credentials"), "{wrong}");
    assert!(judge_no_credentials(Err(trap())).is_err());
}

#[test]
fn an_intake_parser_that_proposes_from_unknown_input_fails() {
    assert!(judge_unknown_input(Ok(Vec::new())).is_ok());
    assert!(judge_unknown_input(Err(refusal())).is_ok());
    let wrong = judge_unknown_input(Ok(vec![proposal("https://example.org/a")]))
        .expect_err("a candidate from nothing");
    assert!(wrong.contains("https://example.org/a"), "{wrong}");
    assert!(judge_unknown_input(Err(trap())).is_err());
}

#[test]
fn an_enricher_that_fails_or_invents_on_empty_input_fails() {
    assert!(judge_no_fields(Ok(Vec::new())).is_ok());
    assert!(judge_no_fields(Err(refusal())).is_err());
    let wrong = judge_no_fields(Ok(vec![("title".to_owned(), "x".to_owned())]))
        .expect_err("a field for nothing");
    assert!(wrong.contains("`title`"), "{wrong}");
}

#[test]
fn a_step_that_fails_or_warns_on_an_empty_package_fails() {
    assert!(judge_empty_package(Ok(StepOutcome::Skipped)).is_ok());
    assert!(
        judge_empty_package(Ok(StepOutcome::Complete {
            checkpoint: None,
            warnings: Vec::new()
        }))
        .is_ok()
    );
    let failed = judge_empty_package(Ok(StepOutcome::Failed {
        message: "no files".to_owned(),
    }))
    .expect_err("failed");
    assert!(failed.contains("no files"), "{failed}");
    let warned = judge_empty_package(Ok(StepOutcome::Complete {
        checkpoint: None,
        warnings: vec![StepWarning {
            code: "demo.unchecked".to_owned(),
            params: Vec::new(),
            message: "unchecked".to_owned(),
        }],
    }))
    .expect_err("warned");
    assert!(warned.contains("demo.unchecked"), "{warned}");
    assert!(
        judge_empty_package(Ok(StepOutcome::Stopped {
            checkpoint: Vec::new()
        }))
        .is_err()
    );
    assert!(judge_empty_package(Err(trap())).is_err());
}

#[test]
fn a_destination_that_confirms_an_unreached_upload_fails() {
    assert!(judge_unreached_verify(Ok(false)).is_ok());
    assert!(judge_unreached_verify(Err(refusal())).is_ok());
    let wrong = judge_unreached_verify(Ok(true)).expect_err("confirmed");
    assert!(wrong.contains(UNREACHED_OBJECT), "{wrong}");
    assert!(judge_unreached_verify(Err(trap())).is_err());
}

#[test]
fn a_backend_that_answers_for_an_undeclared_scheme_fails() {
    let refused = Failure::coded(FailureKind::Permanent, "demo.bad_url", "bad address");
    assert!(judge_undeclared_scheme(Err(refused)).is_ok());
    let wrong = judge_undeclared_scheme(Ok(RemoteFile {
        size: Some(1),
        last_modified: None,
        resumable: true,
    }))
    .expect_err("a file at a foreign scheme");
    assert!(wrong.contains(UNDECLARED_SCHEME), "{wrong}");
    let trapped = Failure::coded(FailureKind::Permanent, "plugin.trapped", "crashed");
    assert!(judge_undeclared_scheme(Err(trapped)).is_err());
}

#[test]
fn a_remote_job_plugin_off_its_declared_containers_fails() {
    let torrent_only = vec!["torrent".to_owned()];
    let consistent = vec![("torrent", Ok(true)), ("nzb", Ok(false))];
    assert!(judge_containers(&torrent_only, consistent).is_ok());
    assert!(judge_containers(&[], vec![("torrent", Ok(false)), ("nzb", Ok(false))]).is_ok());

    let claims_more = vec![("torrent", Ok(true)), ("nzb", Ok(true))];
    let wrong = judge_containers(&torrent_only, claims_more).expect_err("claims an nzb");
    assert!(
        wrong.contains("claims a nzb container it does not declare"),
        "{wrong}"
    );

    let claims_less = vec![("torrent", Ok(false)), ("nzb", Ok(false))];
    let wrong = judge_containers(&torrent_only, claims_less).expect_err("claims no torrent");
    assert!(wrong.contains("declares torrent"), "{wrong}");

    assert!(judge_containers(&[], vec![("torrent", Err(trap()))]).is_err());
}

#[test]
fn a_stream_transform_that_claims_or_fails_on_the_empty_address_fails() {
    assert!(judge_unclaimed(Ok(false)).is_ok());
    assert!(judge_unclaimed(Ok(true)).is_err());
    assert!(judge_unclaimed(Err(refusal())).is_err());
}

/// The samples are what the bundled plugins' own readers recognise: a bencoded dictionary
/// with an `info` key, and a document whose root element is `<nzb`.
#[test]
fn the_container_samples_have_their_formats_shape() {
    assert!(TORRENT.starts_with(b"d4:info") && TORRENT.ends_with(b"ee"));
    let pieces = TORRENT
        .windows(9)
        .position(|window| window == b"pieces20:")
        .expect("pieces");
    assert_eq!(TORRENT.len() - (pieces + 9) - 2, 20, "20 piece bytes");
    assert!(NZB.windows(4).any(|window| window == b"<nzb"));
    assert!(NZB.len() > 16 && TORRENT.len() > 16);
}
