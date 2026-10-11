//! The tray's health poll: whether the service answers, and which version it names.

use std::time::Instant;

use url::Url;

use super::UserEvent;
use crate::status::{HEALTH_INTERVAL, HealthProbe, HealthWatch, ServerStatus, health_probe};

/// Reports the service's reachability and version to the event loop.
///
/// `/api/v1/health` needs no authentication, so this works before the agent is paired and says
/// nothing about the installation beyond whether it answers and its version. A silence right
/// after launch reads as "starting" rather than "not reachable": both are launched together at
/// login, and the service takes far longer to come up than the tray does.
///
/// Nothing here decides anything: the request is made, its outcome is handed to `health_probe`,
/// and what a run of probes means is [`HealthWatch`]. Both live in `status`, which compiles and
/// is tested on every host; which version an answer names is `client::health_version`, tested
/// the same way (RD-1240-06). This function is only the part that cannot exist without an event
/// loop to send the result to.
pub(super) fn spawn_health_poll(
    runtime: &tokio::runtime::Runtime,
    service: Url,
    proxy: tao::event_loop::EventLoopProxy<UserEvent>,
) {
    runtime.spawn(async move {
        let Ok(health) = service.join("api/v1/health") else {
            let _ = proxy.send_event(UserEvent::Server(ServerStatus::Unreachable, None));
            return;
        };
        // Not `unwrap_or_default()`: that produced a client *without* the deadline it was
        // written for, so the poll hung on its first request, the status line froze on whatever
        // it last said, and no line explained it. A client that cannot be built is a fault, and
        // the state it leaves behind is "not reachable" (RD-109-06).
        let client = match crate::client::build(crate::client::Purpose::Health) {
            Ok(client) => client,
            Err(error) => {
                tracing::error!(
                    %error,
                    "the tray cannot build an HTTP client; the service state stays unknown"
                );
                let _ = proxy.send_event(UserEvent::Server(ServerStatus::Unreachable, None));
                return;
            }
        };
        let started = Instant::now();
        let mut watch = HealthWatch::default();
        loop {
            // The status is the whole of what the rule reads; a transport failure has no status
            // at all, and that absence is what "silence" means one line further down.
            let response = client.get(health.clone()).send().await.ok();
            let probe = health_probe(response.as_ref().map(reqwest::Response::status));
            // The body only of a healthy answer, and only as far as the identity check reads it.
            let version = match response {
                Some(response) if probe == HealthProbe::Healthy => {
                    let body = crate::client::read_health_answer(response).await;
                    crate::client::health_version(&body)
                }
                _ => None,
            };
            let status = watch.observe(probe, started.elapsed());
            // Sending on every tick is fine: the agent ignores a reading it already holds.
            let _ = proxy.send_event(UserEvent::Server(status, version));
            tokio::time::sleep(HEALTH_INTERVAL).await;
        }
    });
}
