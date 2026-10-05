-- RD-1100-01: a package's own download limit, next to the global one and the profiles' scoped
-- limits. A row exists only while the package has a limit; removing the limit removes the row,
-- and removing the package removes it with the package.
CREATE TABLE package_speed_limits (
    package_id TEXT PRIMARY KEY NOT NULL REFERENCES packages(id) ON DELETE CASCADE,
    download_bytes_per_second INTEGER NOT NULL CHECK (download_bytes_per_second > 0),
    updated_at TEXT NOT NULL
);
