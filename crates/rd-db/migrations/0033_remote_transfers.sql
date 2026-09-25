-- FTP/FTPS, SFTP and WebDAV transfer sources (RD-060-01 .. RD-060-03).
-- Credentials live in the secret store; only opaque vault:// references are kept here.
CREATE TABLE remote_credentials (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    protocol TEXT NOT NULL,
    host TEXT NOT NULL,
    port INTEGER NOT NULL,
    username TEXT,
    auth_mode TEXT NOT NULL,
    passive INTEGER NOT NULL DEFAULT 1,
    secret_ref TEXT,
    key_ref TEXT,
    passphrase_ref TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- One credential per endpoint and user, so auto-matching never has to break a tie between
-- two logins that cover exactly the same server. A NULL username is the catch-all for the
-- endpoint and collates as the empty string here.
CREATE UNIQUE INDEX remote_credentials_endpoint_idx
    ON remote_credentials(protocol, host, port, IFNULL(username, ''));
CREATE INDEX remote_credentials_host_idx ON remote_credentials(host, port);

-- SSH host keys a person confirmed once. Without this an SFTP client either refuses every
-- server or accepts every server, and the second option is indistinguishable from an
-- active man in the middle.
CREATE TABLE ssh_known_hosts (
    host TEXT NOT NULL,
    port INTEGER NOT NULL,
    algorithm TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    first_seen TEXT NOT NULL,
    PRIMARY KEY (host, port, algorithm)
);

-- Reviewed remote directory listing, typed JSON following the torrent_json precedent;
-- NULL for every link that is not an ftp/sftp/webdav source.
ALTER TABLE link_candidates ADD COLUMN listing_json TEXT;
ALTER TABLE link_candidates ADD COLUMN remote_credential_id TEXT;

-- Which stored login a queue row authenticates with; NULL matches by host and port at
-- transfer time, so a rotated credential keeps working.
ALTER TABLE downloads ADD COLUMN remote_credential_id TEXT;
CREATE INDEX downloads_remote_credential_idx ON downloads(remote_credential_id);
