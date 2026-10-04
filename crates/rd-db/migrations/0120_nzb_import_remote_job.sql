-- RD-191-13: an NZB from the LinkGrabber handed to a remote-job provider instead of the queue.
-- The import stays in the LinkGrabber naming the job it went to, so it is not queued a second
-- time by accident. Removing the job from the remote-job list (or its account) clears the mark.
ALTER TABLE nzb_imports ADD COLUMN remote_job_id TEXT REFERENCES remote_jobs(id) ON DELETE SET NULL;
