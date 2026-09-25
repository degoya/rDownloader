-- RD-080-09: what a livestream recording produced besides the video.
--
-- Written after every segment rather than at the end, because the whole point of the
-- reconnect loop is that a crash three hours in must not lose the three hours already on
-- disk. The history says which files exist, where the gaps are, and which sidecars were
-- asked for but not offered.
ALTER TABLE downloads ADD COLUMN recording_json TEXT;

-- The recording policy of a channel: splitting, remux target, sidecars and VOD fallback.
-- On the channel rather than globally, because "split this 12-hour marathon hourly" is a
-- statement about one channel, not about recording in general.
ALTER TABLE stream_channels ADD COLUMN recording_json TEXT;
