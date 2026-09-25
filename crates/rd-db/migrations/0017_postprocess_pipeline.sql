-- SABnzbd-style post-processing: per-package/category level and script, package
-- lifecycle state, ordered steps with progress, plus media metadata columns.
ALTER TABLE packages ADD COLUMN postprocess_level TEXT;
ALTER TABLE packages ADD COLUMN script TEXT;
ALTER TABLE packages ADD COLUMN postprocess_stage TEXT;
ALTER TABLE packages ADD COLUMN postprocess_percent INTEGER;
ALTER TABLE packages ADD COLUMN postprocess_current TEXT;
ALTER TABLE categories ADD COLUMN postprocess_level TEXT;
ALTER TABLE categories ADD COLUMN script TEXT;
ALTER TABLE collector_packages ADD COLUMN postprocess_level TEXT;
ALTER TABLE collector_packages ADD COLUMN script TEXT;
ALTER TABLE postprocess_steps ADD COLUMN position INTEGER NOT NULL DEFAULT 0;
ALTER TABLE postprocess_steps ADD COLUMN progress_percent INTEGER;
ALTER TABLE postprocess_steps ADD COLUMN started_at TEXT;
ALTER TABLE link_candidates ADD COLUMN media_json TEXT;
ALTER TABLE downloads ADD COLUMN media_json TEXT;
-- Packages whose files are all complete were never marked; treat them as finished.
UPDATE packages SET state = 'completed'
 WHERE state = 'queued'
   AND EXISTS (SELECT 1 FROM downloads d WHERE d.package_id = packages.id)
   AND NOT EXISTS (SELECT 1 FROM downloads d WHERE d.package_id = packages.id AND d.state != 'completed');
CREATE INDEX IF NOT EXISTS packages_state_idx ON packages(state);
