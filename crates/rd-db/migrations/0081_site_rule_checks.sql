-- What the rule self-test found, one row per rule (RD-110-09).
--
-- Not a column on `site_rules`: that table holds a person's *own* rules only, and the rules
-- the project ships live in the signed pack with no row anywhere. A result has to exist for
-- both sides, so it gets its own table keyed by the rule's own identifier -- and therefore no
-- foreign key, because half the keys deliberately have nothing to point at.
--
-- `verdict` is one of `ok`, `structural`, `blocked`, `dead`, as `rd_siterules::Verdict`
-- spells it; a word this build does not know is ignored rather than guessed at. `code` is the
-- refusal's stable code (`site_rules.structure`, ...) so the reason survives the four-way
-- sort, and is NULL exactly when the rule answered. `checked_at` is the run's own date: the
-- `checked` inside a shipped rule sits in the signed payload and cannot be rewritten here.
--
-- A rule that is removed leaves its row behind. Nothing reads a row whose rule is gone, and
-- a row is a great deal cheaper than a delete path that has to run on every pack update.
CREATE TABLE site_rule_checks (
    rule_id TEXT PRIMARY KEY,
    verdict TEXT NOT NULL,
    code TEXT,
    links INTEGER NOT NULL DEFAULT 0,
    pages INTEGER NOT NULL DEFAULT 0,
    checked_at TEXT NOT NULL
);
