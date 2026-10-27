//! The tray's side of the link (RD-1390-02): what the agent says about it reaches the event loop,
//! and a link to another service moves the health poll and "Open rDownloader" there.

use tao::event_loop::EventLoopProxy;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::{Agent, UserEvent, health::spawn_health_poll};
use crate::link_phase::LinkStatus;

/// Hands every change of the link to the event loop.
pub(super) fn forward(
    runtime: &tokio::runtime::Runtime,
    mut status: watch::Receiver<LinkStatus>,
    proxy: EventLoopProxy<UserEvent>,
) {
    runtime.spawn(async move {
        while status.changed().await.is_ok() {
            let current = status.borrow_and_update().clone();
            if proxy.send_event(UserEvent::Link(current)).is_err() {
                return;
            }
        }
    });
}

impl Agent {
    /// The agent is linked or not; see `TrayState::on_link`. A link to another service moves the
    /// health poll and the pages "Open" and "About" open.
    pub(super) fn on_link(&mut self, status: &LinkStatus) {
        if let LinkStatus::Linked { service, .. } = status
            && *service != self.service
        {
            self.service = service.clone();
            self.health.cancel();
            self.health = CancellationToken::new();
            spawn_health_poll(
                &self._runtime,
                service.clone(),
                self.proxy.clone(),
                self.health.clone(),
            );
        }
        let update = self.state.on_link(status);
        self.apply(update);
    }
}
