-- RD-130-19: a cron expression that replaces a subscription's interval.
--
-- Five fields in the service's local time (`0 6 * * *` is six in the morning wherever the
-- service runs). NULL keeps the interval, which is what every subscription written before
-- this column did. Only a script subscription carries one today; the column is general so
-- that allowing it for another kind later is a validation change and not a migration.
--
-- The script itself needs no column: its name is stored as a `script:<name>` address in
-- `url`, which is NOT NULL and read as a URL everywhere, so the table is not rebuilt.
ALTER TABLE subscriptions ADD COLUMN schedule TEXT NULL;
