-- One manual order across both LinkGrabber kinds.
--
-- The list shows collector packages and NZB imports in a single stream, but only the packages
-- carried a `position`; NZB imports were slotted in by creation time and could not be dragged.
-- Two independent sequences cannot interleave, so both tables now draw from one sequence.
ALTER TABLE nzb_imports ADD COLUMN position INTEGER NOT NULL DEFAULT 0;

-- The backfill reproduces the order the list shows *before* this migration, so nothing moves on
-- upgrade. That order is not "everything by creation time": the collector packages already come
-- in their dragged `position` order, and the client merged the NZB imports into that sequence by
-- comparing creation times as it walked it. Numbering the union by `created_at` alone would throw
-- away every drag a person had performed.
--
-- The spine is the collector sequence exactly as the listing returns it.
CREATE TABLE grabber_spine_0074 AS
SELECT id AS id,
       created_at AS created_at,
       ROW_NUMBER() OVER (ORDER BY position, created_at, id) AS rank
FROM collector_packages;

CREATE TABLE grabber_order_0074 (
    kind TEXT NOT NULL,
    id TEXT NOT NULL,
    -- How many spine entries come before this row.
    spine INTEGER NOT NULL,
    -- 0 = collector, 1 = NZB: at an equal spine value the package comes first, which is what the
    -- client's merge did when it emitted the package it was holding before the import.
    kind_rank INTEGER NOT NULL,
    within TEXT NOT NULL
);

INSERT INTO grabber_order_0074 (kind, id, spine, kind_rank, within)
SELECT 'collector', id, rank, 0, created_at FROM grabber_spine_0074;

-- The client emitted spine entries while their creation time was not newer than the import's, so
-- an import lands after the longest such prefix — that is, before the first spine entry that is
-- newer than it. With no newer entry it goes to the end. The imports are themselves ordered by
-- creation time, so a single prefix per import reproduces the whole merge.
INSERT INTO grabber_order_0074 (kind, id, spine, kind_rank, within)
SELECT 'nzb', n.id,
       COALESCE(
           (SELECT MIN(s.rank) FROM grabber_spine_0074 s WHERE s.created_at > n.created_at),
           (SELECT COUNT(*) FROM grabber_spine_0074) + 1
       ) - 1,
       1, n.created_at
FROM nzb_imports n;

-- Materialised rather than computed inside the two UPDATEs: reading a table while updating it is
-- the kind of statement whose result depends on the scan order, and the positions must not.
CREATE TABLE grabber_merged_0074 AS
SELECT kind AS kind,
       id AS id,
       ROW_NUMBER() OVER (ORDER BY spine, kind_rank, within, id) AS merged
FROM grabber_order_0074;

UPDATE collector_packages SET position = COALESCE(
    (SELECT merged FROM grabber_merged_0074 m WHERE m.kind = 'collector' AND m.id = collector_packages.id),
    position
);

UPDATE nzb_imports SET position = COALESCE(
    (SELECT merged FROM grabber_merged_0074 m WHERE m.kind = 'nzb' AND m.id = nzb_imports.id),
    0
);

DROP TABLE grabber_merged_0074;
DROP TABLE grabber_order_0074;
DROP TABLE grabber_spine_0074;

CREATE INDEX nzb_imports_position_idx ON nzb_imports(position);
