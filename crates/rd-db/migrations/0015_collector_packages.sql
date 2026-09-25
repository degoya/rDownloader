-- LinkGrabber packages, candidate ordering/check metadata and per-package file order.
CREATE TABLE collector_packages (
    id TEXT PRIMARY KEY NOT NULL,
    batch_id TEXT NOT NULL REFERENCES collector_batches(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    auto_named INTEGER NOT NULL DEFAULT 1,
    category_id TEXT REFERENCES categories(id),
    priority INTEGER NOT NULL DEFAULT 0,
    position INTEGER NOT NULL DEFAULT 0,
    password TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX collector_packages_position_idx ON collector_packages(position);
ALTER TABLE link_candidates ADD COLUMN package_id TEXT REFERENCES collector_packages(id) ON DELETE SET NULL;
ALTER TABLE link_candidates ADD COLUMN position INTEGER NOT NULL DEFAULT 0;
ALTER TABLE link_candidates ADD COLUMN checked_at TEXT;
INSERT INTO collector_packages (id, batch_id, name, auto_named, category_id, priority, position, created_at, updated_at)
  SELECT b.id, b.id, COALESCE(b.source_label, 'Paket'), 1,
         (SELECT c.category_id FROM link_candidates c WHERE c.batch_id = b.id LIMIT 1),
         COALESCE((SELECT c.priority FROM link_candidates c WHERE c.batch_id = b.id LIMIT 1), 0),
         b.rowid, b.created_at, b.created_at
  FROM collector_batches b
  WHERE EXISTS (SELECT 1 FROM link_candidates c WHERE c.batch_id = b.id AND c.state != 'enqueued');
UPDATE link_candidates SET package_id = batch_id
  WHERE package_id IS NULL AND EXISTS (SELECT 1 FROM collector_packages p WHERE p.id = link_candidates.batch_id);
UPDATE link_candidates SET position = rowid;
CREATE INDEX link_candidates_package_idx ON link_candidates(package_id, position);
ALTER TABLE downloads ADD COLUMN position INTEGER NOT NULL DEFAULT 0;
UPDATE downloads SET position = rowid;
