//! The notification hub's two background loops (RD-050-14).
//!
//! One subscribes to the event bus and turns matching events into queued deliveries; the
//! other works the queue. They are separate so a hanging target can never delay the event
//! stream, and every target gets its own task so one dead endpoint does not block the rest.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use rd_core::{NotificationTargetId, PackageState};
use rd_notify::{
    Attempt, Delivery, DeliveryState, Message, NotificationEvent, NotificationTarget, TargetConfig,
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// How often the delivery queue is swept.
const SWEEP: Duration = Duration::from_secs(5);

/// Newest deliveries returned by the history endpoint by default.
pub const DEFAULT_HISTORY: u32 = 100;

struct Inner {
    database: rd_db::Database,
    secrets: rd_secrets::SecretStore,
    power: rd_power::PowerService,
    http: reqwest::Client,
    /// Where the notification-destination plugins are installed, and the host they reach the
    /// outside world through.
    plugins: rd_plugin_host::PluginInstaller,
    plugin_host: Arc<dyn rd_plugin_api::ResolverHost>,
    /// Compiled on first use rather than at startup. Compiling a component costs a few
    /// milliseconds, an installation with no such target never pays it, and a newly installed
    /// plugin needs a restart before it can be built anyway — the same rule the resolvers and
    /// the intake parsers follow.
    notifiers: tokio::sync::OnceCell<Arc<rd_plugin_ext::NotifierPlugins>>,
    /// Targets currently being delivered to, so one slow endpoint is not attempted twice
    /// in parallel and cannot hold up the others.
    in_flight: Mutex<HashSet<NotificationTargetId>>,
    shutdown: CancellationToken,
}

/// Cloneable handle of the notification hub.
#[derive(Clone)]
pub struct NotificationService {
    inner: Arc<Inner>,
}

impl NotificationService {
    #[must_use]
    pub fn start(
        database: rd_db::Database,
        secrets: rd_secrets::SecretStore,
        power: rd_power::PowerService,
        plugins: rd_plugin_host::PluginInstaller,
        plugin_host: Arc<dyn rd_plugin_api::ResolverHost>,
    ) -> Self {
        let service = Self {
            inner: Arc::new(Inner {
                database,
                secrets,
                power,
                plugins,
                plugin_host,
                notifiers: tokio::sync::OnceCell::new(),
                http: reqwest::Client::builder()
                    .timeout(Duration::from_secs(30))
                    .build()
                    .unwrap_or_default(),
                in_flight: Mutex::new(HashSet::new()),
                shutdown: CancellationToken::new(),
            }),
        };
        tokio::spawn(service.clone().watch_events());
        tokio::spawn(service.clone().deliver_loop());
        service
    }

    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }

    /// The installed notification-destination plugins, compiled on first use.
    ///
    /// A failure to load costs the plugin targets and nothing else: the built-in kinds keep
    /// delivering, which is why this answers with an empty set rather than an error.
    pub async fn notifiers(&self) -> Arc<rd_plugin_ext::NotifierPlugins> {
        Arc::clone(
            self.inner
                .notifiers
                .get_or_init(|| async {
                    match rd_plugin_ext::NotifierPlugins::load(
                        &self.inner.plugins,
                        Some(Arc::clone(&self.inner.plugin_host)),
                    )
                    .await
                    {
                        Ok(plugins) => Arc::new(plugins),
                        Err(error) => {
                            tracing::warn!(%error, "could not load notification destination plugins");
                            Arc::new(rd_plugin_ext::NotifierPlugins::none())
                        }
                    }
                })
                .await,
        )
    }

    /// Delivers through a plugin destination, or reports why it could not.
    ///
    /// Separate from `rd_notify::send` because a plugin runs in the host's sandbox, which
    /// `rd-notify` deliberately knows nothing about. What comes back is the same `Attempt`,
    /// so the retry policy above does not care which transport answered.
    async fn deliver_through_plugin(
        &self,
        target: &NotificationTarget,
        config: &TargetConfig,
        message: &Message,
    ) -> Attempt {
        let Some(plugin_id) = config.plugin_id.as_deref().filter(|id| !id.is_empty()) else {
            return Attempt::could_not_deliver("this destination names no plugin", false);
        };
        // The event name as the contract spells it: the same `snake_case` the REST API and
        // the automation triggers use, so a plugin and a webhook receiver see one vocabulary.
        let event = serde_json::to_string(&message.event).unwrap_or_default();
        let delivery = rd_plugin_host::extension::Delivery {
            title: &message.title,
            body: &message.body,
            event: event.trim_matches('"'),
            severity: severity_name(message.event.severity()),
            idempotency_key: &message.idempotency_key,
            destination: &target.endpoint,
            secret_ref: target.secret_ref.as_deref(),
        };
        match self.notifiers().await.deliver(plugin_id, delivery).await {
            Some(Ok(())) => Attempt::succeeded(),
            Some(Err(error)) => plugin_failure(&error),
            // The plugin was removed or never installed. Retrying cannot fix that, and a
            // target that keeps trying for days hides the actual problem.
            None => Attempt::could_not_deliver(
                format!("no installed notification destination with id {plugin_id}"),
                false,
            ),
        }
    }

    /// Sends one message to a target right now, for the test action.
    pub async fn test_target(&self, target: &NotificationTarget) -> Attempt {
        let config = target_config(target);
        let message = Message {
            title: "rDownloader test".to_owned(),
            body: "This is a test notification from rDownloader.".to_owned(),
            event: NotificationEvent::PackageCompleted,
            idempotency_key: format!("test:{}", target.id),
            payload: serde_json::json!({
                "event": "test",
                "title": "rDownloader test",
                "body": "This is a test notification from rDownloader."
            }),
        };
        if target.kind == rd_notify::TargetKind::Plugin {
            return self.deliver_through_plugin(target, &config, &message).await;
        }
        let secret = self.resolve_secret(target).await;
        let vendor = vendor_directory(&self.inner.database).await;
        rd_notify::send(
            &self.inner.http,
            target,
            &config,
            &message,
            secret.as_ref(),
            vendor.as_deref(),
        )
        .await
    }

    async fn resolve_secret(&self, target: &NotificationTarget) -> Option<secrecy::SecretString> {
        let reference = target.secret_ref.as_deref()?;
        match self.inner.secrets.get(reference).await {
            Ok(secret) => Some(secret),
            Err(error) => {
                tracing::warn!(target = %target.name, %error, "target secret is unreadable");
                None
            }
        }
    }

    /// Turns matching bus events into queued deliveries.
    async fn watch_events(self) {
        let mut events = self.inner.database.subscribe();
        loop {
            let event = tokio::select! {
                () = self.inner.shutdown.cancelled() => return,
                event = events.recv() => match event {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                },
            };
            if let Err(error) = self.handle_event(&event).await {
                tracing::warn!(%error, "notification event could not be queued");
            }
        }
    }

    async fn handle_event(&self, event: &rd_core::EventEnvelope) -> anyhow::Result<()> {
        let Some((kind, category_id, title, body)) = self.classify(event).await? else {
            return Ok(());
        };
        for rule in self.inner.database.list_notification_rules().await? {
            if !rule.matches(kind, category_id) {
                continue;
            }
            self.inner
                .database
                .queue_notification_delivery(rd_db::NewDelivery {
                    rule_id: rule.id,
                    target_id: rule.target_id,
                    // Derived from the rule and the event id, so a replay after a crash
                    // lands on the same key and the unique index drops it.
                    idempotency_key: rd_notify::idempotency_key(rule.id, &event.id.to_string()),
                    event: kind,
                    title: title.clone(),
                    body: body.clone(),
                })
                .await?;
        }
        Ok(())
    }

    /// Maps a bus event onto a notification event, its category and its text.
    #[allow(clippy::type_complexity)]
    async fn classify(
        &self,
        event: &rd_core::EventEnvelope,
    ) -> anyhow::Result<
        Option<(
            NotificationEvent,
            Option<rd_core::CategoryId>,
            String,
            String,
        )>,
    > {
        match event.kind {
            rd_core::EventKind::PackageState => {
                let Some(id) = event.payload.get("package_id").and_then(|v| v.as_str()) else {
                    return Ok(None);
                };
                let Some(package) = self
                    .inner
                    .database
                    .list_packages()
                    .await?
                    .into_iter()
                    .find(|package| package.id.to_string() == id)
                else {
                    return Ok(None);
                };
                let kind = match package.state {
                    PackageState::Completed => NotificationEvent::PackageCompleted,
                    PackageState::Failed => NotificationEvent::PackageFailed,
                    _ => return Ok(None),
                };
                let title = match kind {
                    NotificationEvent::PackageCompleted => {
                        format!("Package finished: {}", package.name)
                    }
                    _ => format!("Package failed: {}", package.name),
                };
                Ok(Some((
                    kind,
                    package.category_id,
                    title,
                    format!("{} ({})", package.name, package.state),
                )))
            }
            rd_core::EventKind::StorageCapacity => Ok(Some((
                NotificationEvent::StorageBlocked,
                None,
                "Storage root out of space".to_owned(),
                "A storage root fell below its free-space threshold and takes no new work."
                    .to_owned(),
            ))),
            rd_core::EventKind::CaptchaChanged => Ok(Some((
                NotificationEvent::CaptchaWaiting,
                None,
                "A captcha is waiting".to_owned(),
                "A download is waiting for a captcha to be answered.".to_owned(),
            ))),
            rd_core::EventKind::PowerChanged => Ok(Some((
                NotificationEvent::PowerPending,
                None,
                "Power action pending".to_owned(),
                "The queue is done and a power action is counting down.".to_owned(),
            ))),
            rd_core::EventKind::BandwidthChanged => Ok(budget_exhausted(&event.payload)),
            _ => Ok(None),
        }
    }

    /// Works the delivery queue.
    async fn deliver_loop(self) {
        let mut ticker = tokio::time::interval(SWEEP);
        loop {
            tokio::select! {
                () = self.inner.shutdown.cancelled() => return,
                _ = ticker.tick() => {
                    if let Err(error) = self.sweep().await {
                        tracing::warn!(%error, "notification delivery sweep failed");
                    }
                }
            }
        }
    }

    async fn sweep(&self) -> anyhow::Result<()> {
        let now = chrono::Utc::now();
        // Quiet hours group deliveries rather than dropping them: the queue keeps filling
        // and is worked off in one go when the window ends.
        if self.inner.power.defers_notifications(now).await {
            return Ok(());
        }
        let due = self.inner.database.due_notification_deliveries(now).await?;
        if due.is_empty() {
            return Ok(());
        }
        let targets: HashMap<_, _> = self
            .inner
            .database
            .list_notification_targets()
            .await?
            .into_iter()
            .map(|target| (target.id, target))
            .collect();
        for delivery in due {
            let Some(target) = targets.get(&delivery.target_id) else {
                continue;
            };
            if !target.enabled {
                continue;
            }
            // One attempt per target at a time: a target that times out must not stall the
            // sweep for every other one.
            if !self.inner.in_flight.lock().await.insert(target.id) {
                continue;
            }
            let service = self.clone();
            let target = target.clone();
            tokio::spawn(async move {
                service.attempt(delivery, target).await;
            });
        }
        Ok(())
    }

    async fn attempt(&self, delivery: Delivery, target: NotificationTarget) {
        let config = target_config(&target);
        let message = Message {
            title: delivery.title.clone(),
            body: delivery.body.clone(),
            event: delivery.event,
            idempotency_key: delivery.idempotency_key.clone(),
            payload: serde_json::json!({
                "event": delivery.event,
                "title": delivery.title,
                "body": delivery.body,
                "idempotency_key": delivery.idempotency_key,
            }),
        };
        let outcome = if target.kind == rd_notify::TargetKind::Plugin {
            self.deliver_through_plugin(&target, &config, &message)
                .await
        } else {
            let secret = self.resolve_secret(&target).await;
            let vendor = vendor_directory(&self.inner.database).await;
            rd_notify::send(
                &self.inner.http,
                &target,
                &config,
                &message,
                secret.as_ref(),
                vendor.as_deref(),
            )
            .await
        };
        self.inner.in_flight.lock().await.remove(&target.id);

        let attempt = delivery.attempt.saturating_add(1);
        let (state, next) = settle(&outcome, attempt, chrono::Utc::now());
        if state == DeliveryState::Failed {
            // Repeated failure is worth surfacing: a target nobody notices is broken is
            // worse than no target at all.
            tracing::warn!(
                target = %target.name,
                attempts = attempt,
                status = ?outcome.status,
                "notification target gave up"
            );
        }
        if let Err(error) = self
            .inner
            .database
            .record_notification_attempt(
                delivery.id,
                state,
                attempt,
                next,
                outcome.status,
                outcome.excerpt,
            )
            .await
        {
            tracing::warn!(%error, "delivery attempt could not be recorded");
        }
    }
}

