-- When a package finished, for the automatic removal of completed packages (RD-094-01).
--
-- `updated_at` cannot stand in for this: renaming a package, changing its priority or a
-- post-processing write all bump it, so a person touching a finished package would restart the
-- removal delay without knowing it.
ALTER TABLE packages ADD COLUMN completed_at TEXT;

-- Packages that are already finished get their last write as an approximation, which is the
-- best available answer and never later than the truth.
UPDATE packages SET completed_at = updated_at WHERE state = 'completed';
