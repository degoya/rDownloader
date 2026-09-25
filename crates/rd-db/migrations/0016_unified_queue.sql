-- Usenet imports become packages in the same queue as HTTP downloads.
ALTER TABLE packages ADD COLUMN kind TEXT NOT NULL DEFAULT 'http';
ALTER TABLE packages ADD COLUMN nzb_import_id TEXT REFERENCES nzb_imports(id) ON DELETE SET NULL;
CREATE UNIQUE INDEX packages_nzb_import_idx ON packages(nzb_import_id) WHERE nzb_import_id IS NOT NULL;
ALTER TABLE downloads ADD COLUMN kind TEXT NOT NULL DEFAULT 'http';
ALTER TABLE downloads ADD COLUMN nzb_file_id TEXT REFERENCES nzb_files(id) ON DELETE SET NULL;
CREATE UNIQUE INDEX downloads_nzb_file_idx ON downloads(nzb_file_id) WHERE nzb_file_id IS NOT NULL;

-- Backfill: every import that already left the review stage becomes a package (same id),
-- every NZB file a download row (same id) so postprocess_steps.owner_id stays valid.
INSERT INTO packages (id, name, state, destination, category_id, priority, position, kind, nzb_import_id, password, created_at, updated_at)
  SELECT i.id, i.name,
         CASE i.state WHEN 'completed' THEN 'completed' WHEN 'failed' THEN 'failed' ELSE 'queued' END,
         '', i.category_id, 0, (SELECT COALESCE(MAX(position), 0) FROM packages) + i.rowid,
         'usenet', i.id, i.password, i.created_at, i.updated_at
  FROM nzb_imports i WHERE i.state != 'imported';
INSERT INTO downloads (id, package_id, source_url, file_name, state, total_bytes, committed_bytes, kind, nzb_file_id, position, created_at, updated_at)
  SELECT f.id, f.import_id, 'nzb://' || f.import_id || '/' || f.id,
         COALESCE(f.assembly_name, f.subject),
         CASE WHEN f.output_path IS NOT NULL
                   AND NOT EXISTS (SELECT 1 FROM nzb_segments s WHERE s.file_id = f.id AND s.state != 'completed')
              THEN 'completed' ELSE 'queued' END,
         f.total_bytes,
         (SELECT COALESCE(SUM(s.bytes), 0) FROM nzb_segments s WHERE s.file_id = f.id AND s.state = 'completed'),
         'usenet', f.id, f.ordinal + 1, i.created_at, i.updated_at
  FROM nzb_files f JOIN nzb_imports i ON i.id = f.import_id WHERE i.state != 'imported';
UPDATE nzb_imports SET state = 'enqueued' WHERE state IN ('queued', 'downloading', 'processing', 'completed');
