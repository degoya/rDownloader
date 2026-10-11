-- RD-1240-10: an automation can run at a time of day or on an interval. The schedule belongs to
-- the immutable version like the trigger it qualifies: `{"kind":"cron","expression":"0 6 * * *"}`
-- or `{"kind":"interval","minutes":60}`, NULL for every trigger but `schedule`.
--
-- Which slot already ran needs no column of its own: a scheduled run's idempotency key names
-- the automation and the slot's wall-clock time, and `automation_runs.idempotency_key` is
-- unique, so a slot seen again after a restart collides there instead of running twice.
ALTER TABLE automation_versions ADD COLUMN schedule_json TEXT;
