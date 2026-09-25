-- Authentication flows a plugin is running for a provider account (RD-090-13).
--
-- One row per account, because a second flow for the same account would be two people signing
-- in to one thing. The row is what makes a flow survive a restart: the sweep loop picks up
-- whatever is due, and the interface reads state rather than driving it.
--
-- No token is here. Whatever a flow produces goes into the vault through the host and is
-- referenced from `accounts.secret_ref` like every other credential.
CREATE TABLE auth_flows (
    account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    plugin_id TEXT NOT NULL,
    state TEXT NOT NULL,
    verification_url TEXT,
    user_code TEXT,
    expires_at TEXT,
    next_poll_at TEXT,
    message TEXT,
    started_at TEXT NOT NULL
);
CREATE INDEX auth_flows_due_idx ON auth_flows(state, next_poll_at);

-- The plugin's own bookkeeping for the next poll — a device code, a PIN check value. Stored
-- verbatim and handed back, because a guest is instantiated fresh for every call and remembers
-- nothing of its own. Not a credential, and never serialised out of the API.
ALTER TABLE auth_flows ADD COLUMN flow_state TEXT NULL;
