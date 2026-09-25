-- Per-category SFV verification override; NULL inherits the global setting.
ALTER TABLE categories ADD COLUMN sfv_verify INTEGER NULL;
