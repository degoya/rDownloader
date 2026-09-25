-- Links a person stated are not mirrors of each other (RD-110-34).
--
-- RD-110-18 knows three sources for a mirror group, and the third -- a shared file name with
-- nothing to corroborate it -- is explicitly a proposal. RD-110-19 made it read as one, but
-- left it just as irreversible as a declaration: whoever could see that two links are a
-- different cut of the same release could only pin one of them. This table is the way out.
--
-- A pair, not a group and not a flag on the link. A group key is derived from the file name
-- and changes the moment a size arrives; the source is re-decided on every regroup. A
-- decision anchored to either would evaporate exactly at the moment it has to hold -- the
-- online check -- so what is stored is the fact about the two links themselves, which no
-- recomputation revises. `rd_collector::group_mirrors` reads it before any of the three
-- sources is allowed to form a group, the same place the rule lives that two links with the
-- same address are never mirrors of each other.
--
-- The smaller identifier is always `left_id`, so one pair is one row whichever way round it
-- was stated. The cascade is what makes a deleted candidate take its decisions with it: the
-- statement was about that link, and nothing is left behind to bind a later one.
CREATE TABLE collector_mirror_separations (
    left_id TEXT NOT NULL REFERENCES link_candidates (id) ON DELETE CASCADE,
    right_id TEXT NOT NULL REFERENCES link_candidates (id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    PRIMARY KEY (left_id, right_id)
);

-- The regroup asks for the pairs of one package, which reaches the rows through both ends.
CREATE INDEX idx_mirror_separations_right ON collector_mirror_separations (right_id);
