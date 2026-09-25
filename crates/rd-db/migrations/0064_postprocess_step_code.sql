-- RD-107-04: a post-processing step records a stable code beside its English message, so an
-- outcome like "the recovery set is too small" can be said in every language instead of only
-- in the server's own words. Both columns are NULL for every step written before this.
ALTER TABLE postprocess_steps ADD COLUMN code TEXT;
ALTER TABLE postprocess_steps ADD COLUMN params_json TEXT;
