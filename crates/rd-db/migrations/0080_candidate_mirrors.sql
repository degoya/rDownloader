-- Mirror groups in the LinkGrabber (RD-110-18).
--
-- A release page posts the same file to five hosters. Those five links are mirrors of one
-- another, not five candidates and not duplicates: a duplicate is the same address a second
-- time and is set aside, a mirror is a different address for the same bytes and is kept,
-- because it is what remains when the chosen one goes offline. `state` is untouched by all
-- of this; the two concepts never write to the same column.
--
-- Two kinds of column, deliberately separated.
--
-- `mirror_declared`, `mirror_quality` and `mirror_language` are what the *source* said —
-- the site rule whose page is one release, or the crawler that named the group. They are
-- written once, at intake, and never recomputed, so regrouping after an online check cannot
-- lose what the page stated.
--
-- `mirror_group`, `mirror_source` and `mirror_selected` are derived and are rewritten in
-- full whenever the package they belong to is regrouped. A group key is unique within one
-- package and means nothing outside it, which is why nothing here is a foreign key: a group
-- lives inside a package because that is the unit the queue downloads.
--
-- `mirror_source` records which of the three sources produced the group, because they are not
-- equally strong and the interface has to be able to say so: `declared` is what the page
-- stated, `name_and_size` is two links agreeing on both, and `name` is a shared file name
-- with nothing to corroborate it -- a proposal somebody may want to overrule.
ALTER TABLE link_candidates ADD COLUMN mirror_declared TEXT;
ALTER TABLE link_candidates ADD COLUMN mirror_quality TEXT;
ALTER TABLE link_candidates ADD COLUMN mirror_language TEXT;
ALTER TABLE link_candidates ADD COLUMN mirror_group TEXT;
ALTER TABLE link_candidates ADD COLUMN mirror_source TEXT;
ALTER TABLE link_candidates ADD COLUMN mirror_selected INTEGER NOT NULL DEFAULT 0;

-- The one query this adds: the members of a package, read back in submission order whenever
-- the package is regrouped and whenever the interface draws the group.
CREATE INDEX idx_link_candidates_mirror ON link_candidates (package_id, mirror_group);
