//! The tray's side of "Start at login" (RD-1390-03): the switch runs on the agent's runtime, off
//! the event loop, and the registration it left comes back to tick the entry. The health poll
//! reads the registration again now and then, so the tick follows `rdownloader-capture autostart`
//! run outside the tray. The rules are `autostart_toggle.rs`'s.

use tao::event_loop::EventLoopProxy;

use super::{Agent, UserEvent};
use crate::autostart_toggle;

/// Every how many health readings the registration is read again: half a minute at the poll's
/// five seconds, rather than a `reg.exe` on every tick.
const REFRESH_EVERY: u32 = 6;

impl Agent {
    /// The entry was clicked and left ticked as `wanted`: the registration is brought to that,
    /// never turned the other way when it changed outside the tray.
    pub(super) fn click_autostart(&self, wanted: bool) {
        self.spawn_autostart(Some(wanted));
    }

    /// Its shortcut was pressed: a key press shows no tick, so it switches the registration the
    /// system has. Beside the service it switches nothing; `autostart_toggle::requested` logs
    /// that.
    pub(super) fn switch_autostart(&self) {
        self.spawn_autostart(None);
    }

    fn spawn_autostart(&self, wanted: Option<bool>) {
        let proxy = self.proxy.clone();
        self._runtime.spawn(async move {
            if let Some(registered) = autostart_toggle::requested(wanted).await {
                let _ = proxy.send_event(UserEvent::Autostart(registered));
            }
        });
    }

    /// The switch is done, or failed and left the registration as it was, or the health poll read
    /// it again: the tick says which.
    pub(super) fn on_autostart(&self, registered: bool) {
        if let Some(tray) = self.tray.as_ref() {
            tray.show_autostart(registered);
        }
    }
}

/// Called on every health reading; on every [`REFRESH_EVERY`]th, the first included, the
/// registration is read again and ticks the entry. Nothing beside the service, where it is hidden.
pub(super) async fn refresh(proxy: &EventLoopProxy<UserEvent>, readings: &mut u32) {
    let due = readings.is_multiple_of(REFRESH_EVERY);
    *readings = readings.wrapping_add(1);
    if !due {
        return;
    }
    if let Ok(Some(registered)) = tokio::task::spawn_blocking(autostart_toggle::current).await {
        let _ = proxy.send_event(UserEvent::Autostart(registered));
    }
}
