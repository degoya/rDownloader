ALTER TABLE nzb_imports ADD COLUMN category_id TEXT REFERENCES categories(id);
ALTER TABLE nzb_imports ADD COLUMN import_mode TEXT NOT NULL DEFAULT 'review';
ALTER TABLE nzb_imports ADD COLUMN source_path TEXT;
