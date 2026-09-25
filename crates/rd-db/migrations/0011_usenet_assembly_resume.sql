ALTER TABLE nzb_files ADD COLUMN assembly_name TEXT;
ALTER TABLE nzb_files ADD COLUMN declared_size INTEGER CHECK(declared_size >= 0);
ALTER TABLE nzb_segments ADD COLUMN part_begin INTEGER CHECK(part_begin > 0);
ALTER TABLE nzb_segments ADD COLUMN part_end INTEGER CHECK(part_end > 0);
