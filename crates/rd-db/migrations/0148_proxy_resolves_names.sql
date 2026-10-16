-- Whether a proxy resolves target names itself (RD-1320-07). 0, the default, pins a stranger's
-- address: the service resolves and checks the name and asks the proxy for a tunnel to that
-- address. 1 hands the proxy the name, for a network where only the proxy can resolve.
ALTER TABLE proxy_profiles ADD COLUMN resolves_names INTEGER NOT NULL DEFAULT 0;
