use super::{MAX_TABLE_ELEMENTS, PluginLimits, SandboxEngine, allowed_imports, timeout_ticks};

fn manifest(plugin_type: &str) -> crate::PluginManifest {
    toml::from_str(&format!(
        r#"
            manifest_version = 3
            plugin_type = "{plugin_type}"
            api_version = "0.10.0"
            id = "11111111-1111-4111-8111-111111111111"
            name = "Demo"
            version = "0.1.0"
            key_id = "demo-v1"
            public_key = "AAAA"
            [metadata]
            description = "d"
            author = "a"
            license = "MIT"
            min_app_version = "0.8.0"
            "#
    ))
    .expect("manifest parses")
}

/// Writing a credential back is bound to the authentication type, not to a capability.
/// A manifest cannot ask for it, so the only way to reach it is to be that type.
#[test]
fn only_an_authentication_plugin_may_write_a_credential() {
    const CREDENTIALS: &str = "rdownloader:plugin/credentials";
    assert!(
        allowed_imports(&manifest("auth")).contains(&CREDENTIALS),
        "an authentication plugin needs it"
    );
    for other in [
        "resolver",
        "transfer",
        "intake",
        "enricher",
        "notifier",
        "postprocess",
        "storage",
    ] {
        assert!(
            !allowed_imports(&manifest(other)).contains(&CREDENTIALS),
            "{other} must not reach the credential store"
        );
    }
}

/// The name a remote job was added under is read by the remote-job type and nobody else.
#[test]
fn only_a_remote_job_plugin_may_read_its_job_context() {
    const JOB_CONTEXT: &str = "rdownloader:plugin/job-context";
    assert!(allowed_imports(&manifest("remote-job")).contains(&JOB_CONTEXT));
    for other in ["resolver", "crawler", "auth", "storage"] {
        assert!(
            !allowed_imports(&manifest(other)).contains(&JOB_CONTEXT),
            "{other} has no job to ask about"
        );
    }
}

/// A target's settings are read by the notifier type and nobody else (RD-170-09).
#[test]
fn only_a_notifier_may_read_its_destination_settings() {
    const SETTINGS: &str = "rdownloader:plugin/destination-settings";
    assert!(allowed_imports(&manifest("notifier")).contains(&SETTINGS));
    for other in ["resolver", "enricher", "remote-job", "storage"] {
        assert!(
            !allowed_imports(&manifest(other)).contains(&SETTINGS),
            "{other} has no target to be set"
        );
    }
}

/// One engine and one epoch ticker for the process, however many sandboxes there are.
#[test]
fn every_sandbox_shares_one_engine() {
    let first = SandboxEngine::new(PluginLimits::default()).expect("sandbox");
    let second = SandboxEngine::new(PluginLimits {
        fuel: 1,
        ..PluginLimits::default()
    })
    .expect("sandbox");
    assert!(wasmtime::Engine::same(first.engine(), second.engine()));
}

#[test]
fn timeout_is_rounded_up_to_epoch_ticks() {
    assert_eq!(timeout_ticks(1), 1);
    assert_eq!(timeout_ticks(10), 1);
    assert_eq!(timeout_ticks(11), 2);
}

#[test]
fn response_budget_is_cumulative() {
    let limits = PluginLimits {
        max_response_bytes: 8,
        ..PluginLimits::default()
    };
    let sandbox = SandboxEngine::new(limits).expect("sandbox");
    let mut store = sandbox
        .create_store(vec!["example.test".to_owned()])
        .expect("store");
    store
        .data_mut()
        .account_response_bytes(5)
        .expect("first response");
    assert!(store.data_mut().account_response_bytes(4).is_err());
}

