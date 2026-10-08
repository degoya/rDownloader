-- Whether an object storage profile that signs with the machine's own credentials may send them
-- to its custom endpoint (RD-1190-20). The instance role, the managed identity or the Google
-- token would otherwise go to whatever host the endpoint names; only an explicit yes on the
-- profile lets them. 0 for every profile that names no endpoint or keeps its key in the vault.
ALTER TABLE object_storage_profiles ADD COLUMN ambient_custom_endpoint INTEGER NOT NULL DEFAULT 0;