/// What one attempt leaves the delivery as, and when it is tried next.
///
/// A failure the transport or the plugin calls permanent ends the delivery on the spot; only
/// a retryable one walks the backoff until the attempt limit.
fn settle(
    outcome: &Attempt,
    attempt: u32,
    now: chrono::DateTime<chrono::Utc>,
) -> (DeliveryState, Option<chrono::DateTime<chrono::Utc>>) {
    if outcome.ok {
        (DeliveryState::Delivered, None)
    } else if outcome.retryable {
        match rd_notify::next_attempt_at(attempt, now) {
            Some(next) => (DeliveryState::Retrying, Some(next)),
            None => (DeliveryState::Failed, None),
        }
    } else {
        (DeliveryState::Failed, None)
    }
}

/// A failed plugin delivery as an attempt, retryable only when the plugin says so.
///
/// The plugin reports what kind of failure it was (RD-120-62): a rejected Telegram token or a
/// deleted Discord webhook is `auth_required` or `permanent`, and trying it six times over
/// half an hour only delays the "Failed" the user needs to see. An error that carries no
/// category — a trap, a component that would not instantiate — says nothing about the
/// destination and stays retryable, as every plugin failure was before.
fn plugin_failure(error: &anyhow::Error) -> Attempt {
    let retryable = error
        .downcast_ref::<rd_core::Failure>()
        .is_none_or(|failure| failure.category.is_retryable());
    Attempt::could_not_deliver(error.to_string(), retryable)
}

