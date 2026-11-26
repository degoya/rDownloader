//! The transports of a delivery: apprise's run, its secret and its output, and what of an answer
//! the history keeps.

use super::excerpt;

/// A hung apprise is killed at its limit and the attempt fails retryably, instead of
/// holding its target `in_flight` for good (audit 1.9.1, INTAKE-05).
#[cfg(unix)]
#[tokio::test]
async fn a_hung_apprise_is_killed_at_its_limit() {
    use std::os::unix::fs::PermissionsExt;

    let folder = tempfile::tempdir().expect("tempdir");
    let script = folder.path().join("apprise");
    std::fs::write(&script, "#!/bin/sh\nexec sleep 30\n").expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let mut command = tokio::process::Command::new(&script);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let secret = secrecy::SecretString::from("tgram://token/chat");
    let started = std::time::Instant::now();
    let attempt = super::run_apprise(command, &secret, std::time::Duration::from_millis(300))
        .await
        .expect("attempt");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert!(!attempt.ok);
    assert!(attempt.retryable);
    assert!(
        attempt
            .excerpt
            .as_deref()
            .is_some_and(|text| text.contains("did not finish")),
        "{attempt:?}"
    );
}

/// Apprise is found in the folder configured under Settings -> Tools (RD-120-62), not only
/// beside the program, in the data folder and on `PATH`.
#[cfg(unix)]
#[tokio::test]
async fn apprise_is_found_in_the_configured_vendor_folder() {
    use std::os::unix::fs::PermissionsExt;

    use super::{Message, TargetConfig, send};
    use crate::model::{NotificationEvent, NotificationTarget, TargetKind};

    let vendor = tempfile::tempdir().expect("tempdir");
    // The stand-in records its arguments, the `APPRISE_URLS` it was handed and its stdin:
    // the real CLI takes the target URL from that variable and reads stdin as the body.
    let received = vendor.path().join("received");
    let arguments = vendor.path().join("arguments");
    let input = vendor.path().join("input");
    let script = vendor.path().join("apprise");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncat > '{}'\nprintf '%s' \"$APPRISE_URLS\" > '{}'\n",
            arguments.display(),
            input.display(),
            received.display(),
        ),
    )
    .expect("write stand-in");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("make it executable");
    let target = NotificationTarget {
        id: rd_core::NotificationTargetId::new(),
        name: "apprise".to_owned(),
        kind: TargetKind::Apprise,
        enabled: true,
        endpoint: "tgram".to_owned(),
        config: serde_json::json!({}),
        secret_ref: Some("vault:apprise".to_owned()),
        has_secret: true,
    };
    let message = Message {
        title: "Package finished".to_owned(),
        body: "example.iso".to_owned(),
        event: NotificationEvent::PackageCompleted,
        idempotency_key: "test:1".to_owned(),
        payload: serde_json::json!({}),
    };
    let secret = secrecy::SecretString::from("tgram://token/chat");
    let reach = rd_http::AddressPolicy::new(false);
    let folder = vendor.path().to_string_lossy().into_owned();

    // Without the setting the stand-in is out of reach, whatever else the machine has.
    send(
        &reach,
        &target,
        &TargetConfig::default(),
        &message,
        Some(&secret),
        None,
    )
    .await;
    assert!(!received.exists(), "the stand-in ran without the setting");

    let attempt = send(
        &reach,
        &target,
        &TargetConfig::default(),
        &message,
        Some(&secret),
        Some(&folder),
    )
    .await;
    assert!(attempt.ok, "{attempt:?}");
    assert_eq!(
        std::fs::read_to_string(&received).expect("the stand-in ran"),
        "tgram://token/chat"
    );
    // The URL is nowhere in argv, where the process list would show it, and stdin — which
    // apprise would send as the body if `--body` were missing — stays empty.
    let argv = std::fs::read_to_string(&arguments).expect("arguments recorded");
    assert!(!argv.contains("token"), "{argv}");
    assert_eq!(
        argv.lines().collect::<Vec<_>>(),
        [
            "--input-format",
            "text",
            "--title",
            "Package finished",
            "--body",
            "example.iso"
        ]
    );
    assert_eq!(std::fs::read_to_string(&input).expect("stdin recorded"), "");
}

