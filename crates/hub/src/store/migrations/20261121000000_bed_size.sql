-- A Marlin printer's bed as its space says it (X, Y, Z in mm, a JSON array), for one whose profile
-- comes from its firmware: M115 does not say it (ui.md "Its model"; ours, UI only).
ALTER TABLE machines ADD COLUMN bed_mm text;
