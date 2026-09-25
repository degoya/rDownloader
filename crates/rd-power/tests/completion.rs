//! The completion cycle runs its action exactly once per drained work cycle, and a power
//! action is approved, counted down and cancellable — all against a fake adapter, so no
//! test ever suspends the machine it runs on.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use chrono::{Duration, TimeZone, Utc};
use chrono_tz::Tz;
use rd_power::{
    CompletionAction, PowerAdapter, PowerCapabilities, PowerService, PowerSettings, PowerState,
};

#[derive(Debug, Default)]
struct FakeAdapter {
    capabilities: PowerCapabilities,
    state: PowerState,
    standby_calls: AtomicUsize,
    shutdown_calls: AtomicUsize,
}

#[async_trait]
impl PowerAdapter for FakeAdapter {
    fn capabilities(&self) -> PowerCapabilities {
        self.capabilities
    }

    async fn state(&self) -> PowerState {
        self.state
    }

    async fn standby(&self) -> anyhow::Result<()> {
        self.standby_calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        self.shutdown_calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

fn capable() -> Arc<FakeAdapter> {
    Arc::new(FakeAdapter {
        capabilities: PowerCapabilities {
            standby: true,
            shutdown: true,
            battery: true,
            metered: true,
            inhibit_standby: true,
            inhibit_display: true,
        },
        ..FakeAdapter::default()
    })
}

fn settings(action: CompletionAction, approved: bool) -> PowerSettings {
    PowerSettings {
        completion_action: action,
        power_actions_allowed: approved,
        completion_countdown_seconds: 60,
        ..PowerSettings::default()
    }
}

#[tokio::test]
async fn a_script_action_fires_once_per_work_cycle() {
    let service = PowerService::new(capable());
    service
        .apply(settings(CompletionAction::Script, false), Tz::UTC)
        .await;
    let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();

    // Idle before anything happened: nothing to complete.
    assert!(service.observe(false, now).await.is_none());
    // A cycle of work, then the queue drains.
    assert!(service.observe(true, now).await.is_none());
    let due = service.observe(false, now).await.expect("action is due");
    assert_eq!(due.action, CompletionAction::Script);
    // Staying idle must not fire it again.
    assert!(service.observe(false, now).await.is_none());
    assert!(service.observe(false, now).await.is_none());

    // A second cycle earns a second run.
    assert!(service.observe(true, now).await.is_none());
    assert!(service.observe(false, now).await.is_some());
}

#[tokio::test]
async fn a_power_action_waits_out_its_countdown_and_can_be_cancelled() {
    let service = PowerService::new(capable());
    service
        .apply(settings(CompletionAction::Shutdown, true), Tz::UTC)
        .await;
    let start = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
    service.observe(true, start).await;

    // The countdown starts; nothing is due yet.
    assert!(service.observe(false, start).await.is_none());
    let status = service.status(start).await;
    let pending = status.pending.expect("countdown running");
    assert_eq!(pending.action, CompletionAction::Shutdown);
    assert_eq!(pending.runs_at, start + Duration::seconds(60));
    assert!(
        service
            .observe(false, start + Duration::seconds(30))
            .await
            .is_none()
    );

    assert!(service.cancel().await);
    assert!(service.status(start).await.pending.is_none());
    // Cancelling settles the cycle: it must not start counting down again.
    assert!(
        service
            .observe(false, start + Duration::seconds(120))
            .await
            .is_none()
    );
    assert!(!service.cancel().await);
}

#[tokio::test]
async fn a_countdown_that_runs_out_hands_the_action_over_exactly_once() {
    let service = PowerService::new(capable());
    service
        .apply(settings(CompletionAction::Standby, true), Tz::UTC)
        .await;
    let start = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
    service.observe(true, start).await;
    assert!(service.observe(false, start).await.is_none());
    let due = service
        .observe(false, start + Duration::seconds(61))
        .await
        .expect("countdown elapsed");
    assert_eq!(due.action, CompletionAction::Standby);
    assert!(
        service
            .observe(false, start + Duration::seconds(120))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn new_work_during_the_countdown_calls_the_action_off() {
    let service = PowerService::new(capable());
    service
        .apply(settings(CompletionAction::Shutdown, true), Tz::UTC)
        .await;
    let start = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
    service.observe(true, start).await;
    service.observe(false, start).await;
    assert!(service.status(start).await.pending.is_some());

    // Something was added again — the machine must stay on.
    service.observe(true, start + Duration::seconds(10)).await;
    assert!(service.status(start).await.pending.is_none());
    // The new cycle counts down on its own terms.
    assert!(
        service
            .observe(false, start + Duration::seconds(20))
            .await
            .is_none()
    );
    let pending = service
        .status(start + Duration::seconds(20))
        .await
        .pending
        .expect("second countdown");
    assert_eq!(pending.runs_at, start + Duration::seconds(80));
}

#[tokio::test]
async fn without_the_local_approval_a_power_action_never_runs() {
    let service = PowerService::new(capable());
    service
        .apply(settings(CompletionAction::Shutdown, false), Tz::UTC)
        .await;
    let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
    service.observe(true, now).await;
    assert!(service.observe(false, now).await.is_none());
    let status = service.status(now).await;
    assert!(status.pending.is_none());
    // The UI needs to be able to say why nothing will happen.
    assert!(status.approval_missing);
}

#[tokio::test]
async fn withdrawing_the_approval_stops_a_running_countdown() {
    let service = PowerService::new(capable());
    service
        .apply(settings(CompletionAction::Shutdown, true), Tz::UTC)
        .await;
    let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
    service.observe(true, now).await;
    service.observe(false, now).await;
    assert!(service.status(now).await.pending.is_some());

    service
        .apply(settings(CompletionAction::Shutdown, false), Tz::UTC)
        .await;
    assert!(service.status(now).await.pending.is_none());
}

#[tokio::test]
async fn a_missing_platform_capability_degrades_visibly_without_blocking_anything() {
    let service = PowerService::new(Arc::new(FakeAdapter::default()));
    service
        .apply(settings(CompletionAction::Standby, true), Tz::UTC)
        .await;
    let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
    let status = service.status(now).await;
    assert!(!status.capabilities.standby);
    assert!(!status.capabilities.battery);
    // Quiet hours and the queue are untouched by the missing capability.
    assert!(!status.quiet);
    assert!(status.paused_reason.is_none());
    // Executing it surfaces the platform error instead of pretending it worked.
    let unsupported = PowerService::new(Arc::new(rd_power::UnsupportedAdapter));
    assert!(
        unsupported
            .execute(CompletionAction::Standby)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn the_network_context_pauses_only_what_it_was_asked_to() {
    let adapter = Arc::new(FakeAdapter {
        capabilities: PowerCapabilities {
            battery: true,
            metered: true,
            ..PowerCapabilities::default()
        },
        state: PowerState {
            on_battery: Some(true),
            metered: Some(false),
        },
        ..FakeAdapter::default()
    });
    let service = PowerService::new(adapter);
    service.refresh_state().await;

    service.apply(PowerSettings::default(), Tz::UTC).await;
    assert!(service.hold_reason().await.is_none(), "not configured");

    service
        .apply(
            PowerSettings {
                pause_on_battery: true,
                ..PowerSettings::default()
            },
            Tz::UTC,
        )
        .await;
    service.refresh_state().await;
    assert_eq!(service.hold_reason().await, Some("battery"));

    // A metered policy does not trigger on a connection that reports itself unmetered.
    service
        .apply(
            PowerSettings {
                pause_on_metered: true,
                ..PowerSettings::default()
            },
            Tz::UTC,
        )
        .await;
    service.refresh_state().await;
    assert!(service.hold_reason().await.is_none());
}

#[tokio::test]
async fn quiet_hours_defer_only_the_configured_work() {
    let service = PowerService::new(capable());
    let quiet = rd_limits::QuietHours {
        enabled: true,
        windows: vec![rd_limits::QuietWindow {
            days: rd_limits::DaySet::EVERY_DAY,
            start_minute: 23 * 60,
            end_minute: 7 * 60,
        }],
    };
    service
        .apply(
            PowerSettings {
                quiet_hours: quiet,
                quiet_hours_defer_postprocess: true,
                quiet_hours_defer_notifications: false,
                ..PowerSettings::default()
            },
            Tz::Europe__Berlin,
        )
        .await;
    // 2026-01-16 01:00 Berlin = 00:00 UTC.
    let night = Utc.with_ymd_and_hms(2026, 1, 16, 0, 0, 0).unwrap();
    assert!(service.is_quiet(night).await);
    assert!(service.defers_postprocess(night).await);
    assert!(!service.defers_notifications(night).await, "not configured");

    let day = Utc.with_ymd_and_hms(2026, 1, 16, 11, 0, 0).unwrap();
    assert!(!service.defers_postprocess(day).await);
}
