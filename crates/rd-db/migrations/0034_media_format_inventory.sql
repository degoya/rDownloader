-- Normalised extractor format inventory and the semantic criteria a media link was chosen
-- with (RD-080-01). A typed JSON blob following the torrent_json / listing_json precedent;
-- NULL for every candidate that predates the selector or is not a media link.
--
-- Existing rows are deliberately not rewritten. A media selection stored on a download row
-- carries contract_version 0, which is read as "legacy preset row" and resolved through
-- MediaFormatCriteria::preset(), so `best`, `1080p` and `audio_mp3` keep working with no
-- backfill and no risk of touching rows that are mid-download.
ALTER TABLE link_candidates ADD COLUMN media_formats_json TEXT;
