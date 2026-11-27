-- A door controller's locks open to every member (wiki Design: Devices and Rights, "Machines in
-- Permissions is listed per type"): a JSON list of lock numbers from 0. Any other lock needs
-- permission, through a group that holds it.
ALTER TABLE machines ADD COLUMN locks_open text;
