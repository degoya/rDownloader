-- Metadata an enricher plugin added to a link (RD-090-14).
--
-- A JSON array of `{name, value, plugin_id, fetched_at}`. Kept beside the core fields rather
-- than merged into them: what a plugin contributed has to stay distinguishable from what the
-- application resolved itself, both so it can be shown with its source and so a core field
-- can never be silently replaced.
ALTER TABLE link_candidates ADD COLUMN enrichment_json TEXT NULL;
