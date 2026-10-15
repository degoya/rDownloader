-- RD-1310-01: a LinkFilter condition on the online check's verdict. 1 holds only for a link the
-- check found unresolvable with `collector.check_no_resolver` -- no installed resolver claims its
-- host and the address answered with a page. The new action `delete` needs no column: `action`
-- is TEXT without a CHECK (0135).
ALTER TABLE link_filter_rules ADD COLUMN without_resolver INTEGER NOT NULL DEFAULT 0;
