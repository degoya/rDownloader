//! Whether the agent updates itself, and how (RD-1210-03).

use std::path::Path;

use super::*;
use crate::install::fixture::write;

fn folder_with(files: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
    let root = tempfile::tempdir().expect("tempdir");
    let folder = root.path().join("rDownloader");
    for name in files {
        write(&folder.join(name), "file");
    }
    let agent = folder.join(agent_executable());
    write(&agent, "agent");
    (root, agent)
}

/// Two updaters on one folder is what this rule keeps out: beside the service, whose update
/// replaces both programs, the agent offers nothing of its own — whatever else the folder says.
#[test]
fn an_agent_beside_the_service_leaves_its_update_to_the_service() {
    let (_root, agent) = folder_with(&[service_executable(), "VERSION.txt"]);
    assert_eq!(AgentSetup::of(Some(&agent)), AgentSetup::WithService);
    assert_eq!(AgentSetup::WithService.action("9.9.9"), None);
    assert_eq!(AgentSetup::WithService.as_str(), "with_service");
    let (_root, agent) = folder_with(&[service_executable(), "install-kind"]);
    assert_eq!(AgentSetup::of(Some(&agent)), AgentSetup::WithService);
}

#[test]
fn a_portable_agent_alone_installs_its_update_itself() {
    let (_root, agent) = folder_with(&["VERSION.txt", "start-capture.sh"]);
    let setup = AgentSetup::of(Some(&agent));
    assert_eq!(setup, AgentSetup::Alone(InstallKind::Portable));
    assert_eq!(setup.action("1.21.0"), Some(UpdateAction::Install));
}

#[test]
fn homebrews_agent_formula_shows_its_command() {
    let agent =
        Path::new("/opt/homebrew/Cellar/rdownloader-capture/1.20.0/bin/rdownloader-capture");
    let setup = AgentSetup::of(Some(agent));
    assert_eq!(setup, AgentSetup::Alone(InstallKind::Homebrew));
    assert_eq!(
        setup.action("1.21.0"),
        Some(UpdateAction::Command {
            command: "brew upgrade rdownloader-capture".to_owned(),
            hint: None,
        })
    );
}

#[test]
fn an_agent_in_a_folder_nothing_explains_is_offered_the_download() {
    let (_root, agent) = folder_with(&[]);
    let setup = AgentSetup::of(Some(&agent));
    assert_eq!(setup, AgentSetup::Alone(InstallKind::Unknown));
    assert_eq!(setup.action("1.21.0"), Some(UpdateAction::Download));
    assert_eq!(
        AgentSetup::of(None),
        AgentSetup::Alone(InstallKind::Unknown)
    );
}

/// The service's channel when it named one, stable otherwise; stable for a package manager that
/// publishes no pre-releases.
#[test]
fn the_agent_reads_the_services_channel_or_stable() {
    let portable = AgentSetup::Alone(InstallKind::Portable);
    assert_eq!(portable.channel(Some(Channel::Beta)), Channel::Beta);
    assert_eq!(portable.channel(None), Channel::Stable);
    let brewed = AgentSetup::Alone(InstallKind::Homebrew);
    assert_eq!(brewed.channel(Some(Channel::Beta)), Channel::Stable);
}
