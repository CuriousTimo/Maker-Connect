//! Accounts: people, how they sign in, which spaces they belong to, and their sessions.
//!
//! Built to go from one person on a Pi to a hosted service with many spaces:
//!
//! - A **user** belongs to the instance, not to a space; **memberships** put them in spaces,
//!   with a role in each. Being an instance admin (running the server) is separate from
//!   owning a space, and does not by itself show a space's data.
//! - How someone signs in is an **identity** of theirs: a password now, an OpenID Connect
//!   provider later, both leading to the same user.
//! - Everything lives in the database, nothing only in memory, so several cloud processes
//!   can share it later. Secrets are kept only as hashes: passwords with Argon2id, session
//!   tokens with SHA-256 (they are long and random, so a fast hash is enough).

use crate::store::{Store, now};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use rand::RngCore;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// A session lasts this long after it was last used.
pub const SESSION_SECONDS: i64 = 30 * 24 * 3600;
/// `last_seen` is written at most this often, so reading pages does not write every time.
const SEEN_EVERY: i64 = 300;
/// The shortest password taken.
pub const MIN_PASSWORD: usize = 10;

/// The tables. Only ever adds (see [`crate::store`]'s migration).
pub fn migrate(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS users (
             id INTEGER PRIMARY KEY,
             name TEXT NOT NULL UNIQUE COLLATE NOCASE,
             display_name TEXT NOT NULL,
             instance_admin INTEGER NOT NULL DEFAULT 0,
             disabled INTEGER NOT NULL DEFAULT 0,
             created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS identities (
             id INTEGER PRIMARY KEY,
             user_id INTEGER NOT NULL REFERENCES users(id),
             provider TEXT NOT NULL,
             subject TEXT NOT NULL,
             secret TEXT,
             created_at INTEGER NOT NULL,
             UNIQUE (provider, subject)
         );
         CREATE TABLE IF NOT EXISTS memberships (
             user_id INTEGER NOT NULL REFERENCES users(id),
             space_id INTEGER NOT NULL REFERENCES spaces(id),
             role TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             PRIMARY KEY (user_id, space_id)
         );
         CREATE INDEX IF NOT EXISTS memberships_by_space ON memberships (space_id);
         CREATE TABLE IF NOT EXISTS sessions (
             id INTEGER PRIMARY KEY,
             token_hash TEXT NOT NULL UNIQUE,
             user_id INTEGER NOT NULL REFERENCES users(id),
             space_id INTEGER REFERENCES spaces(id),
             created_at INTEGER NOT NULL,
             last_seen INTEGER NOT NULL,
             expires_at INTEGER NOT NULL,
             user_agent TEXT NOT NULL DEFAULT '',
             address TEXT NOT NULL DEFAULT ''
         );
         CREATE INDEX IF NOT EXISTS sessions_by_user ON sessions (user_id);
         CREATE TABLE IF NOT EXISTS invites (
             id INTEGER PRIMARY KEY,
             token_hash TEXT NOT NULL UNIQUE,
             space_id INTEGER NOT NULL REFERENCES spaces(id),
             role TEXT NOT NULL,
             created_by INTEGER NOT NULL REFERENCES users(id),
             created_at INTEGER NOT NULL,
             expires_at INTEGER NOT NULL,
             used_by INTEGER REFERENCES users(id),
             used_at INTEGER,
             revoked INTEGER NOT NULL DEFAULT 0,
             -- Into the whole organisation of the space (all its spaces), or this space only.
             whole_organisation INTEGER NOT NULL DEFAULT 1
         );
         CREATE TABLE IF NOT EXISTS machine_grants (
             space_id INTEGER NOT NULL REFERENCES spaces(id),
             machine_id TEXT NOT NULL,
             user_id INTEGER NOT NULL REFERENCES users(id),
             level TEXT NOT NULL,
             granted_by INTEGER,
             granted_at INTEGER NOT NULL,
             expires_at INTEGER,
             PRIMARY KEY (machine_id, user_id)
         );
         CREATE TABLE IF NOT EXISTS api_tokens (
             id INTEGER PRIMARY KEY,
             token_hash TEXT NOT NULL UNIQUE,
             space_id INTEGER NOT NULL REFERENCES spaces(id),
             created_by INTEGER NOT NULL REFERENCES users(id),
             name TEXT NOT NULL,
             role TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             last_used INTEGER,
             revoked INTEGER NOT NULL DEFAULT 0
         );
         CREATE TABLE IF NOT EXISTS audit_log (
             id INTEGER PRIMARY KEY,
             space_id INTEGER NOT NULL,
             at INTEGER NOT NULL,
             user_id INTEGER,
             token_id INTEGER,
             actor TEXT NOT NULL,
             action TEXT NOT NULL,
             target TEXT NOT NULL DEFAULT '',
             detail TEXT NOT NULL DEFAULT ''
         );
         CREATE INDEX IF NOT EXISTS audit_by_space ON audit_log (space_id, at);
         CREATE TABLE IF NOT EXISTS organisations (
             id INTEGER PRIMARY KEY,
             uid TEXT NOT NULL UNIQUE,
             name TEXT NOT NULL,
             created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS org_memberships (
             user_id INTEGER NOT NULL REFERENCES users(id),
             organisation_id INTEGER NOT NULL REFERENCES organisations(id),
             role TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             PRIMARY KEY (user_id, organisation_id)
         );
         -- Groups of machines (3D printers, say): permission on a group is permission on
         -- every machine in it, also on those added later.
         CREATE TABLE IF NOT EXISTS machine_groups (
             id INTEGER PRIMARY KEY,
             space_id INTEGER NOT NULL REFERENCES spaces(id),
             name TEXT NOT NULL,
             created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS machine_group_members (
             group_id INTEGER NOT NULL REFERENCES machine_groups(id),
             machine_id TEXT NOT NULL,
             PRIMARY KEY (group_id, machine_id)
         );
         -- Someone asks the staff for permission (or a training) on a machine.
         CREATE TABLE IF NOT EXISTS access_requests (
             id INTEGER PRIMARY KEY,
             space_id INTEGER NOT NULL REFERENCES spaces(id),
             user_id INTEGER NOT NULL REFERENCES users(id),
             machine_id TEXT NOT NULL,
             asked_at INTEGER NOT NULL,
             state TEXT NOT NULL DEFAULT 'open',
             handled_by INTEGER,
             handled_at INTEGER
         );
         CREATE TABLE IF NOT EXISTS group_grants (
             group_id INTEGER NOT NULL REFERENCES machine_groups(id),
             user_id INTEGER NOT NULL REFERENCES users(id),
             level TEXT NOT NULL,
             granted_by INTEGER,
             granted_at INTEGER NOT NULL,
             expires_at INTEGER,
             PRIMARY KEY (group_id, user_id)
         );",
    )?;
    // Spaces belong to an organisation and have an id of their own under the hood (a UUID),
    // apart from their name and from their row number (which counts up and repeats on
    // another server).
    for (column, definition) in [
        ("uid", "TEXT"),
        // Ask for PPE before a machine is used (crate::safety): for a makerspace, not at home.
        ("ppe_checks", "INTEGER NOT NULL DEFAULT 0"),
        // The example files went in once (crate::samples).
        ("samples_given", "INTEGER NOT NULL DEFAULT 0"),
        ("organisation_id", "INTEGER REFERENCES organisations(id)"),
    ] {
        let present: bool = db.query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('spaces') WHERE name = ?1",
            [column],
            |r| r.get(0),
        )?;
        if !present {
            db.execute(
                &format!("ALTER TABLE spaces ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
    }
    db.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS spaces_by_uid ON spaces (uid);")?;
    let bare: Vec<(i64, String, Option<String>)> = {
        let mut q = db.prepare(
            "SELECT id, name, uid FROM spaces WHERE uid IS NULL OR organisation_id IS NULL",
        )?;
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (space, name, uid) in bare {
        // A space from before organisations: an organisation of its own, of the same name,
        // its members that organisation's.
        db.execute(
            "INSERT INTO organisations (uid, name, created_at) VALUES (?1, ?2, ?3)",
            params![new_uuid(), name, now()],
        )?;
        let org = db.last_insert_rowid();
        db.execute(
            "UPDATE spaces SET organisation_id = ?2, uid = COALESCE(uid, ?3) WHERE id = ?1",
            params![space, org, uid.unwrap_or_else(new_uuid)],
        )?;
        db.execute(
            "INSERT OR IGNORE INTO org_memberships (user_id, organisation_id, role, created_at)
             SELECT user_id, ?2, role, created_at FROM memberships WHERE space_id = ?1",
            params![space, org],
        )?;
        db.execute("DELETE FROM memberships WHERE space_id = ?1", [space])?;
    }
    Ok(())
}

/// An id under the hood, the same wherever the data goes and saying nothing about how many
/// there are: a random UUID (version 4), as `0f9c2b6e-…`. People see names, not this.
pub fn new_uuid() -> String {
    let mut b = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // the RFC 4122 variant
    let h = hex(&b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// Someone signed in, as a request knows them.
#[derive(Debug, Clone, Serialize)]
pub struct Signed {
    pub user_id: i64,
    pub name: String,
    pub display_name: String,
    pub instance_admin: bool,
    #[serde(skip)]
    pub session_id: i64,
    /// The space the session is in, when it is in one (see [`crate::access`]).
    pub space_id: Option<i64>,
}

/// A user's password, as kept for signing in.
pub struct Credentials {
    pub user_id: i64,
    pub hash: String,
    pub disabled: bool,
}

/// A long random secret, as hex: for session tokens.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex(&bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

/// A password's Argon2id hash (PHC string). Slow on purpose: call it outside the store's lock.
pub fn hash_password(password: &str) -> String {
    let mut salt = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    let salt = SaltString::encode_b64(&salt).expect("16 bytes make a salt");
    argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("Argon2 hashes any password")
        .to_string()
}

/// Whether `password` is the one `hash` was made from. Slow on purpose, like [`hash_password`].
pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|parsed| {
        argon2::Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}

/// What a password must be; the reason when it is not.
pub fn password_problem(password: &str) -> Option<String> {
    if password.chars().count() < MIN_PASSWORD {
        return Some(format!(
            "a password needs at least {MIN_PASSWORD} characters"
        ));
    }
    if password.len() > 1024 {
        return Some("that password is longer than 1024 bytes".to_owned());
    }
    None
}

/// What a sign-in name must be: 2 to 40 letters, digits, `.`, `-` or `_`.
pub fn name_problem(name: &str) -> Option<&'static str> {
    let ok = (2..=40).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    (!ok).then_some("a sign-in name is 2 to 40 letters, digits, dots, dashes or underscores")
}

/// A code people type: 8 characters that read well, as `ABCD-EFGH`.
pub fn setup_code() -> String {
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let code: String = bytes
        .iter()
        .map(|b| CHARS[*b as usize % CHARS.len()] as char)
        .collect();
    format!("{}-{}", &code[..4], &code[4..])
}

impl Store {
    pub fn has_users(&self) -> rusqlite::Result<bool> {
        self.db
            .query_row("SELECT EXISTS (SELECT 1 FROM users)", [], |r| r.get(0))
    }

    /// A new user who signs in with a password (already hashed).
    pub fn create_user(
        &self,
        name: &str,
        display_name: &str,
        password_hash: &str,
        instance_admin: bool,
    ) -> rusqlite::Result<i64> {
        self.db.execute(
            "INSERT INTO users (name, display_name, instance_admin, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![name, display_name, instance_admin, now()],
        )?;
        let id = self.db.last_insert_rowid();
        self.db.execute(
            "INSERT INTO identities (user_id, provider, subject, secret, created_at)
             VALUES (?1, 'password', ?2, ?3, ?4)",
            params![id, id.to_string(), password_hash, now()],
        )?;
        Ok(id)
    }

    /// Whether a user by this name exists (names are the same whatever their case).
    pub fn user_id(&self, name: &str) -> rusqlite::Result<Option<i64>> {
        self.db
            .query_row("SELECT id FROM users WHERE name = ?1", [name], |r| r.get(0))
            .optional()
    }

    /// The password of the user by this name, to check a sign-in against.
    pub fn credentials(&self, name: &str) -> rusqlite::Result<Option<Credentials>> {
        self.db
            .query_row(
                "SELECT u.id, i.secret, u.disabled FROM users u
                 JOIN identities i ON i.user_id = u.id AND i.provider = 'password'
                 WHERE u.name = ?1",
                [name],
                |r| {
                    Ok(Credentials {
                        user_id: r.get(0)?,
                        hash: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                        disabled: r.get(2)?,
                    })
                },
            )
            .optional()
    }

    /// A new password for a user; their sessions end, wherever they were signed in.
    pub fn set_password(&self, user_id: i64, password_hash: &str) -> rusqlite::Result<()> {
        let changed = self.db.execute(
            "UPDATE identities SET secret = ?2 WHERE user_id = ?1 AND provider = 'password'",
            params![user_id, password_hash],
        )?;
        if changed == 0 {
            self.db.execute(
                "INSERT INTO identities (user_id, provider, subject, secret, created_at)
                 VALUES (?1, 'password', ?2, ?3, ?4)",
                params![user_id, user_id.to_string(), password_hash, now()],
            )?;
        }
        self.db
            .execute("DELETE FROM sessions WHERE user_id = ?1", [user_id])?;
        Ok(())
    }

    /// Puts a user in a space with a role (or changes their role there).
    pub fn set_membership(&self, user_id: i64, space_id: i64, role: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO memberships (user_id, space_id, role, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (user_id, space_id) DO UPDATE SET role = excluded.role",
            params![user_id, space_id, role, now()],
        )?;
        Ok(())
    }

    /// The first space a user belongs to, if any.
    pub fn first_space_of(&self, user_id: i64) -> rusqlite::Result<Option<i64>> {
        self.db
            .query_row(
                "SELECT space_id FROM memberships WHERE user_id = ?1 ORDER BY created_at, space_id LIMIT 1",
                [user_id],
                |r| r.get(0),
            )
            .optional()
    }

    pub fn rename_space(&self, space_id: i64, name: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE spaces SET name = ?2 WHERE id = ?1",
            params![space_id, name],
        )?;
        Ok(())
    }

    /// A new session for a user; the token for their cookie (only its hash is kept).
    pub fn create_session(
        &self,
        user_id: i64,
        space_id: Option<i64>,
        user_agent: &str,
        address: &str,
    ) -> rusqlite::Result<String> {
        let token = random_token();
        let at = now();
        let user_agent: String = user_agent.chars().take(200).collect();
        self.db.execute(
            "INSERT INTO sessions (token_hash, user_id, space_id, created_at, last_seen, expires_at, user_agent, address)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7)",
            params![token_hash(&token), user_id, space_id, at, at + SESSION_SECONDS, user_agent, address],
        )?;
        Ok(token)
    }

    /// Who a session token belongs to, while it lasts and its user may sign in. Keeps it
    /// going: each use moves its end on (written at most every few minutes).
    pub fn session(&self, token: &str) -> rusqlite::Result<Option<Signed>> {
        let at = now();
        let found = self
            .db
            .query_row(
                "SELECT s.id, s.last_seen, u.id, u.name, u.display_name, u.instance_admin, s.space_id
                 FROM sessions s JOIN users u ON u.id = s.user_id
                 WHERE s.token_hash = ?1 AND s.expires_at > ?2 AND u.disabled = 0",
                params![token_hash(token), at],
                |r| {
                    Ok((
                        r.get::<_, i64>(1)?,
                        r.get::<_, Option<i64>>(6)?,
                        Signed {
                            session_id: r.get(0)?,
                            user_id: r.get(2)?,
                            name: r.get(3)?,
                            display_name: r.get(4)?,
                            instance_admin: r.get(5)?,
                            space_id: None,
                        },
                    ))
                },
            )
            .optional()?;
        let Some((last_seen, space, mut signed)) = found else {
            return Ok(None);
        };
        if at - last_seen >= SEEN_EVERY {
            self.db.execute(
                "UPDATE sessions SET last_seen = ?2, expires_at = ?3 WHERE id = ?1",
                params![signed.session_id, at, at + SESSION_SECONDS],
            )?;
        }
        signed.space_id = space;
        Ok(Some(signed))
    }

    pub fn end_session(&self, session_id: i64) -> rusqlite::Result<()> {
        self.db
            .execute("DELETE FROM sessions WHERE id = ?1", [session_id])?;
        Ok(())
    }

    /// Forgets sessions that ran out.
    pub fn end_old_sessions(&self) -> rusqlite::Result<usize> {
        self.db
            .execute("DELETE FROM sessions WHERE expires_at <= ?1", [now()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_are_hashed_and_checked() {
        let hash = hash_password("correct horse battery");
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery", &hash));
        assert!(!verify_password("correct horse batterY", &hash));
        assert!(!verify_password("anything", "not a hash"));
        assert_ne!(hash, hash_password("correct horse battery"), "salted");
    }

    #[test]
    fn names_and_passwords_are_checked() {
        assert!(name_problem("timo").is_none());
        assert!(name_problem("m.abdi_2").is_none());
        for bad in ["t", "with space", "ünïcode", "a/b", &"x".repeat(41)] {
            assert!(name_problem(bad).is_some(), "{bad}");
        }
        assert!(password_problem("short").is_some());
        assert!(password_problem("long enough!").is_none());
    }

    #[test]
    fn a_session_is_found_by_its_token_until_it_ends() {
        let store = Store::open(":memory:").unwrap();
        assert!(!store.has_users().unwrap());
        let id = store
            .create_user("Timo", "Timo", &hash_password("a fine password"), true)
            .unwrap();
        assert!(store.has_users().unwrap());
        assert_eq!(
            store.user_id("timo").unwrap(),
            Some(id),
            "names ignore case"
        );
        let token = store.create_session(id, None, "test", "127.0.0.1").unwrap();
        assert_eq!(token.len(), 64);
        let signed = store.session(&token).unwrap().unwrap();
        assert_eq!((signed.user_id, signed.name.as_str()), (id, "Timo"));
        assert!(store.session("not the token").unwrap().is_none());
        // Only the hash is kept.
        let kept: i64 = store
            .db
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE token_hash = ?1",
                [&token],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, 0);
        // A new password ends every session.
        store
            .set_password(id, &hash_password("another password"))
            .unwrap();
        assert!(store.session(&token).unwrap().is_none());
        // A disabled user's sessions stop working.
        let token = store.create_session(id, None, "", "").unwrap();
        store
            .db
            .execute("UPDATE users SET disabled = 1", [])
            .unwrap();
        assert!(store.session(&token).unwrap().is_none());
    }

    #[test]
    fn an_expired_session_is_not_found() {
        let store = Store::open(":memory:").unwrap();
        let id = store.create_user("ann", "Ann", "x", false).unwrap();
        let token = store.create_session(id, None, "", "").unwrap();
        store
            .db
            .execute("UPDATE sessions SET expires_at = 1", [])
            .unwrap();
        assert!(store.session(&token).unwrap().is_none());
        assert_eq!(store.end_old_sessions().unwrap(), 1);
    }

    #[test]
    fn setup_codes_read_well() {
        let code = setup_code();
        assert_eq!(code.len(), 9);
        assert_eq!(&code[4..5], "-");
        assert!(!code.contains(['0', 'O', '1', 'I']));
    }
}
