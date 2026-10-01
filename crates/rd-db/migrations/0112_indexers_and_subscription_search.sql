-- RD-180-19: Newznab indexers defined once, searched from the LinkGrabber and taken into
-- indexer subscriptions. The API key lives in the vault; `secret_ref` is its reference, as for
-- every other credential. `categories_json` is a JSON array of the indexer's own category ids a
-- search asks for when it names none (empty asks for everything).
CREATE TABLE indexers (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    url TEXT NOT NULL,
    secret_ref TEXT,
    categories_json TEXT NOT NULL DEFAULT '[]',
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- RD-180-20: the search term and parameters an indexer subscription sends (`q`, `maxage`, `pw`,
-- `pred`), as a JSON object. Every existing subscription gets the empty object, which sends
-- exactly what it sent before this column.
ALTER TABLE subscriptions ADD COLUMN indexer_search_json TEXT NOT NULL DEFAULT '{}';