/// What apprise prints about a URL it cannot parse contains the URL; the history must not.
#[cfg(unix)]
#[tokio::test]
async fn an_apprise_failure_keeps_the_target_url_out_of_the_history() {
    use std::os::unix::fs::PermissionsExt;

    use super::{Message, TargetConfig, send};
    use crate::model::{NotificationEvent, NotificationTarget, TargetKind};

    let vendor = tempfile::tempdir().expect("tempdir");
    let script = vendor.path().join("apprise");
    std::fs::write(
        &script,
        "#!/bin/sh\necho \"ERROR - Unparseable URL $APPRISE_URLS\" >&2\nexit 1\n",
    )
    .expect("write stand-in");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("make it executable");
    let target = NotificationTarget {
        id: rd_core::NotificationTargetId::new(),
        name: "apprise".to_owned(),
        kind: TargetKind::Apprise,
        enabled: true,
        endpoint: "tgram".to_owned(),
        config: serde_json::json!({}),
        secret_ref: Some("vault:apprise".to_owned()),
        has_secret: true,
    };
    let message = Message {
        title: "Package finished".to_owned(),
        body: "example.iso".to_owned(),
        event: NotificationEvent::PackageCompleted,
        idempotency_key: "test:2".to_owned(),
        payload: serde_json::json!({}),
    };
    let secret = secrecy::SecretString::from("tgram://123456:sekrit/999");
    let folder = vendor.path().to_string_lossy().into_owned();

    let attempt = send(
        &rd_http::AddressPolicy::new(false),
        &target,
        &TargetConfig::default(),
        &message,
        Some(&secret),
        Some(&folder),
    )
    .await;
    assert!(!attempt.ok);
    let excerpt = attempt.excerpt.expect("stderr kept");
    assert!(excerpt.contains("Unparseable URL"), "{excerpt}");
    assert!(!excerpt.contains("sekrit"), "{excerpt}");
}

/// apprise reports most failures on stdout; the history keeps both streams, stderr first,
/// with the target URL redacted (RD-1110-16).
#[cfg(unix)]
#[tokio::test]
async fn an_apprise_failure_keeps_both_output_streams() {
    use std::os::unix::fs::PermissionsExt;

    let folder = tempfile::tempdir().expect("tempdir");
    let script = folder.path().join("apprise");
    let body = "#!/bin/sh\necho 'ERROR - tgram://123456:sekrit/999 refused'\n\
                echo 'WARNING - first' >&2\nexit 1\n";
    std::fs::write(&script, body).expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let mut command = tokio::process::Command::new(&script);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let secret = secrecy::SecretString::from("tgram://123456:sekrit/999");
    let attempt = super::run_apprise(command, &secret, std::time::Duration::from_secs(20))
        .await
        .expect("attempt");
    assert!(!attempt.ok && attempt.retryable, "{attempt:?}");
    let excerpt = attempt.excerpt.expect("output kept");
    let stderr = excerpt.find("WARNING - first").expect("stderr kept");
    let stdout = excerpt.find("refused").expect("stdout kept");
    assert!(stderr < stdout, "{excerpt}");
    assert!(!excerpt.contains("sekrit"), "{excerpt}");
}

#[test]
fn a_response_excerpt_is_truncated_and_redacted() {
    let long = "x".repeat(5_000);
    assert_eq!(excerpt(long).chars().count(), 500);
    let signed = excerpt("failed for https://host/file?X-Amz-Signature=abcdef".to_owned());
    assert!(!signed.contains("abcdef"), "{signed}");
}
