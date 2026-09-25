-- RD-120-37: how the LinkGrabber draws a subscription's pending hits, and whether the card
-- slider turns its pages on its own.
--
-- Both default to what every subscription showed before the choice existed -- the list, and no
-- movement -- so an existing subscription looks exactly as it did without anybody touching it.
-- One migration for the two columns because they are one feature: autoplay means nothing
-- without the card view.
ALTER TABLE subscriptions ADD COLUMN view TEXT NOT NULL DEFAULT 'list';
ALTER TABLE subscriptions ADD COLUMN autoplay INTEGER NOT NULL DEFAULT 0;
