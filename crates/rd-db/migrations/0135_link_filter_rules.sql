-- RD-1240-09: LinkFilter rules, after JDownloader's LinkFilter. Each rule's conditions -- name
-- (glob or regex), size bounds, file types, hoster, source -- decide at intake, and again when a
-- person re-applies the rules to the LinkGrabber, whether a link is hidden, kept, or put into a
-- package or category. The enabled rules are asked in `position` order; the first match wins.
--
-- `category_id` carries no foreign key: a settings import replaces the categories wholesale, and
-- a cascade there would empty every rule's target. A category that is gone reads as none.
CREATE TABLE link_filter_rules (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    position INTEGER NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    name_pattern TEXT,
    name_syntax TEXT NOT NULL DEFAULT 'glob',
    size_min INTEGER,
    size_max INTEGER,
    extensions_json TEXT NOT NULL DEFAULT '[]',
    hoster TEXT,
    source TEXT,
    action TEXT NOT NULL,
    package_name TEXT,
    category_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX link_filter_rules_position_idx ON link_filter_rules(position);

-- The rule that hid a LinkGrabber link. A hidden link is kept, never deleted; deleting the rule
-- shows its links again.
ALTER TABLE link_candidates
    ADD COLUMN hidden_by_filter TEXT REFERENCES link_filter_rules(id) ON DELETE SET NULL;
