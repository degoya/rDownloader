-- RD-191-12: waits for a hoster's limit and the automatic retry of failed downloads.
--
-- limit_waits: consecutive waits a download sat out because the hoster stated a limit (a rate
-- or daily limit, an IP block). They no longer spend retry_count, which max_retries bounds;
-- this column bounds them instead, and any other outcome sets it back to 0.
-- auto_retry_rounds: how often the automatic retry put the failed download back into the
-- queue; bounded by the auto_retry_max_rounds setting, set back to 0 by a reset.
ALTER TABLE downloads ADD COLUMN limit_waits INTEGER NOT NULL DEFAULT 0;
ALTER TABLE downloads ADD COLUMN auto_retry_rounds INTEGER NOT NULL DEFAULT 0;
