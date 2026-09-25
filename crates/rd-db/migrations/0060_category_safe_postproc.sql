-- Per-category override for "a failed verification blocks the unpack" (SABnzbd's
-- `safe_postproc`); NULL inherits the global setting, which is on.
ALTER TABLE categories ADD COLUMN safe_postproc INTEGER NULL;
