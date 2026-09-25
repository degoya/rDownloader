-- RD-110-38: a link fragment that is key material goes into the vault, not into the row.
--
-- A provider that encrypts on the client puts the file key in the fragment of the address
-- (ADR 0011), and intake strips every fragment before a candidate row exists (RD-109-32).
-- These two columns hold the `vault://` reference the fragment was put away under, never the
-- fragment itself. Ownership moves from the candidate to the download when the link is
-- enqueued, exactly as `replay_body_ref` does.
ALTER TABLE link_candidates ADD COLUMN secret_fragment_ref TEXT;
ALTER TABLE downloads ADD COLUMN secret_fragment_ref TEXT;
