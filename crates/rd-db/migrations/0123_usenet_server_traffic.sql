-- RD-1100-05: traffic per Usenet server, and an optional quota on each server.
--
-- usenet_server_traffic: the bytes each server delivered, one row per server and UTC day. The
-- download path counts in memory and the counts arrive here in batches, every few seconds, so
-- a row is written per flush and server, never per article. Day rows are what the day, week,
-- month, year and all-time figures are summed from; at one row per server and day the table
-- stays small without a retention sweep. A deleted server takes its rows with it.
--
-- The quota columns on usenet_servers: quota_bytes is the limit (NULL = no quota),
-- quota_action what happens once it is reached ('backup': asked only after every other
-- server; 'pause': not asked at all), quota_reset_on an optional date (YYYY-MM-DD) from which
-- the used figure starts again at zero, once. quota_used_bytes counts since the quota was set
-- or last reset, independently of the statistics, so clearing the statistics leaves it alone.
-- quota_reached_at is set in the flush that crosses the limit, which is what makes the
-- notification fire once per crossing.
CREATE TABLE usenet_server_traffic (
    server_id TEXT NOT NULL REFERENCES usenet_servers(id) ON DELETE CASCADE,
    day TEXT NOT NULL,
    bytes INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (server_id, day)
) WITHOUT ROWID;

ALTER TABLE usenet_servers ADD COLUMN quota_bytes INTEGER;
ALTER TABLE usenet_servers ADD COLUMN quota_action TEXT NOT NULL DEFAULT 'backup';
ALTER TABLE usenet_servers ADD COLUMN quota_reset_on TEXT;
ALTER TABLE usenet_servers ADD COLUMN quota_used_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE usenet_servers ADD COLUMN quota_reached_at TEXT;
