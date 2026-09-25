ALTER TABLE nzb_files ADD COLUMN output_path TEXT;

CREATE TABLE nzb_postprocess_steps (
    import_id TEXT NOT NULL REFERENCES nzb_imports(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    source_path TEXT NOT NULL,
    state TEXT NOT NULL,
    output_path TEXT,
    message TEXT,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(import_id, kind, source_path)
);

CREATE INDEX nzb_postprocess_state_idx
    ON nzb_postprocess_steps(import_id, state, updated_at);
