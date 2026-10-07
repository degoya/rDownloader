-- Which name a category rule's name pattern is matched against (RD-1140-02): 'file', 'package'
-- or 'either'. Every rule stored before the column existed was matched against the file name,
-- so that is the default.
ALTER TABLE category_rules ADD COLUMN name_target TEXT NOT NULL DEFAULT 'file'
    CHECK (name_target IN ('file', 'package', 'either'));
