ALTER TABLE downloads ADD COLUMN account_id TEXT;
ALTER TABLE downloads ADD COLUMN proxy_profile_id TEXT;

CREATE INDEX downloads_account_idx ON downloads(account_id);
CREATE INDEX downloads_proxy_idx ON downloads(proxy_profile_id);
