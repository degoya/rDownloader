-- RD-190-16: how the LinkGrabber's search draws an indexer's hits -- `compact`, one line per hit,
-- or `detailed`, with the metadata the indexer sends and a small cover.
--
-- Defaults to what every indexer showed before the choice existed, so an existing indexer looks
-- exactly as it did without anybody touching it.
ALTER TABLE indexers ADD COLUMN list_style TEXT NOT NULL DEFAULT 'compact';
