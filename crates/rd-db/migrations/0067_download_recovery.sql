-- PAR2 repair data marked on the download row itself (RD-107-10).
--
-- Until now the difference between payload and recovery data existed only on disk, where
-- post-processing scans for it. The queue therefore had to report a `vol…par2` volume that
-- expired on the servers as a plain failure, even for a package whose payload was complete
-- and which unpacked without ever asking for a repair block.
--
-- The value is decided when the NZB is queued, from the file name the NZB gives, and it is
-- backfilled here with exactly the same rule so packages that are already in the queue stop
-- showing the false error.
ALTER TABLE downloads ADD COLUMN recovery INTEGER NOT NULL DEFAULT 0;

UPDATE downloads SET recovery = 1
WHERE kind = 'usenet' AND lower(file_name) LIKE '%.par2';
