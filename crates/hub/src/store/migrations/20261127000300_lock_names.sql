-- A door controller's own names for its locks (#123, Space -> Access): a JSON list, one per lock
-- in order, "" where the lock keeps its number ("Lock 3").
ALTER TABLE machines ADD COLUMN lock_names text;
