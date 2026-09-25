//! Keeping the machine awake while work is in flight.
//!
//! Against a fake adapter that counts what it hands out and notices when it is dropped, so the
//! rules can be checked without a platform that actually sleeps.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use rd_power::{
    Inhibition, PowerAdapter, PowerCapabilities, PowerService, PowerSettings, PowerState,
};

/// Counts up when handed out and down when dropped, which is exactly what "released" means.
#[derive(Debug)]
struct Token {
    live: Arc<AtomicUsize>,
}

impl Drop for Token {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Debug, Default)]
struct FakeAdapter {
    live: Arc<AtomicUsize>,
    handed_out: Arc<AtomicUsize>,
    with_display: Arc<AtomicUsize>,
    refuse: bool,
    /// Signalled the moment `inhibit` is entered.
    entered: Arc<tokio::sync::Notify>,
    /// Awaited before `inhibit` answers, when a test sets it. The two together stand in for
    /// the platform helper being slow to spawn, without a test sleeping on a wall clock.
    gate: Option<Arc<tokio::sync::Notify>>,
}

#[async_trait]
impl PowerAdapter for FakeAdapter {
    fn capabilities(&self) -> PowerCapabilities {
        PowerCapabilities {
            inhibit_standby: true,
            inhibit_display: true,
            ..PowerCapabilities::default()
        }
    }

    async fn state(&self) -> PowerState {
        PowerState::default()
    }

    async fn standby(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn inhibit(&self, display: bool) -> anyhow::Result<Inhibition> {
        anyhow::ensure!(!self.refuse, "this platform cannot stay awake");
        self.entered.notify_one();
        if let Some(gate) = &self.gate {
            gate.notified().await;
        }
        self.handed_out.fetch_add(1, Ordering::SeqCst);
        if display {
            self.with_display.fetch_add(1, Ordering::SeqCst);
        }
        self.live.fetch_add(1, Ordering::SeqCst);
        Ok(Inhibition::holding(Token {
            live: Arc::clone(&self.live),
        }))
    }
}

fn settings(prevent: bool, display: bool) -> PowerSettings {
    PowerSettings {
        prevent_standby: prevent,
        prevent_display_standby: display,
        ..PowerSettings::default()
    }
}

async fn service(adapter: Arc<FakeAdapter>, settings: PowerSettings) -> PowerService {
    let service = PowerService::new(adapter);
    service.apply(settings, chrono_tz::Tz::UTC).await;
    service
}

#[tokio::test]
async fn the_machine_is_held_awake_while_work_runs_and_released_afterwards() {
    let adapter = Arc::new(FakeAdapter::default());
    let service = service(Arc::clone(&adapter), settings(true, false)).await;

    service.set_inhibited(true).await;
    assert_eq!(adapter.live.load(Ordering::SeqCst), 1, "held while working");

    // Staying busy must not take a second one: this is the steady state during a download.
    service.set_inhibited(true).await;
    assert_eq!(adapter.handed_out.load(Ordering::SeqCst), 1, "taken once");

    service.set_inhibited(false).await;
    assert_eq!(adapter.live.load(Ordering::SeqCst), 0, "released when idle");
}

#[tokio::test]
async fn nothing_is_held_while_the_setting_is_off() {
    let adapter = Arc::new(FakeAdapter::default());
    let service = service(Arc::clone(&adapter), settings(false, false)).await;

    service.set_inhibited(true).await;

    assert_eq!(adapter.handed_out.load(Ordering::SeqCst), 0);
    assert!(!service.status(chrono::Utc::now()).await.inhibiting);
}

#[tokio::test]
async fn switching_the_setting_off_mid_download_lets_the_machine_sleep() {
    let adapter = Arc::new(FakeAdapter::default());
    let service = service(Arc::clone(&adapter), settings(true, false)).await;
    service.set_inhibited(true).await;
    assert_eq!(adapter.live.load(Ordering::SeqCst), 1);

    service
        .apply(settings(false, false), chrono_tz::Tz::UTC)
        .await;
    service.set_inhibited(true).await;

    assert_eq!(
        adapter.live.load(Ordering::SeqCst),
        0,
        "the download is still running, but the wish was withdrawn"
    );
}

#[tokio::test]
async fn asking_for_the_display_too_takes_a_new_inhibition() {
    let adapter = Arc::new(FakeAdapter::default());
    let service = service(Arc::clone(&adapter), settings(true, false)).await;
    service.set_inhibited(true).await;
    assert_eq!(adapter.with_display.load(Ordering::SeqCst), 0);

    // The platform decides what an inhibition covers when it is taken, so the wish changing
    // has to replace it rather than amend it.
    service
        .apply(settings(true, true), chrono_tz::Tz::UTC)
        .await;
    service.set_inhibited(true).await;

    assert_eq!(adapter.handed_out.load(Ordering::SeqCst), 2);
    assert_eq!(adapter.with_display.load(Ordering::SeqCst), 1);
    assert_eq!(
        adapter.live.load(Ordering::SeqCst),
        1,
        "the old one is gone"
    );
}

#[tokio::test]
async fn a_platform_that_refuses_is_not_asked_again_every_tick() {
    let adapter = Arc::new(FakeAdapter {
        refuse: true,
        ..FakeAdapter::default()
    });
    let service = service(Arc::clone(&adapter), settings(true, false)).await;

    service.set_inhibited(true).await;
    service.set_inhibited(true).await;
    service.set_inhibited(true).await;

    assert_eq!(
        adapter.handed_out.load(Ordering::SeqCst),
        0,
        "the fake never succeeds"
    );
    assert!(!service.status(chrono::Utc::now()).await.inhibiting);
}

/// Taking an inhibition spawns a helper process on every real platform. The write guard used
/// to be held across that spawn, so `status`, `hold_reason`, `is_quiet` and
/// `defers_postprocess` — the REST layer and the supervision tick — all queued behind it.
#[tokio::test]
async fn a_status_read_does_not_wait_for_the_platform_helper() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let adapter = Arc::new(FakeAdapter {
        gate: Some(Arc::clone(&gate)),
        ..FakeAdapter::default()
    });
    let service = service(Arc::clone(&adapter), settings(true, false)).await;

