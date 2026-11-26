-- RD-1480-11: when a queued notification happened, and the package and the file it is about.
--
-- A webhook's body names them, and a delivery that is retried -- after a restart too -- has to
-- say the same as its first attempt. NULL where an event has no package or no file, and for a
-- delivery queued before these columns existed, which says when it was queued instead.
ALTER TABLE notification_deliveries ADD COLUMN occurred_at TEXT;
ALTER TABLE notification_deliveries ADD COLUMN package_id TEXT;
ALTER TABLE notification_deliveries ADD COLUMN download_id TEXT;
