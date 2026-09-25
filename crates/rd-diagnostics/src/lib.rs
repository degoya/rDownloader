//! Structured log capture and the diagnostic bundle (RD-110-02).
//!
//! Three things, each in its own module:
//!
//! * [`capture`] — the `tracing` layer that turns an event into a [`rd_db::NewLogRecord`],
//!   redacts it through `rd_core::redact_text` **before** it leaves the layer, and hands it to
//!   a bounded channel without ever blocking the thread that logged.
//! * [`sink`] — the task that drains that channel into the database in batches and keeps the
//!   store within the retention a person configured, deleting in bounded steps so a queue
//!   mutation never waits behind a sweep.
//! * [`bundle`] — the inventory a person approves and the deterministic archive built from it.
//! * [`notes`] — the codes that carry every line a person reads about the bundle, and the
//!   English rendering the archive keeps for a reader outside the application.
//! * [`trace_layer`] and [`otlp`] — the optional trace export (RD-110-03): a second layer
//!   that builds an OTLP span from any span carrying a trace context, and the task that
//!   posts batches of them. Off by default, never blocking, never retrying.
//!
//! A leaf crate on purpose: the binary installs the layer before it knows whether the database
//! will open, so this must not pull the rest of the application in. The system checks the
//! bundle carries are collected in `rd-api`, which already links every crate they need; this
//! crate only knows their shape ([`checks`]).

pub mod bundle;
pub mod capture;
pub mod checks;
pub mod notes;
pub mod otlp;
pub mod sink;
pub mod trace_layer;

pub use bundle::{BuiltBundle, BundleInput, Inventory, InventoryEntry, Manifest};
pub use capture::{CaptureSnapshot, LogCaptureLayer, LogStream, install, snapshot};
pub use checks::{Check, CheckStatus};
pub use notes::Note;
pub use otlp::{ExportSnapshot, SpanRecord, SpanSink, SpanStream};
pub use trace_layer::TraceExportLayer;
