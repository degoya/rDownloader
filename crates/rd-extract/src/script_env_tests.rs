//! Security review 2026-09-28, finding 7: nothing of the service's environment reaches a
//! script but the allowlist and the values the service sets itself.
//!
//! A test cannot put a variable into its own process environment - `std::env::set_var` is
//! `unsafe`, and the workspace denies `unsafe_code` - so the test starts its own binary a second
//! time, with the canary set on that child through `Command::env`. The child plays the service:
//! it runs a script through `execute`, and the script writes the environment it received into
//! the package folder, where the parent reads it.

use std::{path::Path, time::Duration};

use crate::script_job::{ScriptContext, execute};

/// Set on the child only; its value is the folder the script runs in.
const CHILD: &str = "RD_TEST_SCRIPT_ENVIRONMENT_CHILD";
/// The secret the service's environment carries and no script may see.
const CANARY: &str = "RD_TEST_SECRET_CANARY";
/// This test's name as the test harness filters it (the module path without the crate).
const NAME: &str = "script_env_tests::a_variable_of_the_service_environment_never_reaches_a_script";

#[tokio::test]
async fn a_variable_of_the_service_environment_never_reaches_a_script() {
    if let Some(folder) = std::env::var_os(CHILD) {
        run_as_the_service(Path::new(&folder)).await;
        return;
    }
    let temp = tempfile::tempdir().expect("tempdir");
    // Not executable, so it runs through `sh`, which is found through the kept `PATH`.
    std::fs::write(temp.path().join("env.sh"), "env > seen.env\n").expect("script");
    let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([NAME, "--exact", "--test-threads=1"])
        .env(CHILD, temp.path())
        .env(CANARY, "canary-4b1f")
        .env("RD_FINAL_DIR", "/inherited/from/the/service")
        .status()
        .expect("start the test binary as the service");
    assert!(status.success(), "the child run failed: {status}");

    let seen = std::fs::read_to_string(temp.path().join("seen.env"))
        .expect("the script ran and wrote its environment");
    let names: Vec<&str> = seen
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name))
        .collect();
    assert!(
        !names.contains(&CANARY),
        "the canary reached the script:\n{seen}"
    );
    assert!(!names.contains(&CHILD), "{seen}");
    assert!(names.contains(&"PATH"), "{seen}");
    // The service's own values arrive, and win over a variable of the same name it inherited.
    assert!(seen.contains("RD_PACKAGE_ID=pkg-env\n"), "{seen}");
    assert!(
        seen.contains(&format!("RD_FINAL_DIR={}\n", temp.path().display())),
        "{seen}"
    );
    assert!(!seen.contains("/inherited/from/the/service"), "{seen}");
}

/// The child's half: the canary is really in this process's environment, and a script
/// started from here is asked what it sees.
async fn run_as_the_service(folder: &Path) {
    assert!(
        std::env::var_os(CANARY).is_some(),
        "the canary is not in the service's environment, so the test would prove nothing"
    );
    let context = ScriptContext {
        package_id: "pkg-env".to_owned(),
        package_name: "Environment".to_owned(),
        final_dir: folder.to_owned(),
        category: None,
        kind: "http".to_owned(),
        status: 0,
    };
    let (ok, output) = execute(
        &folder.join("env.sh"),
        folder,
        &context,
        Duration::from_secs(10),
    )
    .await
    .expect("run the script");
    assert!(ok, "{output}");
}
