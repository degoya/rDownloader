-- RD-107-06: a job that runs at the provider, and the row that makes it survive a restart.
--
-- The durable half of `world remote-job-plugin`. A plugin is instantiated fresh for every call
-- and remembers nothing of its own, so everything that has to last is here: which account and
-- which plugin, what was handed over, what the provider called the job it created, where it
-- has got to, what a person is being asked and what they answered.
-- `docs/adr/0003-a-job-that-runs-at-the-provider.md` argues the split.
CREATE TABLE remote_jobs (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    plugin_id TEXT NOT NULL,
    -- What the plugin's `identify` answered for the source, derived locally and without a
    -- request. It is written before anything is handed to any provider, and the unique index
    -- below is what it is for.
    content_key TEXT NOT NULL,
    -- The provider's own identifier, written the moment `submit` answers and NULL until then.
    -- The window in which this is NULL and `submit_attempts` is not zero is exactly the crash
    -- window the adoption check closes.
    remote_id TEXT,
    state TEXT NOT NULL,
    -- 'magnet' or 'container', and the bytes themselves, so a restart can offer the very same
    -- source again rather than asking the person to paste it a second time.
    source_kind TEXT NOT NULL,
    source BLOB NOT NULL,
    submit_attempts INTEGER NOT NULL DEFAULT 0,
    adoption_checked INTEGER NOT NULL DEFAULT 0,
    -- The LinkGrabber package the finished addresses go to, once there is one.
    package_id TEXT,
    -- What the person is being asked, and what they answered. JSON, because the shape is the
    -- provider's own list of entries and nothing here queries inside it.
    entries TEXT,
    chosen TEXT,
    progress_permille INTEGER,
    message TEXT,
    code TEXT,
    next_poll_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    -- The plugin's own bookkeeping for the next call. Stored verbatim and handed back, like
    -- `auth_flows.flow_state`. Not a credential, and never serialised out of the API.
    job_state TEXT
);

-- The duplicate guard, and the reason this job needed a table at all.
--
-- `torrents/addMagnet` is not idempotent: it answers with a new id every time, so a repeat
-- leaves a second torrent in somebody's account. This index makes a second row for the same
-- content on the same account impossible *before* any network call happens, which is the only
-- place the guarantee can be made -- a plugin remembers nothing between calls and the provider
-- will happily create the duplicate it is asked for.
CREATE UNIQUE INDEX remote_jobs_content_idx ON remote_jobs(account_id, content_key);

-- What the sweep reads: rows in a state that is still asked about, by when they are due.
CREATE INDEX remote_jobs_due_idx ON remote_jobs(state, next_poll_at);
