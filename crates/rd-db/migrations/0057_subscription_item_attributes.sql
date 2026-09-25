-- What an indexer said about a hit, and the archive password it announced (RD-101-17).
--
-- `attributes_json` is the filtered `<newznab:attr>` block: cover address, ids, resolution,
-- size, and whatever else the indexer chose to emit. A JSON column rather than typed ones
-- because indexers disagree about which attributes they send, exactly as the parser already
-- documents.
--
-- `password` is kept apart from that map on purpose. It is a secret: it is never serialized
-- to a client and exists only so the extractor can try it before the shared password list.
-- The Newznab `password` attribute is a flag (0/1/2) and stays inside `attributes_json`;
-- only an indexer that writes a real password there fills this column.
ALTER TABLE subscription_items ADD COLUMN attributes_json TEXT;
ALTER TABLE subscription_items ADD COLUMN password TEXT;
