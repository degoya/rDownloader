-- Which indexer categories a subscription asks for, as a JSON array of source category ids.
--
-- The category map only ever sorted results after they arrived, so a subscription interested in
-- one category still pulled the indexer's whole feed and discarded most of it. This is the other
-- half: what to ask for. NULL and an empty array both mean "everything", which is what every
-- subscription written before this column did, so nothing changes for one that has none.
ALTER TABLE subscriptions ADD COLUMN source_categories_json TEXT;