    let taking = tokio::spawn({
        let service = service.clone();
        async move { service.set_inhibited(true).await }
    });
    adapter.entered.notified().await;

    assert!(
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            service.status(chrono::Utc::now()),
        )
        .await
        .is_ok(),
        "a status read must answer while the helper is still starting"
    );

    gate.notify_one();
    taking.await.expect("the inhibition task");
    assert_eq!(adapter.live.load(Ordering::SeqCst), 1);
}

/// The race the released lock introduces: the queue can go idle while the helper is starting.
/// Whoever acted last decides, so the inhibition that arrives afterwards is released rather
/// than stored — otherwise a finished queue would keep the machine awake indefinitely.
#[tokio::test]
async fn an_inhibition_that_arrives_after_the_queue_went_idle_is_released() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let adapter = Arc::new(FakeAdapter {
        gate: Some(Arc::clone(&gate)),
        ..FakeAdapter::default()
    });
    let service = service(Arc::clone(&adapter), settings(true, false)).await;

    let taking = tokio::spawn({
        let service = service.clone();
        async move { service.set_inhibited(true).await }
    });
    adapter.entered.notified().await;

    // The last download finishes while the helper is mid-spawn.
    service.set_inhibited(false).await;
    gate.notify_one();
    taking.await.expect("the inhibition task");

    assert_eq!(
        adapter.handed_out.load(Ordering::SeqCst),
        1,
        "the platform was asked, because the queue was busy when the call started"
    );
    assert_eq!(
        adapter.live.load(Ordering::SeqCst),
        0,
        "but it is released again, not stored over the newer decision"
    );
    assert!(!service.status(chrono::Utc::now()).await.inhibiting);
}
