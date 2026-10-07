-- Per-category override of the package-name rules (RD-1140-05): a JSON object of four optional
-- switches; NULL, and every switch the object leaves out, inherits the global setting.
ALTER TABLE categories ADD COLUMN package_name_rules_json TEXT NULL;
-- The category's regex find -> replace pairs, a JSON list; NULL inherits the global list, a
-- list (an empty one too) replaces it.
ALTER TABLE categories ADD COLUMN package_name_regex_json TEXT NULL;
