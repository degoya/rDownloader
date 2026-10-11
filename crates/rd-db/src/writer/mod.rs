//! The serialized writer: the single task every mutation in the process goes through.
//!
//! [`Writer::run`] owns the one write connection and applies commands strictly in the order
//! they were sent. It routes each [`WriterCommand`] to the module that owns the store behind
//! it — the split follows `src/*_store.rs`, so the writer half of `notify_store` is in
//! [`notify`] — and the helpers below are what those modules share.

use anyhow::{Context, Result};
use rd_core::EventEnvelope;
use sqlx::SqliteConnection;
use tokio::sync::{mpsc, oneshot};

use crate::commands::{Reply, WriterCommand};

mod archive_passwords;
mod audit;
mod auth;
mod bandwidth;
mod collector;
mod config;
mod download_rows;
pub(crate) use download_rows::rows_event;
mod downloads;
mod full_backup;
mod history;
mod indexers;
mod logs;
mod maintenance;
mod network;
mod notify;
mod nzb;
mod object_storage;
mod packages;
mod plugin_repositories;
mod plugins;
mod sessions;
mod sources;
mod storage;
mod streams;
mod subscriptions;

pub(crate) struct Writer {
    pub(crate) connection: SqliteConnection,
    commands: mpsc::Receiver<WriterCommand>,
    pub(crate) events: crate::EventBus,
    /// Last broadcast progress event per download (events are throttled, not persisted).
    last_progress: std::collections::HashMap<String, std::time::Instant>,
}

const PROGRESS_EVENT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(750);

impl Writer {
    pub(crate) fn new(
        connection: SqliteConnection,
        commands: mpsc::Receiver<WriterCommand>,
        events: crate::EventBus,
    ) -> Self {
        Self {
            last_progress: std::collections::HashMap::new(),
            connection,
            commands,
            events,
        }
    }

    pub(crate) async fn run(mut self) {
        while let Some(command) = self.commands.recv().await {
            // Exhaustive over `WriterCommand`, and every handler over the enum of its area: a new
            // command does not compile until it is applied, which is what keeps one from being
            // accepted and silently dropped.
            match command {
                WriterCommand::Close { reply } => {
                    let closed = sqlx::Connection::close(self.connection)
                        .await
                        .map_err(anyhow::Error::from);
                    let _ = reply.send(closed);
                    return;
                }
                WriterCommand::Downloads(command) => self.handle_downloads(command).await,
                WriterCommand::Sources(command) => self.handle_sources(command).await,
                WriterCommand::Packages(command) => self.handle_packages(command).await,
                WriterCommand::Collector(command) => self.handle_collector(command).await,
                WriterCommand::Nzb(command) => self.handle_nzb(command).await,
                WriterCommand::Config(command) => self.handle_config(command).await,
                WriterCommand::Network(command) => self.handle_network(command).await,
                WriterCommand::Auth(command) => self.handle_auth(command).await,
                WriterCommand::Sessions(command) => self.handle_sessions(command).await,
                WriterCommand::Plugins(command) => self.handle_plugins(command).await,
                WriterCommand::PluginRepositories(command) => {
                    self.handle_plugin_repositories(command).await
                }
                WriterCommand::ObjectStorage(command) => self.handle_object_storage(command).await,
                WriterCommand::Indexers(command) => self.handle_indexers(command).await,
                WriterCommand::Streams(command) => self.handle_streams(command).await,
                WriterCommand::Subscriptions(command) => self.handle_subscriptions(command).await,
                WriterCommand::Notify(command) => self.handle_notify(command).await,
                WriterCommand::Bandwidth(command) => self.handle_bandwidth(command).await,
                WriterCommand::Maintenance(command) => self.handle_maintenance(command).await,
                WriterCommand::ArchivePasswords(command) => {
                    self.handle_archive_passwords(command).await
                }
                WriterCommand::FullBackup(command) => self.handle_full_backup(command).await,
                WriterCommand::BackupLedger(command) => self.handle_backup_ledger(command).await,
                WriterCommand::Logs(command) => self.handle_logs(command).await,
                WriterCommand::Audit(command) => self.handle_audit(command).await,
                WriterCommand::Storage(command) => self.handle_storage(command).await,
                WriterCommand::History(command) => self.handle_history(command).await,
            }
        }
    }
}

