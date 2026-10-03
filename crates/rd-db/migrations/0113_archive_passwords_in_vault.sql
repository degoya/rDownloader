-- RD-190-04: archive passwords move from plain columns into the vault.
--
-- Each of the four tables gets the `vault://` reference its password is stored under. The
-- old `password` columns stay, because SQLite cannot take a column out cheaply and SQL cannot
-- write the vault: the first start after this migration moves every value into the vault and
-- empties the column in the same transaction that sets the reference
-- (`Database::take_over_archive_passwords`). Nothing writes the old columns any more.
ALTER TABLE packages ADD COLUMN password_ref TEXT;
ALTER TABLE collector_packages ADD COLUMN password_ref TEXT;
ALTER TABLE nzb_imports ADD COLUMN password_ref TEXT;
ALTER TABLE subscription_items ADD COLUMN password_ref TEXT;

-- The vault entries nothing points at any more, and the ones a write reserved before it
-- stored the value. A reference lands here in the same transaction that lets go of it, so
-- no delete path — a package removed with its last file, a batch cascading to its packages,
-- a subscription's history cleared — can leave an entry behind; the sweep removes the vault
-- entry and then the row. `reserved = 1` is a write still under way: only a start removes
-- those, when no write can be.
CREATE TABLE archive_password_sweep (
    reference TEXT PRIMARY KEY NOT NULL,
    reserved INTEGER NOT NULL DEFAULT 0
);

CREATE TRIGGER packages_password_ref_deleted AFTER DELETE ON packages
WHEN OLD.password_ref IS NOT NULL
BEGIN
    INSERT OR IGNORE INTO archive_password_sweep (reference, reserved) VALUES (OLD.password_ref, 0);
END;

CREATE TRIGGER packages_password_ref_replaced AFTER UPDATE OF password_ref ON packages
WHEN OLD.password_ref IS NOT NULL AND OLD.password_ref IS NOT NEW.password_ref
BEGIN
    INSERT OR IGNORE INTO archive_password_sweep (reference, reserved) VALUES (OLD.password_ref, 0);
END;

CREATE TRIGGER collector_packages_password_ref_deleted AFTER DELETE ON collector_packages
WHEN OLD.password_ref IS NOT NULL
BEGIN
    INSERT OR IGNORE INTO archive_password_sweep (reference, reserved) VALUES (OLD.password_ref, 0);
END;

CREATE TRIGGER collector_packages_password_ref_replaced AFTER UPDATE OF password_ref ON collector_packages
WHEN OLD.password_ref IS NOT NULL AND OLD.password_ref IS NOT NEW.password_ref
BEGIN
    INSERT OR IGNORE INTO archive_password_sweep (reference, reserved) VALUES (OLD.password_ref, 0);
END;

CREATE TRIGGER nzb_imports_password_ref_deleted AFTER DELETE ON nzb_imports
WHEN OLD.password_ref IS NOT NULL
BEGIN
    INSERT OR IGNORE INTO archive_password_sweep (reference, reserved) VALUES (OLD.password_ref, 0);
END;

CREATE TRIGGER nzb_imports_password_ref_replaced AFTER UPDATE OF password_ref ON nzb_imports
WHEN OLD.password_ref IS NOT NULL AND OLD.password_ref IS NOT NEW.password_ref
BEGIN
    INSERT OR IGNORE INTO archive_password_sweep (reference, reserved) VALUES (OLD.password_ref, 0);
END;

CREATE TRIGGER subscription_items_password_ref_deleted AFTER DELETE ON subscription_items
WHEN OLD.password_ref IS NOT NULL
BEGIN
    INSERT OR IGNORE INTO archive_password_sweep (reference, reserved) VALUES (OLD.password_ref, 0);
END;

CREATE TRIGGER subscription_items_password_ref_replaced AFTER UPDATE OF password_ref ON subscription_items
WHEN OLD.password_ref IS NOT NULL AND OLD.password_ref IS NOT NEW.password_ref
BEGIN
    INSERT OR IGNORE INTO archive_password_sweep (reference, reserved) VALUES (OLD.password_ref, 0);
END;
