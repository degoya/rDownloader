-- Files inside a package are read and now also written in `position` order (RD-104-05).
-- The candidates got their index in 0015; the downloads never had one, so every package
-- listing sorted its files without help from the index.
CREATE INDEX IF NOT EXISTS downloads_package_position_idx ON downloads(package_id, position);
