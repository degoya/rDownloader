-- What the indexer said, and what an enricher found, beyond the candidate row (RD-107-02).
--
-- `link_candidates.source_attributes_json` is the filtered `<newznab:attr>` /
-- `<torznab:attr>` block of the subscription hit this link came from — the same map
-- `subscription_items.attributes_json` holds, written through the same gate
-- (`rd_subscription::retain_attributes`): credentials dropped whole, values redacted, cover
-- addresses limited to absolute http/https, the Newznab `password` reduced to its flag. It
-- exists so an enricher can be asked about a hit without having to guess the title back out
-- of the file name. A manually pasted link has no subscription behind it and leaves the
-- column NULL, which is what "ask without attributes" looks like.
--
-- `packages.enrichment_json` and `downloads.enrichment_json` are the enricher fields carried
-- across the enqueue. Until now they hung on `link_candidates` alone, so a subscription in
-- `auto_queue` mode — where the candidate is gone within seconds — showed a rating for about
-- that long and nothing afterwards. Both columns hold the same `EnrichmentField` list shape
-- the candidate column holds; the package's is the union of its files'.
--
-- Nothing is backfilled. A package created before this migration keeps NULL: its candidate
-- rows are gone, so there is nothing honest to fill it from.
ALTER TABLE link_candidates ADD COLUMN source_attributes_json TEXT;
ALTER TABLE packages ADD COLUMN enrichment_json TEXT;
ALTER TABLE downloads ADD COLUMN enrichment_json TEXT;