/// A core module with a one-element funcref table and `grow(n)`, which runs
/// `table.grow 0 (ref.null func) n` and returns the old size (-1 on a refused grow).
const TABLE_GROWER: &[u8] = &[
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // magic, version
    0x01, 0x06, 0x01, 0x60, 0x01, 0x7f, 0x01, 0x7f, // type: (i32) -> i32
    0x03, 0x02, 0x01, 0x00, // function 0 has type 0
    0x04, 0x04, 0x01, 0x70, 0x00, 0x01, // table 0: funcref, min 1, no max
    0x07, 0x08, 0x01, 0x04, b'g', b'r', b'o', b'w', 0x00, 0x00, // export "grow"
    0x0a, 0x0b, 0x01, 0x09, 0x00, // code: one body, no locals
    0xd0, 0x70, 0x20, 0x00, 0xfc, 0x0f, 0x00,
    0x0b, // ref.null func, local.get 0, table.grow 0
];

/// PL-01: table storage does not count against `memory_bytes`, so a plugin's tables are
/// capped on their own. Up to the cap a grow succeeds; one element past it traps instead of
/// taking host memory (without the cap the second grow returned the old size).
#[tokio::test]
async fn a_table_cannot_grow_past_its_element_cap() {
    let sandbox = SandboxEngine::new(PluginLimits::default()).expect("sandbox");
    let module = wasmtime::Module::new(sandbox.engine(), TABLE_GROWER).expect("module");
    let mut store = sandbox.create_store(Vec::new()).expect("store");
    let instance = wasmtime::Instance::new_async(&mut store, &module, &[])
        .await
        .expect("instance");
    let grow = instance
        .get_typed_func::<i32, i32>(&mut store, "grow")
        .expect("grow export");
    let to_cap = i32::try_from(MAX_TABLE_ELEMENTS - 1).expect("cap fits i32");

    assert_eq!(
        grow.call_async(&mut store, to_cap)
            .await
            .expect("grow to the cap"),
        1
    );
    assert!(
        grow.call_async(&mut store, 1).await.is_err(),
        "growing past the cap must trap"
    );
}

/// Time spent inside a host call is waiting, not computing. A resolver's HTTP request was
/// never credited, so a slow hoster burned the same 15s budget as an endless loop — with
/// ten downloads at once several tripped `plugin.timeout` while a retry worked fine.
/// Unlike a hoster countdown this must not touch the wait budget, which exists for
/// guest-requested waits.
#[test]
fn a_host_call_extends_the_deadline_without_spending_the_wait_budget() {
    let limits = PluginLimits {
        timeout_milliseconds: 15_000,
        wait_budget_milliseconds: 60_000,
        ..PluginLimits::default()
    };
    let sandbox = SandboxEngine::new(limits).expect("sandbox");
    let mut store = sandbox.create_store(Vec::new()).expect("store");
    let deadline_before = store.data().execution_deadline;

    store
        .data_mut()
        .credit_host_time(std::time::Duration::from_secs(12));

    assert_eq!(
        store.data().execution_deadline - deadline_before,
        std::time::Duration::from_secs(12),
        "a 12s HTTP wait must buy 12s of extra execution time"
    );
    assert_eq!(
        store.data().wait_budget(),
        std::time::Duration::from_secs(60),
        "host time is not a guest-requested wait and must leave the budget untouched"
    );
}

/// A hoster countdown must come out of the wait budget and buy the same amount of extra
/// execution time, so waiting is never mistaken for a hung plugin — and must run out.
#[test]
fn waiting_spends_the_budget_and_extends_the_execution_deadline() {
    let limits = PluginLimits {
        timeout_milliseconds: 15_000,
        wait_budget_milliseconds: 60_000,
        ..PluginLimits::default()
    };
    let sandbox = SandboxEngine::new(limits).expect("sandbox");
    let mut store = sandbox.create_store(Vec::new()).expect("store");
    let deadline_before = store.data().execution_deadline;

    let granted = store
        .data_mut()
        .claim_wait(std::time::Duration::from_secs(45))
        .expect("45s fits the budget");

    assert_eq!(granted, std::time::Duration::from_secs(45));
    assert_eq!(
        store.data().execution_deadline - deadline_before,
        std::time::Duration::from_secs(45)
    );
    assert_eq!(
        store.data().wait_budget(),
        std::time::Duration::from_secs(15)
    );
    assert!(
        store
            .data_mut()
            .claim_wait(std::time::Duration::from_secs(16))
            .is_none(),
        "a wait beyond the remaining budget must be refused"
    );
}

