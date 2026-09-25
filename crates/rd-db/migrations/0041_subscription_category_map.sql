-- RD-080-11: mapping an indexer's own categories onto rDownloader categories.
--
-- An indexer category id like `5030` means nothing on its own and means something different
-- on the next indexer, so the mapping belongs to the subscription rather than to a global
-- table. Stored as JSON because it is a small, wholly-replaced list that nothing queries by:
-- the same reasoning as the filter set next to it.
--
-- Existing rows get NULL, which is an empty map — every item then lands in the
-- subscription's own category, exactly as before this migration.
ALTER TABLE subscriptions ADD COLUMN category_map_json TEXT;

-- The category the source assigned to an item, kept raw so a mapping added later applies to
-- items that were already archived.
ALTER TABLE subscription_items ADD COLUMN source_category TEXT;
