-- User-written site rules (RD-110-04).
--
-- The rules the project ships arrive as one signed document compiled into the binary and are
-- never in this table. A person's own rules live here, unsigned and only for this instance.
-- `rule_json` is the rule as `rd_siterules::Rule` serialises it; the database stores it as
-- text and does not read it, because the validation belongs to the system boundary that
-- accepted it and a dependency on the rule crate would rebuild this one on every change to
-- the executor. `name`, `rule_group` and `enabled` are repeated outside the body so a list
-- can be drawn without parsing every rule.
--
-- `id` is the rule's own identifier. A user rule never takes a shipped rule's id: the
-- catalogue that holds both sides refuses it before anything is written here.
CREATE TABLE site_rules (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    rule_group TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    rule_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
