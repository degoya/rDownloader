-- RD-1240-35: what stays of a skipped or dismissed subscription item once it is older than
-- `subscription_item_retention_days` -- its key, nothing else. The poll recognises an item by
-- `(subscription_id, item_key)`; a key here counts as archived, so a feed that still lists the
-- item does not bring it back, while its title, address and details leave the database.
-- WITHOUT ROWID: the primary key is the whole row, one b-tree and no second index.
CREATE TABLE subscription_item_keys (
    subscription_id TEXT NOT NULL REFERENCES subscriptions(id) ON DELETE CASCADE,
    item_key TEXT NOT NULL,
    PRIMARY KEY (subscription_id, item_key)
) WITHOUT ROWID;
