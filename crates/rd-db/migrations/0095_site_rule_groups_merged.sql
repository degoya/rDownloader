-- Site rules without a compiled-in pack, and five groups instead of seven (RD-130-07).
--
-- Since 1.3 no rule arrives with the binary: the project's rules are a signed release file
-- that the import verifies and stores in `site_rules` as the person's own, switched off. An
-- installation keeps nothing of the pack it had compiled in -- the owner's decision of
-- 2026-09-25 -- so the `rule` rows of `site_rule_switches`, which only ever held the switches
-- of those rules, go. A rule's switch is `site_rules.enabled` for every rule now.
--
-- The groups `comics` and `magazines` are merged into `ebooks`, both in the column the list is
-- drawn from and in the rule body the catalogue reads, so the two never disagree. `graphics`
-- stays a group of its own -- 3D and graphics assets are not books -- and is not touched here.
-- A body that is not JSON at all is left as it is; the list already shows such a row as
-- unreadable, and rewriting it would make it no more readable.
--
-- A group switch survives the merge, and off wins: `ebooks` is switched off afterwards when
-- any of the three was. Whoever switched magazines off did not want those pages fetched, and
-- turning the merged group on would do exactly that behind their back; a switch that is off
-- too many is visible and one click away.

DELETE FROM site_rule_switches WHERE scope = 'rule';

UPDATE site_rules
SET rule_group = 'ebooks',
    rule_json = CASE
        WHEN json_valid(rule_json) THEN json_set(rule_json, '$.group', 'ebooks')
        ELSE rule_json
    END
WHERE rule_group IN ('comics', 'magazines')
   OR CASE
          WHEN json_valid(rule_json) THEN json_extract(rule_json, '$.group')
      END IN ('comics', 'magazines');

INSERT OR REPLACE INTO site_rule_switches (scope, key, enabled, updated_at)
SELECT 'group', 'ebooks', MIN(enabled), MAX(updated_at)
FROM site_rule_switches
WHERE scope = 'group' AND key IN ('ebooks', 'comics', 'magazines')
GROUP BY scope;

DELETE FROM site_rule_switches
WHERE scope = 'group' AND key IN ('comics', 'magazines');
