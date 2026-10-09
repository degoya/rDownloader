//! The agent's two ordinary exit codes, told apart from a failure (`main.rs`).

use super::{NotPaired, PortBusy, config, report};

#[test]
fn an_unpaired_agent_reports_its_own_exit_code() {
    let error = anyhow::Error::new(NotPaired);
    assert_eq!(report(&error), config::EXIT_NOT_PAIRED);
}

#[test]
fn a_taken_click_n_load_port_reports_its_own_exit_code() {
    let error = anyhow::Error::new(PortBusy {
        addresses: vec!["127.0.0.1:9666".parse().expect("valid address")],
    });
    assert_eq!(report(&error), config::EXIT_PORT_BUSY);
}

#[test]
fn the_two_ordinary_states_do_not_share_a_code_with_a_real_failure() {
    // The launchers tell these apart by number alone, so a collision would silently turn
    // "already running" back into "could not be started".
    assert_ne!(config::EXIT_NOT_PAIRED, config::EXIT_PORT_BUSY);
    assert_eq!(report(&anyhow::anyhow!("disk on fire")), 1);
    assert_ne!(config::EXIT_NOT_PAIRED, 1);
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
