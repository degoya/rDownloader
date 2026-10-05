-- Per-category override for "unpack a multi-volume RAR set while it downloads" (RD-1100-07);
-- NULL inherits the global setting, which is off.
ALTER TABLE categories ADD COLUMN direct_unpack INTEGER NULL;
