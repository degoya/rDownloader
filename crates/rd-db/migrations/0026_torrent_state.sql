-- Torrent file tree, selection, per-file priorities, tracker edits and the per-torrent
-- seeding override (RD-050-05 .. RD-050-11). Typed JSON blobs following the media_json
-- precedent; NULL for every non-torrent row.
ALTER TABLE link_candidates ADD COLUMN torrent_json TEXT;
ALTER TABLE downloads ADD COLUMN torrent_json TEXT;
