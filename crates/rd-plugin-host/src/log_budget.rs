//! At most [`MAX_LOG_LINES_PER_CALL`] lines a plugin writes per call (owner, 2026-10-12,
//! RD-1260-12).
//!
//! The service log keeps a bounded number of records for every area together; one chatty plugin
//! pushed everybody else's lines out of it. Each call (one store) gets a budget: the lines the
//! level lets through count, the first fifty are written, the rest are counted and dropped, and
//! when the call ends — the store is dropped — one warning names how many were suppressed, with
//! the plugin's id, version and operation. A line the level filters out costs nothing and does
//! not count. The next call starts at zero.

use tracing::Span;

/// Lines one plugin call may write to the service log.
pub const MAX_LOG_LINES_PER_CALL: u32 = 50;

/// Who a call's lines belong to: the plugin, its version and what was called.
#[derive(Clone, Debug, Default)]
pub(crate) struct LogOrigin {
    pub(crate) plugin_id: String,
    pub(crate) version: String,
    /// The plugin type, as the manifest names it (`resolver`, `notifier`, …).
    pub(crate) kind: String,
    /// The export called, such as `resolve`; the plugin type where one store serves the call
    /// without the host naming the export.
    pub(crate) operation: String,
}

/// One call's budget. Dropped with the store, which is the end of the call.
#[derive(Debug, Default)]
pub(crate) struct LogBudget {
    origin: Option<LogOrigin>,
    written: u32,
    suppressed: u64,
}

impl LogBudget {
    /// Names whose lines these are, for the span around each line and the closing warning.
    pub(crate) fn set_origin(&mut self, origin: LogOrigin) {
        self.origin = Some(origin);
    }

    /// The span a plugin's line is written in, so a debug window can pick one plugin or one
    /// plugin type (`rd_core::PLUGIN_LOG_SPAN`). None for a store that names no plugin.
    pub(crate) fn span(&self) -> Span {
        match &self.origin {
            Some(origin) => tracing::error_span!(
                "plugin_log",
                plugin_id = origin.plugin_id.as_str(),
                kind = origin.kind.as_str()
            ),
            None => Span::none(),
        }
    }

    /// Takes one line the level lets through: `true` while the budget lasts.
    pub(crate) fn admit(&mut self) -> bool {
        if self.written < MAX_LOG_LINES_PER_CALL {
            self.written += 1;
            true
        } else {
            self.suppressed = self.suppressed.saturating_add(1);
            false
        }
    }
}

impl Drop for LogBudget {
    fn drop(&mut self) {
        if self.suppressed == 0 {
            return;
        }
        let origin = self.origin.take().unwrap_or_default();
        tracing::warn!(
            code = "plugin.log_lines_suppressed",
            plugin_id = origin.plugin_id.as_str(),
            plugin_version = origin.version.as_str(),
            operation = origin.operation.as_str(),
            suppressed = self.suppressed,
            "... {} more lines suppressed",
            self.suppressed
        );
    }
}

#[cfg(test)]
#[path = "log_budget_tests.rs"]
mod tests;
