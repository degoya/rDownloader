-- RD-1230-03: site rules carry no signature any more. They travel as export files that one
-- installation writes and another imports with their switches, and the app brings a short list
-- of examples for free sites; nothing verifies a signer, so nothing records one.
--
-- The highest sequence per signer (`0132`) goes with the signed file, and so do the signer and
-- the sequence a rule recorded. `origin` stays: `import`, `editor`, `mcp`, `example` or
-- `unknown`. A row that still says `signed` reads as `unknown`, the word for one this build does
-- not know; there is no installed base to rewrite it for.
DROP TABLE site_rule_pack_sequences;
ALTER TABLE site_rules DROP COLUMN origin_signer;
ALTER TABLE site_rules DROP COLUMN origin_sequence;
