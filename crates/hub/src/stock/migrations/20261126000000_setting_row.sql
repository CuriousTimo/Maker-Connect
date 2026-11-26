-- The order Spoolman's `GET /setting/` lists the set settings in: `select(models.Setting)` without
-- an ORDER BY, which SQLite answers in rowid order. `row` is that rowid: an upsert keeps it, a
-- setting set again after it was unset gets a new one at the end.
ALTER TABLE setting ADD COLUMN row bigint GENERATED ALWAYS AS IDENTITY;
