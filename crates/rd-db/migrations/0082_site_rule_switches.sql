-- Which site rules and which rule groups are switched off (RD-110-08).
--
-- A shipped rule has no row in `site_rules` at all -- it lives in the signed pack compiled
-- into the binary -- so its switch cannot be a column there, and a group is not a rule in the
-- first place. Both are decisions *about* rules rather than parts of one, and both have to
-- survive a restart, so they share one small table keyed by what the decision is about.
--
-- A user rule keeps its own switch in `site_rules.enabled`, where RD-110-04 already put it,
-- so no fact has two homes: the `rule` scope here is for the shipped rules alone. The `group`
-- scope covers both sides, because the group is part of a rule body and both kinds carry one.
--
-- Absence means on. A row is written only once somebody decides about that rule or group, and
-- a row whose rule or group no longer exists is simply never read -- the same reasoning
-- `site_rule_checks` records for its leftovers.
CREATE TABLE site_rule_switches (
    scope TEXT NOT NULL CHECK (scope IN ('rule', 'group')),
    key TEXT NOT NULL,
    enabled INTEGER NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (scope, key)
);
