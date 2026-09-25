-- Second-factor credentials and the recovery codes that go with them.
--
-- One row per enrolled factor, so a person can hold a TOTP app and (from RD-100-09's WebAuthn
-- half) one or more passkeys at once, name them, and revoke any of them individually. The
-- `kind` column is what distinguishes them; `material_ref` points into the encrypted secret
-- store rather than holding anything itself, the same way accounts and proxies do.
CREATE TABLE mfa_credentials (
    id TEXT PRIMARY KEY NOT NULL,
    -- 'totp' today; 'webauthn' next.
    kind TEXT NOT NULL,
    -- What the user called it, so a list of three passkeys is a list of three named things.
    label TEXT NOT NULL,
    -- `vault://…` reference. The seed never appears in this database.
    material_ref TEXT NOT NULL,
    created_at TEXT NOT NULL,
    -- Set when the first correct code proved the enrolment actually works. An unconfirmed
    -- credential does not gate sign-in: enrolling and then failing to scan the code must not
    -- lock somebody out of their own service.
    confirmed_at TEXT,
    last_used_at TEXT
);

CREATE INDEX mfa_credentials_kind_idx ON mfa_credentials(kind, confirmed_at);

-- Recovery codes, one row each, so spending one is a delete rather than a rewrite of a blob.
--
-- Stored as SHA-256 digests: a recovery code is high-entropy random, so the guessing attack a
-- memory-hard hash defends against is already infeasible, and a cheap hash keeps checking
-- every code costless.
CREATE TABLE mfa_recovery_codes (
    digest TEXT PRIMARY KEY NOT NULL,
    created_at TEXT NOT NULL,
    used_at TEXT
);
