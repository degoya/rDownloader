-- RD-1390-07: what an unpack put into a package's folder, one row per entry it created there (a
-- file or a whole new folder, absolute path). A full package reset deletes exactly these and
-- never an entry that was in the folder before the unpack, which may belong to somebody else.
-- The rows go with the package; a reset that deleted the entries clears them.
CREATE TABLE package_outputs (
    package_id TEXT NOT NULL REFERENCES packages(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    recorded_at TEXT NOT NULL,
    PRIMARY KEY (package_id, path)
);
