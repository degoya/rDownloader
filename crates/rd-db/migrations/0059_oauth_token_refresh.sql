-- What an OAuth flow needs beyond a device code: an expiry, and the material to renew (RD-103-00).
--
-- Both columns hang off `auth_flows` rather than a table of their own, because the row is already
-- one-per-account and already survives authorization -- the flow is upserted to `authorized`, not
-- deleted. A second table would have the same key and the same lifetime for no gain.
--
-- Still no token here. `refresh_ref` is a vault reference like `accounts.secret_ref`; the value it
-- names never touches this file. The expiry is the one part kept in the clear, and only because the
-- sweep has to be able to ask whether a flow is due without decrypting a secret to find out.
ALTER TABLE auth_flows ADD COLUMN token_expires_at TEXT NULL;
ALTER TABLE auth_flows ADD COLUMN refresh_ref TEXT NULL;

-- The value the provider echoes back on the redirect, and the only thing tying an arriving
-- callback to the account that started it. The host has to be able to read it, which is why it
-- is not folded into `flow_state`: that blob is the plugin's own bookkeeping and opaque here by
-- contract. Unique, so two flows can never both answer to one callback.
ALTER TABLE auth_flows ADD COLUMN callback_state TEXT NULL;
CREATE UNIQUE INDEX auth_flows_callback_state_idx
    ON auth_flows(callback_state)
    WHERE callback_state IS NOT NULL;

-- The renewal sweep reads authorized rows by expiry, which the poll index (state, next_poll_at)
-- does not answer. Partial, because a row with nothing to renew is never a candidate.
CREATE INDEX auth_flows_refresh_due_idx
    ON auth_flows(state, token_expires_at)
    WHERE refresh_ref IS NOT NULL;
