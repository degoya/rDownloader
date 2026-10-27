//! The agent's ordinary exit code, told apart from a failure (`main.rs`), and `link`.

use super::{PortBusy, commands, config, link_phase, report};

#[test]
fn a_taken_click_n_load_port_reports_its_own_exit_code() {
    let error = anyhow::Error::new(PortBusy {
        addresses: vec!["127.0.0.1:9666".parse().expect("valid address")],
    });
    assert_eq!(report(&error), config::EXIT_PORT_BUSY);
}

#[test]
fn the_ordinary_state_does_not_share_a_code_with_a_real_failure() {
    // The launchers tell these apart by number alone, so a collision would silently turn
    // "already running" back into "could not be started".
    assert_eq!(report(&anyhow::anyhow!("disk on fire")), 1);
    assert_ne!(config::EXIT_PORT_BUSY, 1);
}

#[test]
fn a_taken_port_is_still_recognised_through_added_context() {
    // `run` is called through layers that add context; the marker has to survive that,
    // otherwise the code silently degrades to a plain failure.
    let error = anyhow::Error::new(PortBusy {
        addresses: vec!["127.0.0.1:9666".parse().expect("valid address")],
    })
    .context("start the capture agent");
    assert_eq!(report(&error), config::EXIT_PORT_BUSY);
}

#[test]
fn a_taken_port_names_the_addresses_it_tried() {
    let busy = PortBusy {
        addresses: vec![
            "127.0.0.1:9666".parse().expect("valid address"),
            "[::1]:9666".parse().expect("valid address"),
        ],
    };
    assert_eq!(
        busy.to_string(),
        "Click'n'Load could not bind any of 127.0.0.1:9666, [::1]:9666"
    );
}

/// A fresh directory of this test's own, so nothing reaches the real configuration.
fn scratch(name: &str) -> std::path::PathBuf {
    let directory =
        std::env::temp_dir().join(format!("rd-capture-main-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("create the scratch directory");
    directory
}

/// RD-1390-02: without a running agent there is no page, and `link` says what to do, ending on 1.
#[test]
fn link_without_a_running_agent_says_so() {
    let directory = scratch("link-none");
    let error = commands::running_page(&directory).expect_err("no agent runs");
    assert_eq!(
        error.to_string(),
        "The agent is not running; start it first (rdownloader-capture run)"
    );
    assert_eq!(report(&error), 1);
    std::fs::remove_dir_all(&directory).expect("clean up");
}

/// `link --print` prints what the running agent wrote to `link.key`.
#[test]
fn link_print_prints_the_page_url() {
    let directory = scratch("link-print");
    let page = "http://127.0.0.1:9666/link/0123456789abcdef";
    std::fs::write(
        directory.join(link_phase::files::LINK_KEY_FILE),
        format!("{page}\n"),
    )
    .expect("write link.key");
    let found = commands::running_page(&directory).expect("an agent runs");
    assert_eq!(found.as_str(), page);
    std::fs::remove_dir_all(&directory).expect("clean up");
}
