-- Where a package's files lived before its category changed.
--
-- A category change now moves the package's data into `<category>/<package>/`. Files that are
-- still transferring keep their open `.part` where it is and are promoted into the new
-- destination when they finish, so the old directory can only be swept once the last of them is
-- done. This column is what the completion path uses to find it again; it is cleared as soon as
-- the sweep has run. NULL means there is nothing outstanding.
ALTER TABLE packages ADD COLUMN previous_destination TEXT;
