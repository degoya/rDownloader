//! One check of the agent's own update: the service's manifests, the service's key, the agent's
//! archive (RD-1210-03).

use chrono::{DateTime, Utc};
use rd_update::agent::AgentSetup;
use rd_update::{
    Channel, Fetcher, HttpFetcher, InstallKind, Sources, Target, TrustStore, UpdateError,
    newest_agent_offer,
};

use super::State;

/// Checks the official releases now and records what was found in `state`.
///
/// # Errors
///
/// [`UpdateError::NotConfigured`] for a build without the update key; everything else the check
/// meets is recorded in `state` as its `last_error`.
pub(crate) async fn check_now(
    setup: AgentSetup,
    service_channel: Option<Channel>,
    state: &mut State,
    running: &str,
) -> Result<(), UpdateError> {
    let now = Utc::now();
    let trust = rd_update::manifest::release_trust(now)?;
    let channel = setup.channel(service_channel.or(state.service_channel));
    check_with(
        &HttpFetcher::new(),
        (&Sources::official(), &trust),
        channel,
        state,
        (running, now),
    )
    .await;
    Ok(())
}

/// [`check_now`] against explicit sources and trust, so a test serves its own manifests.
///
/// Only a manifest whose signature verifies under `trust` counts, and only a version newer than
/// `running` is offered (`rd_update::newest_agent_offer`); a check where nothing verified keeps
/// the offer it knew and records why.
pub(crate) async fn check_with(
    fetcher: &dyn Fetcher,
    (sources, trust): (&Sources, &TrustStore),
    channel: Channel,
    state: &mut State,
    (running, now): (&str, DateTime<Utc>),
) {
    let report = rd_update::check(fetcher, sources, trust, channel, state.floors, now).await;
    state.floors = report.floors;
    state.last_checked = Some(now);
    state.last_error = report
        .problem
        .as_ref()
        .map(|problem| problem.code().to_owned());
    if !report.manifests.is_empty() {
        state.offer = newest_agent_offer(
            running,
            channel,
            &report.manifests,
            &Target::current(InstallKind::Portable),
        );
    }
    match (&state.offer, &state.last_error) {
        (Some(offer), _) if rd_update::is_newer(&offer.version, running) => {
            tracing::info!(version = %offer.version, "a newer rdownloader-capture is available");
        }
        (_, Some(code)) => tracing::warn!(code, "the agent's update check found a problem"),
        _ => tracing::debug!("rdownloader-capture is up to date"),
    }
}
