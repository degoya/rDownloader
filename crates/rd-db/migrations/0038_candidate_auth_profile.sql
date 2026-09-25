-- RD-080-04: the cookie profile a link uses, chosen before it is queued.
--
-- Downloads already carry this pair (0025_auth_profiles.sql); candidates did not, so the
-- first attempt at a private page always went out with the automatic choice and could only
-- be corrected after it had already failed. Two columns rather than one, for the same
-- reason as on `downloads`: "let the scope decide" and "deliberately send nothing" are
-- different intents and a single nullable id can only express one of them.
--
-- Existing rows get NULL/0, which is `AuthProfileSelection::Auto` — exactly the behaviour
-- every candidate had before this migration.
ALTER TABLE link_candidates ADD COLUMN auth_profile_id TEXT;
ALTER TABLE link_candidates ADD COLUMN auth_profile_pinned INTEGER NOT NULL DEFAULT 0;

CREATE INDEX link_candidates_auth_profile_idx ON link_candidates(auth_profile_id);