/// How long a persisted event is kept.
///
/// The `events` table is append-only and nothing in the workspace reads it — the live view
/// goes through the broadcast channel, not through here. It is kept because it is the only
/// record of what happened before the process started, and an audit view or a history page
/// is a plausible thing to want; without a sweep it was simply the largest table in the file
/// on any busy install, growing for the life of the service.
const EVENT_RETENTION_DAYS: i64 = 30;

/// Event kinds that are broadcast and never written to `events` (RD-1240-35).
///
/// Both are change notices that tell a client to read again — a Usenet checkpoint, a candidate's
/// online state — and they made 87 % of the table on a live install (822 k `usenet_changed` and
/// 326 k `collector_changed` of 1.33 M rows in 27 days). Nothing reads the table back: the
/// resume of an event stream replays the bus's own buffer (`EventBus`), automations and
/// notifications listen on the bus. So they go the way `DownloadProgress` and the torrent's
/// seeding writes already go: live only. Every other kind is persisted as before.
pub(crate) const BROADCAST_ONLY_EVENT_KINDS: [rd_core::EventKind; 2] = [
    rd_core::EventKind::UsenetChanged,
    rd_core::EventKind::CollectorChanged,
];

/// Rows one event purge removes at most, so a sweep over a month of backlog never holds the
/// writer for long (DB-09).
pub(crate) const EVENT_PURGE_BATCH: i64 = 2_000;

/// Deletes at most [`EVENT_PURGE_BATCH`] events past their retention, oldest first, through
/// `events_occurred_idx`.
async fn purge_old_events(connection: &mut sqlx::SqliteConnection) -> Result<u64> {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(EVENT_RETENTION_DAYS);
    let result = sqlx::query(
        "DELETE FROM events WHERE rowid IN (SELECT rowid FROM events WHERE occurred_at < ? \
         ORDER BY occurred_at LIMIT ?)",
    )
    .bind(cutoff)
    .bind(EVENT_PURGE_BATCH)
    .execute(connection)
    .await
    .context("purge old events")?;
    Ok(result.rows_affected())
}

pub(crate) async fn request<T, C: Into<WriterCommand>>(
    writer: &mpsc::Sender<WriterCommand>,
    make: impl FnOnce(Reply<T>) -> C,
) -> Result<T> {
    let (reply_tx, reply_rx) = oneshot::channel();
    writer
        .send(make(reply_tx).into())
        .await
        .context("database writer stopped")?;
    reply_rx.await.context("database writer dropped response")?
}

fn send<T>(reply: Reply<T>, result: Result<T>) {
    let _ = reply.send(result);
}

fn publish_config<T>(
    reply: Reply<T>,
    result: Result<(T, EventEnvelope)>,
    events: &crate::EventBus,
) {
    if let Ok((_, event)) = &result {
        let _ = events.send(event.clone());
    }
    send(reply, result.map(|(value, _)| value));
}

fn publish_unit_event(reply: Reply<()>, result: Result<EventEnvelope>, events: &crate::EventBus) {
    if let Ok(event) = &result {
        let _ = events.send(event.clone());
    }
    send(reply, result.map(|_| ()));
}

/// Writes `event` to `events` inside the caller's transaction — unless its kind is one of
/// [`BROADCAST_ONLY_EVENT_KINDS`], which the caller still broadcasts after the commit.
pub(crate) async fn insert_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    event: &EventEnvelope,
) -> Result<()> {
    if BROADCAST_ONLY_EVENT_KINDS.contains(&event.kind) {
        return Ok(());
    }
    sqlx::query("INSERT INTO events (id, kind, occurred_at, payload_json) VALUES (?, ?, ?, ?)")
        .bind(event.id.to_string())
        .bind(
            serde_json::to_string(&event.kind)?
                .trim_matches('"')
                .to_owned(),
        )
        .bind(event.occurred_at)
        .bind(serde_json::to_string(&event.payload)?)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
