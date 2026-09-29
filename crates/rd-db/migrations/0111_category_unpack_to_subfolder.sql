-- Per-category override for "unpack every archive into a folder of its own" (RD-170-16);
-- NULL inherits the global setting, which is off.
ALTER TABLE categories ADD COLUMN unpack_to_subfolder INTEGER NULL;
