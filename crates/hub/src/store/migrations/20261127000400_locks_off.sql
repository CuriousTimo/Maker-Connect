-- A door controller's relays that drive no lock (#123: 6 relays, 4 lockers): a JSON list of lock
-- numbers from 0. Hidden on Space -> Access; the hub sends no Open for them.
ALTER TABLE machines ADD COLUMN locks_off text;
