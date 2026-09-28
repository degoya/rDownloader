-- The name a remote job's source was handed in under -- a container's file name.
--
-- The bytes cross the plugin contract without a name, and a finished job's files were packaged
-- under whatever the provider called its transfer. At Premiumize that was the fixed upload name
-- `source.nzb` for every NZB (owner report, 2026-09-27). The host keeps the person's own name
-- here and names the job's LinkGrabber package after it. NULL for a magnet, an address, or a
-- caller that sent no name.
ALTER TABLE remote_jobs ADD COLUMN source_name TEXT;
