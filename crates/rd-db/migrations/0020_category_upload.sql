-- Per-category rclone upload overrides; NULL inherits the global setting.
ALTER TABLE categories ADD COLUMN upload_enabled INTEGER NULL;
ALTER TABLE categories ADD COLUMN upload_remote TEXT NULL;
