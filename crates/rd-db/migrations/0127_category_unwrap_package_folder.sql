-- Per-category override for "unwrap a single folder named like the package" (RD-1140-01);
-- NULL inherits the global setting, which is off.
ALTER TABLE categories ADD COLUMN unwrap_package_folder INTEGER NULL;
