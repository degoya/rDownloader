-- RD-110-21: whether a watched release page keeps every release of an episode.
--
-- Off by default, which is the point of watching such a page: the same episode is posted
-- again by another group, in another quality, weeks later, and a subscription that does not
-- recognise that enqueues it every time. The column is the explicit counter-choice for
-- somebody who collects versions -- it makes the item key the address again, which is what
-- every other kind of subscription uses.
ALTER TABLE subscriptions ADD COLUMN every_release INTEGER NOT NULL DEFAULT 0;
