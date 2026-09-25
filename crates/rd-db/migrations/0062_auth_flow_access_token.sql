-- RD-106-03: where the access token of an OAuth account goes when the account's own
-- credential is already taken.
--
-- A provider whose person registers their own application holds two credentials at once: the
-- client secret they typed, which lives in `accounts.secret_ref`, and the access token the
-- sign-in obtained. Until now the sign-in wrote the second over the first, which destroyed
-- the very value the next renewal needs. This column is the second place, beside the refresh
-- material that already lives here and under the same rule: a vault reference, never returned
-- through the API, and dropped with the row.
ALTER TABLE auth_flows ADD COLUMN access_ref TEXT;
