-- What a request asks for: 'use' (jobs) or 'operate' (also move, heat, G-code); Give gives it.
ALTER TABLE access_requests
    ADD COLUMN level text NOT NULL DEFAULT 'use' CHECK (level IN ('use', 'operate'));
