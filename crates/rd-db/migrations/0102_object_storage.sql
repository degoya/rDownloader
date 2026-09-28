-- Object storage profiles and the multipart uploads that outlive a restart (RD-150-04).

-- Where a bucket lives and how to sign for it. Secret values stay in the vault; the row holds
-- only their opaque references. `provider` is open for RD-150-05 (Azure Blob, GCS).
CREATE TABLE object_storage_profiles (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    provider TEXT NOT NULL,
    -- NULL is the provider's own service for the region.
    endpoint TEXT,
    region TEXT,
    -- A bucket the profile is bound to; links into it use this profile.
    bucket TEXT,
    addressing TEXT NOT NULL,
    credential_source TEXT NOT NULL,
    access_key_id TEXT,
    secret_ref TEXT,
    session_token_ref TEXT,
    checksums INTEGER NOT NULL DEFAULT 1,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX object_storage_profiles_name_idx ON object_storage_profiles(name);

-- One multipart upload of one local file, recorded when the service hands out its id so a
-- restart continues it instead of uploading the file again, and so an abandoned one can be
-- aborted rather than left to be billed as stored parts. `completed_at` marks a file whose
-- upload finished while the rest of its package did not.
CREATE TABLE object_uploads (
    id TEXT PRIMARY KEY,
    profile_id TEXT NOT NULL REFERENCES object_storage_profiles(id) ON DELETE CASCADE,
    -- The post-processing owner (the package) the upload belongs to.
    owner TEXT NOT NULL,
    bucket TEXT NOT NULL,
    object_key TEXT NOT NULL,
    local_path TEXT NOT NULL,
    local_size INTEGER NOT NULL,
    -- The local file's modification time; a different one means a different file.
    local_modified TEXT,
    part_size INTEGER NOT NULL,
    -- NULL for a file small enough to go up in one request.
    upload_id TEXT,
    -- Whether the parts carry checksums; the completion has to repeat what the parts said.
    checksums INTEGER NOT NULL,
    completed_at TEXT,
    created_at TEXT NOT NULL,
    UNIQUE (profile_id, bucket, object_key)
);

CREATE INDEX object_uploads_owner_idx ON object_uploads(owner);

-- The parts the service confirmed, with the identifier the completion has to name them by.
CREATE TABLE object_upload_parts (
    upload_id TEXT NOT NULL REFERENCES object_uploads(id) ON DELETE CASCADE,
    part_number INTEGER NOT NULL,
    content_id TEXT NOT NULL,
    size INTEGER NOT NULL,
    PRIMARY KEY (upload_id, part_number)
);
