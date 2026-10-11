-- RD-1240-13: Web Push for the installed app. A browser that turns push on under Settings ›
-- Interface subscribes at its push service and hands the service the subscription; the
-- notification hub then sends to it through a `web_push` target (RFC 8030, encrypted per
-- RFC 8291, signed per RFC 8292).
--
-- The VAPID key pair the service signs with: at most one, `slot = 1`, made the first time a
-- browser asks for it. The private key is a reference into the vault; the public key is the
-- browser's `applicationServerKey`, URL-safe base64 of the uncompressed point, and no secret.
CREATE TABLE web_push_keys (
    slot INTEGER PRIMARY KEY NOT NULL CHECK (slot = 1),
    private_key_ref TEXT NOT NULL,
    public_key TEXT NOT NULL,
    created_at TEXT NOT NULL
);

-- One row per browser. `endpoint` is the push service's address for that browser and names it:
-- a browser that subscribes again with the same address updates its row. `p256dh` and `auth`
-- are the browser's message keys (URL-safe base64), `events_json` the events it wants, empty
-- for every event. A push service that answers 404 or 410 has dropped the subscription, and
-- the row goes with it.
CREATE TABLE web_push_subscriptions (
    id TEXT PRIMARY KEY NOT NULL,
    endpoint TEXT NOT NULL UNIQUE,
    p256dh TEXT NOT NULL,
    auth TEXT NOT NULL,
    device_name TEXT NOT NULL,
    events_json TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
