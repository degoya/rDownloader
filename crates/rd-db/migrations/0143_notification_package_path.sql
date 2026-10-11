-- RD-1250-01: the folder of the finished package a queued notification is about.
--
-- A media server's library refresh scans that folder rather than the whole library, and a
-- delivery that is retried -- after a restart too -- has to name the same folder as its first
-- attempt. Set for `package_completed` only; NULL for every other event and for a delivery
-- queued before this column existed, which then refreshes the whole library as before.
ALTER TABLE notification_deliveries ADD COLUMN package_path TEXT;
