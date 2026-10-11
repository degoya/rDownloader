-- RD-1240-30: a download window in the bandwidth schedule.
--
-- A bandwidth profile may pause downloads while it is in force: no new download starts, and a
-- running transfer that can resume pauses until the profile's window ends.
ALTER TABLE bandwidth_profiles ADD COLUMN pause_downloads INTEGER NOT NULL DEFAULT 0;

-- When a package's files may download, as JSON (`rd_core::DownloadWindow`: the weekly windows
-- and whether the package ignores the schedule's pause). NULL on a package follows its
-- category's; NULL on a category leaves its packages to the bandwidth schedule alone. Read and
-- written as a whole, never queried on its own, like the other JSON columns of both tables.
ALTER TABLE packages ADD COLUMN download_window_json TEXT;
ALTER TABLE categories ADD COLUMN download_window_json TEXT;
