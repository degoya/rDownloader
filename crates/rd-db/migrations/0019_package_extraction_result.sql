-- Persisted outcome of the unpack stage ('success' | 'failed'), kept after the
-- post-processing pipeline finished so the UI can show whether a package was extracted.
ALTER TABLE packages ADD COLUMN extraction_result TEXT;
