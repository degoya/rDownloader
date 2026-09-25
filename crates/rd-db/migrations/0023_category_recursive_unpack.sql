-- Per-category recursive unpack override; NULL inherits the global setting.
ALTER TABLE categories ADD COLUMN recursive_unpack INTEGER NULL;