/// The vendor folder configured under Settings → Tools, which the apprise lookup searches
/// first, the same as every other helper binary (RD-120-62).
///
/// Read per delivery rather than held, so a changed setting applies to the next attempt
/// without a restart. An unreadable setting falls back to the built-in folders.
pub(crate) async fn vendor_directory(database: &rd_db::Database) -> Option<String> {
    match database
        .service_setting_field::<String>("vendor_directory")
        .await
    {
        Ok(vendor) => vendor.filter(|vendor| !vendor.trim().is_empty()),
        Err(error) => {
            tracing::warn!(%error, "vendor directory setting is unreadable");
            None
        }
    }
}

/// The `budget_exhausted` notification for a `bandwidth.changed` event that reports the
/// active profile's traffic budget running out.
///
/// The scheduler announces the transition once, not every sample (`entity: "budget"`,
/// `exhausted: true`); every other `bandwidth.changed` — a profile edit, a schedule save, a
/// profile switch, the budget coming back at the next period — notifies nobody.
#[allow(clippy::type_complexity)]
fn budget_exhausted(
    payload: &serde_json::Value,
) -> Option<(
    NotificationEvent,
    Option<rd_core::CategoryId>,
    String,
    String,
)> {
    if payload.get("entity").and_then(serde_json::Value::as_str) != Some("budget")
        || payload
            .get("exhausted")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
    {
        return None;
    }
    let period = match payload.get("period").and_then(serde_json::Value::as_str) {
        Some("monthly") => "monthly",
        _ => "daily",
    };
    let limit = payload
        .get("limit_bytes")
        .and_then(serde_json::Value::as_u64)
        .map(|bytes| format!(" of {bytes} bytes"))
        .unwrap_or_default();
    Some((
        NotificationEvent::BudgetExhausted,
        None,
        "Traffic budget used up".to_owned(),
        format!(
            "The {period} traffic budget{limit} is used up. Running transfers finish; nothing \
             new starts until the period rolls over."
        ),
    ))
}

/// A target's config, falling back to defaults when the stored JSON does not parse.
fn target_config(target: &NotificationTarget) -> TargetConfig {
    serde_json::from_value(target.config.clone()).unwrap_or_default()
}

/// The severity of an event as the plugin contract spells it.
fn severity_name(severity: rd_notify::Severity) -> &'static str {
    match severity {
        rd_notify::Severity::Info => "info",
        rd_notify::Severity::Warning => "warning",
        rd_notify::Severity::Error => "error",
    }
}

#[cfg(test)]
#[path = "notify_service_tests.rs"]
mod tests;
