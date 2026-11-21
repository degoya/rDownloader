-- RD-1440-07: a LinkGrabber link handed to a remote-job provider. With the setting that keeps
-- handed-over links in the LinkGrabber, the link stays naming the job it went to, so a second
-- hand-over sends nothing again. Removing the job from the remote-job list (or its account)
-- clears the mark.
ALTER TABLE link_candidates ADD COLUMN remote_job_id TEXT REFERENCES remote_jobs(id) ON DELETE SET NULL;
