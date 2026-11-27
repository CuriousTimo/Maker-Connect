-- A file uploaded from a browser straight to a printer's storage (D-J #225), as Prusa Connect Local
-- keeps it (`internal/app/incoming.go`): a temporary copy, never listed in Files, gone once it is
-- sent, kept an hour when the send failed.
ALTER TABLE files ADD COLUMN incoming boolean NOT NULL DEFAULT false;
