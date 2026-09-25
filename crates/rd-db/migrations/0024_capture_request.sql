-- Request metadata of intercepted browser downloads (effective URL, method, referrer,
-- user agent, content disposition, allowlisted headers); NULL for every other source.
ALTER TABLE link_candidates ADD COLUMN request_json TEXT;
