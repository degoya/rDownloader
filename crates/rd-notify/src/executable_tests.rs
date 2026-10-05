use serde_json::json;

use super::{
    EXECUTABLE_SEAL, ExecutableNeedsAdmin, check_executable, runnable_executable, seal_executable,
};
use crate::{NotificationTarget, TargetConfig, TargetKind};

const REFERENCE: &str = "vault:0192f0c4-0000-7000-8000-000000000001";

fn target(config: serde_json::Value, secret_ref: Option<&str>) -> NotificationTarget {
    NotificationTarget {
        id: rd_core::NotificationTargetId::new(),
        name: "apprise".to_owned(),
        kind: TargetKind::Apprise,
        enabled: true,
        endpoint: "tgram".to_owned(),
        config,
        secret_ref: secret_ref.map(str::to_owned),
        has_secret: secret_ref.is_some(),
    }
}

/// What delivery would start for `config` stored under `secret_ref`.
fn runnable(config: &serde_json::Value, secret_ref: Option<&str>) -> Result<Option<String>, ()> {
    let parsed: TargetConfig = serde_json::from_value(config.clone()).unwrap_or_default();
    runnable_executable(&target(config.clone(), secret_ref), &parsed)
        .map(|path| path.map(str::to_owned))
        .map_err(|_| ())
}

/// An administrator's save, as the handler performs it.
fn saved_by_admin(path: &str, secret_ref: Option<&str>) -> serde_json::Value {
    let mut config = json!({ "executable": path });
    let approved = check_executable(&config, None, true).expect("an administrator may");
    seal_executable(&mut config, approved, secret_ref);
    config
}

#[test]
fn a_configuration_token_cannot_name_a_program() {
    let config = json!({ "executable": "/usr/bin/evil" });
    assert_eq!(
        check_executable(&config, None, false),
        Err(ExecutableNeedsAdmin)
    );
    let stored = saved_by_admin("/opt/apprise/bin/apprise", Some(REFERENCE));
    assert_eq!(
        check_executable(&config, Some((&stored, Some(REFERENCE))), false),
        Err(ExecutableNeedsAdmin),
        "changing an administrator's path is naming a program too"
    );
}

#[test]
fn an_administrator_s_path_is_sealed_and_runs() {
    let config = saved_by_admin(" /opt/apprise/bin/apprise ", Some(REFERENCE));
    assert!(config[EXECUTABLE_SEAL].is_string(), "{config}");
    assert_eq!(
        runnable(&config, Some(REFERENCE)),
        Ok(Some("/opt/apprise/bin/apprise".to_owned()))
    );
}

#[test]
fn a_path_left_as_it_was_keeps_its_seal_for_a_configuration_token() {
    let stored = saved_by_admin("/opt/apprise/bin/apprise", Some(REFERENCE));
    // The interface sends the whole configuration back, seal included; the handler recomputes
    // the seal either way, here for a replaced apprise URL.
    let mut config = stored.clone();
    let approved = check_executable(&config, Some((&stored, Some(REFERENCE))), false)
        .expect("an unchanged path needs no administrator");
    assert!(approved);
    let replaced = "vault:0192f0c4-0000-7000-8000-000000000002";
    seal_executable(&mut config, approved, Some(replaced));
    assert_eq!(
        runnable(&config, Some(replaced)),
        Ok(Some("/opt/apprise/bin/apprise".to_owned()))
    );
    // Clearing the path narrows what runs, so it needs nothing, and the seal goes with it.
    let mut cleared = json!({ "executable": "", EXECUTABLE_SEAL: config[EXECUTABLE_SEAL] });
    let approved = check_executable(&cleared, Some((&config, Some(replaced))), false)
        .expect("clearing needs no administrator");
    seal_executable(&mut cleared, approved, Some(replaced));
    assert!(cleared.get(EXECUTABLE_SEAL).is_none(), "{cleared}");
}

/// A path stored before the rule — with whatever its author put beside it — never runs, and a
/// configuration token's save does not make it run either. Only an administrator's save does.
#[test]
fn a_path_no_administrator_sealed_never_runs() {
    for planted in [
        json!({ "executable": "/usr/bin/evil" }),
        json!({ "executable": "/usr/bin/evil", EXECUTABLE_SEAL: true }),
        json!({ "executable": "/usr/bin/evil", EXECUTABLE_SEAL: "00".repeat(32) }),
    ] {
        assert_eq!(runnable(&planted, Some(REFERENCE)), Err(()), "{planted}");

        let mut resaved = planted.clone();
        let approved = check_executable(&resaved, Some((&planted, Some(REFERENCE))), false)
            .expect("an unchanged path may be saved again");
        assert!(!approved, "{planted}");
        seal_executable(&mut resaved, approved, Some(REFERENCE));
        assert!(resaved.get(EXECUTABLE_SEAL).is_none(), "{resaved}");
        assert_eq!(runnable(&resaved, Some(REFERENCE)), Err(()), "{resaved}");
    }
    // Without a vault reference there is nothing a seal could be bound to.
    let config = saved_by_admin("/opt/apprise/bin/apprise", None);
    assert!(config.get(EXECUTABLE_SEAL).is_none(), "{config}");
    assert_eq!(runnable(&config, None), Err(()));
    // A seal belongs to its target's reference, not to the path alone.
    let config = saved_by_admin("/opt/apprise/bin/apprise", Some(REFERENCE));
    assert_eq!(
        runnable(&config, Some("vault:0192f0c4-0000-7000-8000-000000000009")),
        Err(())
    );
}

#[test]
fn a_target_without_a_path_uses_the_lookup() {
    assert_eq!(runnable(&json!({}), Some(REFERENCE)), Ok(None));
    assert_eq!(runnable(&json!({ "executable": "  " }), None), Ok(None));
    assert_eq!(check_executable(&json!({}), None, false), Ok(false));
}

/// The delivery itself: an unsealed path is not started, a sealed one is.
#[cfg(unix)]
#[tokio::test]
async fn delivery_starts_only_a_sealed_path() {
    use std::os::unix::fs::PermissionsExt;

    let folder = tempfile::tempdir().expect("tempdir");
    let ran = folder.path().join("ran");
    let script = folder.path().join("not-apprise");
    std::fs::write(&script, format!("#!/bin/sh\ntouch '{}'\n", ran.display())).expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let path = script.to_string_lossy().into_owned();
    let message = crate::Message {
        title: "t".to_owned(),
        body: "b".to_owned(),
        event: crate::NotificationEvent::PackageCompleted,
        idempotency_key: "test:seal".to_owned(),
        payload: json!({}),
    };
    let secret = secrecy::SecretString::from("tgram://token/chat");
    let deliver = |config: serde_json::Value| {
        let message = message.clone();
        let secret = secret.clone();
        async move {
            let parsed: TargetConfig = serde_json::from_value(config.clone()).expect("config");
            crate::send(
                &rd_http::AddressPolicy::new(false),
                &target(config, Some(REFERENCE)),
                &parsed,
                &message,
                Some(&secret),
                None,
            )
            .await
        }
    };

    let attempt = deliver(json!({ "executable": path })).await;
    assert!(!attempt.ok, "{attempt:?}");
    assert!(!ran.exists(), "an unsealed path was started");

    let attempt = deliver(saved_by_admin(&path, Some(REFERENCE))).await;
    assert!(attempt.ok, "{attempt:?}");
    assert!(ran.exists(), "the sealed path was not started");
}
