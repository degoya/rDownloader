-- RD-1240-14: a package's files start no earlier than this moment ("not before"). NULL starts
-- them as the queue reaches them, as every package did before. The scheduler's dispatch pass
-- skips the waiting files of a package whose moment lies ahead; a moment that has passed holds
-- nothing and stays until the next edit, so the row needs no clean-up when it falls due.
ALTER TABLE packages ADD COLUMN start_after TEXT;
