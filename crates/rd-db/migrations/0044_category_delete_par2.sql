-- Per-category override for deleting the PAR2 recovery set after a successful unpack;
-- NULL inherits the global setting, which is off.
ALTER TABLE categories ADD COLUMN delete_par2 INTEGER NULL;
