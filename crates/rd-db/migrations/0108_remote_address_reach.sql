-- How far an address a stranger's document named may reach (RD-150-03).
--
-- A Metalink document names mirrors, an intake parser proposes links, a crawler finds them on a
-- page; the service then requests those addresses on the person's behalf -- the transfer, and
-- before it the LinkGrabber's automatic online check. Such an address may never point at this
-- machine, and it may point into the person's own network (RFC 1918, IPv6 unique-local) only
-- when they handed the document over themselves -- pasted it or dropped it into a watched
-- folder -- rather than having it relayed from a web page or another program. This machine
-- needs no column; the person's network does.

-- Per source row of a download with a source set. 0 is the strict answer and the default.
ALTER TABLE download_sources ADD COLUMN local_network INTEGER NOT NULL DEFAULT 0
    CHECK (local_network IN (0, 1));

-- Per LinkGrabber candidate: NULL for a link the person gave themselves, which the online check
-- requests as it always did; otherwise the reach its checks are held to.
ALTER TABLE link_candidates ADD COLUMN remote_reach TEXT
    CHECK (remote_reach IN ('internet', 'local_network'));
