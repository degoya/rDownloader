//! The sinks a run under the tray reports through: each one an event for the tray's loop. Apart
//! from `tray.rs` so that file keeps to the event loop.

use rd_core::CaptureAgentSettings;
use tao::event_loop::EventLoopProxy;
use tokio::sync::watch;

use super::UserEvent;
use crate::{
    DesktopSinks, activity::Activity, controls, link_phase::LinkStatus, self_update::UpdateMenu,
    server_update, supervision::AgentNotice,
};

/// What the agent is handed in a run under the tray: every sink leads to `proxy`, the requests of
/// the menu and the shortcuts arrive in `inbox`, and the link goes out through `link`.
pub(super) fn desktop(
    proxy: &EventLoopProxy<UserEvent>,
    inbox: controls::Inbox,
    link: watch::Sender<LinkStatus>,
) -> DesktopSinks {
    let activity_proxy = proxy.clone();
    let notice_proxy = proxy.clone();
    let settings_proxy = proxy.clone();
    let update_proxy = proxy.clone();
    let server_update_proxy = proxy.clone();
    DesktopSinks {
        activity: std::sync::Arc::new(move |activity: Activity| {
            let _ = activity_proxy.send_event(UserEvent::Transfers(activity));
        }),
        notice: std::sync::Arc::new(move |notice: AgentNotice| {
            let _ = notice_proxy.send_event(UserEvent::Notice(notice));
        }),
        settings: std::sync::Arc::new(move |settings: CaptureAgentSettings| {
            let _ = settings_proxy.send_event(UserEvent::Settings(Box::new(settings)));
        }),
        inbox,
        update: std::sync::Arc::new(move |menu: UpdateMenu| {
            let _ = update_proxy.send_event(UserEvent::SelfUpdate(menu));
        }),
        server_update: std::sync::Arc::new(move |view: server_update::View| {
            let _ = server_update_proxy.send_event(UserEvent::ServerUpdate(view));
        }),
        link,
    }
}
