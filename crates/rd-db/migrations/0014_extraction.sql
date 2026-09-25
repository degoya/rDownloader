-- Archive passwords per package / NZB import and owner-agnostic postprocessing checkpoints.
ALTER TABLE packages ADD COLUMN password TEXT;
ALTER TABLE nzb_imports ADD COLUMN password TEXT;

CREATE TABLE postprocess_steps (
    owner_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    source_path TEXT NOT NULL,
    state TEXT NOT NULL,
    output_path TEXT,
    message TEXT,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (owner_id, kind, source_path)
);
INSERT INTO postprocess_steps (owner_id, kind, source_path, state, output_path, message, updated_at)
  SELECT import_id, kind, source_path, state, output_path, message, updated_at FROM nzb_postprocess_steps;
DROP TABLE nzb_postprocess_steps;
CREATE INDEX postprocess_steps_state_idx ON postprocess_steps(owner_id, state, updated_at);
