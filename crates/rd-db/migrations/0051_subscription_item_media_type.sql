-- The media type a feed declared for an item's address (RD-095-03).
--
-- An indexer's download address is an API call with no telling extension, so the only thing
-- that says whether it hands over an NZB or a torrent is the `type` the feed states next to
-- it. Without it the link was fetched as an ordinary file and the document landed in the
-- download folder — or, for an indexer that answers an unauthenticated call with an error,
-- did not even do that.
ALTER TABLE subscription_items ADD COLUMN media_type TEXT;
