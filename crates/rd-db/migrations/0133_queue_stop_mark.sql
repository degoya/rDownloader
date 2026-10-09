-- RD-1210-02: the queue's stop mark. Once its file is done, or every file of its package, the
-- queue pauses until somebody resumes it; what runs at that moment finishes.
--
-- At most one mark: the one row has `slot = 1`, so a new mark replaces the old one in a single
-- statement. It names exactly one target, a file or a package, and goes with it: deleting the
-- file or the package deletes the mark, and moving it in the queue changes nothing here,
-- because the mark follows its target, not a position.
CREATE TABLE queue_stop_mark (
    slot INTEGER PRIMARY KEY NOT NULL CHECK (slot = 1),
    download_id TEXT REFERENCES downloads(id) ON DELETE CASCADE,
    package_id TEXT REFERENCES packages(id) ON DELETE CASCADE,
    set_at TEXT NOT NULL,
    CHECK ((download_id IS NULL) <> (package_id IS NULL))
);
