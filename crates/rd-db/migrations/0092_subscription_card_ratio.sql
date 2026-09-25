-- RD-120-42: the shape of a card's image area in a subscription's card view.
--
-- Stored as the ratio itself ('1:1', '3:2', '16:9', '4:3', '2:1'). The default '2:1' is the
-- closest of the five to the fixed height every card had before the choice existed, so an
-- existing subscription looks as it did without anybody touching it. A column of its own
-- rather than a change to 0090, which is applied on installations and checksummed by sqlx.
ALTER TABLE subscriptions ADD COLUMN card_ratio TEXT NOT NULL DEFAULT '2:1';
