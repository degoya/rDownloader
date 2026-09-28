-- The LinkGrabber asks, for every address it takes in, whether that address is already in the
-- download list (1.5.0). Without an index that is a scan of the whole table per address.
CREATE INDEX downloads_source_url ON downloads(source_url);