#[test]
fn component_log_values_are_redacted_after_cookie_access() {
    let sandbox = SandboxEngine::new(PluginLimits::default()).expect("sandbox");
    let mut store = sandbox.create_store(Vec::new()).expect("store");
    store
        .data_mut()
        .remember_redactions(["session-secret".to_owned()]);

    assert_eq!(
        store.data().redact_log("cookie=session-secret"),
        "cookie=[REDACTED]"
    );
}

/// PLUG-18: a secret that crosses the 4096-character cut is masked whole. Cut first, the
/// part before the cut no longer matched and went to the log as it was.
#[test]
fn a_secret_across_the_cut_is_masked_not_halved() {
    let sandbox = SandboxEngine::new(PluginLimits::default()).expect("sandbox");
    let mut store = sandbox.create_store(Vec::new()).expect("store");
    let secret = "s3cr3t-cookie-value";
    store.data_mut().remember_redactions([secret.to_owned()]);
    // The secret starts ten characters before the cut and ends past it.
    let message = format!("{}{secret}{}", "a".repeat(4086), "b".repeat(100));

    let logged = store.data().redact_log(&message);

    assert_eq!(logged.chars().count(), 4096);
    assert!(
        !logged.contains("s3cr3t"),
        "no prefix of the secret survives"
    );
    assert!(logged.ends_with("[REDACTED]"));
    // A line of any length is still cut.
    assert_eq!(
        store
            .data()
            .redact_log(&"x".repeat(100_000))
            .chars()
            .count(),
        4096
    );
}

/// RA-HOST-03: masks shorter than their secrets must not pull text from past the cut in
/// front of it. Two 60-character secrets early in the line shrink by 100 characters; a
/// third secret starting at 4150 lay past the scanned part's end once the line was masked
/// and cut afterwards, and its first characters were logged.
#[test]
fn shrinking_masks_do_not_pull_a_secret_from_past_the_cut() {
    let sandbox = SandboxEngine::new(PluginLimits::default()).expect("sandbox");
    let mut store = sandbox.create_store(Vec::new()).expect("store");
    let first = format!("first-{}", "1".repeat(54));
    let second = format!("second-{}", "2".repeat(53));
    let third = format!("third-{}", "3".repeat(54));
    store
        .data_mut()
        .remember_redactions([first.clone(), second.clone(), third.clone()]);
    let filler = 4150 - first.len() - second.len();
    let message = format!(
        "{first}{second}{}{third}{}",
        "a".repeat(filler),
        "b".repeat(50)
    );
    assert_eq!(message.find(&third), Some(4150));

    let logged = store.data().redact_log(&message);

    assert!(!logged.contains("third"), "nothing past the cut is logged");
    assert!(!logged.contains("first-") && !logged.contains("second-"));
    assert!(logged.starts_with("[REDACTED][REDACTED]a"));
    // The cut is in the line as written: both masks and the filler up to position 4096.
    assert_eq!(logged.chars().count(), 20 + 4096 - 120);
    assert!(!logged.contains('b'));
}

/// RA-HOST-03: a secret that starts before the cut is masked whole however far it reaches,
/// and overlapping secrets are masked as one stretch.
#[test]
fn a_secret_starting_before_the_cut_is_masked_and_overlaps_merge() {
    let sandbox = SandboxEngine::new(PluginLimits::default()).expect("sandbox");
    let mut store = sandbox.create_store(Vec::new()).expect("store");
    store
        .data_mut()
        .remember_redactions(["token-abc".to_owned(), "abc-cookie".to_owned()]);

    assert_eq!(
        store.data().redact_log("x token-abc-cookie y"),
        "x [REDACTED] y"
    );
    let long = format!("tail-{}", "z".repeat(200));
    store.data_mut().remember_redactions([long.clone()]);
    let message = format!("{}{long}", "a".repeat(4095));
    let logged = store.data().redact_log(&message);
    assert!(logged.starts_with(&"a".repeat(4095)));
    assert!(!logged.contains("tail"));
    assert_eq!(
        logged.chars().count(),
        4096,
        "the mask itself is cut at the limit"
    );
}
