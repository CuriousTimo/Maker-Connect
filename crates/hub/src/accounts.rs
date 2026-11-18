//! Accounts: people, how they sign in, which spaces they belong to, and their sessions.
//!
//! Built to go from one person on a Pi to a hosted service with many spaces:
//!
//! - A **user** belongs to the instance, not to a space; **memberships** put them in spaces,
//!   with a role in each. Being an instance admin (running the server) is separate from
//!   owning a space, and does not by itself show a space's data.
//! - How someone signs in is an **identity** of theirs: a password now, an OpenID Connect
//!   provider later, both leading to the same user.
//! - Everything lives in the database, nothing only in memory, so several hub processes
//!   can share it later. Secrets are kept only as hashes: passwords with Argon2id, session
//!   tokens with SHA-256 (they are long and random, so a fast hash is enough).

use crate::store::{Store, now};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use rand::RngCore;
use rusqlite::Connection;
use serde::Serialize;

mod tenancy;
use sha2::{Digest, Sha256};

/// A session lasts this long after it was last used.
pub const SESSION_SECONDS: i64 = 30 * 24 * 3600;
/// `last_seen` is written at most this often, so reading pages does not write every time.
const SEEN_EVERY: i64 = 300;
/// The shortest password taken.
pub const MIN_PASSWORD: usize = 10;

/// Transitional (lanes 1-4): the access tables still on SQLite (crate::access; lane 2 moves them). The
/// accounts' own tables are on Postgres (`accounts/migrations/`).
pub fn migrate(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS machine_grants (
             space_id INTEGER NOT NULL,
             machine_id TEXT NOT NULL,
             user_id INTEGER NOT NULL,
             level TEXT NOT NULL,
             granted_by INTEGER,
             granted_at INTEGER NOT NULL,
             expires_at INTEGER,
             PRIMARY KEY (machine_id, user_id)
         );
         -- Groups of machines (3D printers, say): permission on a group is permission on
         -- every machine in it, also on those added later.
         CREATE TABLE IF NOT EXISTS machine_groups (
             id INTEGER PRIMARY KEY,
             space_id INTEGER NOT NULL,
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
             space_id INTEGER NOT NULL,
             user_id INTEGER NOT NULL,
             machine_id TEXT NOT NULL,
             asked_at INTEGER NOT NULL,
             state TEXT NOT NULL DEFAULT 'open',
             handled_by INTEGER,
             handled_at INTEGER
         );
         CREATE TABLE IF NOT EXISTS group_grants (
             group_id INTEGER NOT NULL REFERENCES machine_groups(id),
             user_id INTEGER NOT NULL,
             level TEXT NOT NULL,
             granted_by INTEGER,
             granted_at INTEGER NOT NULL,
             expires_at INTEGER,
             PRIMARY KEY (group_id, user_id)
         );",
    )
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
    pub async fn has_users(&self) -> sqlx::Result<bool> {
        sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users) AS "e!""#)
            .fetch_one(&self.pg().admin)
            .await
    }

    /// A new user who signs in with a password (already hashed).
    pub async fn create_user(
        &self,
        name: &str,
        display_name: &str,
        password_hash: &str,
        instance_admin: bool,
    ) -> sqlx::Result<i64> {
        let mut tx = self.pg().admin.begin().await?;
        let at = now();
        let id = sqlx::query_scalar!(
            "INSERT INTO users (name, display_name, instance_admin, created_at) VALUES ($1, $2, $3, $4)
             RETURNING id",
            name,
            display_name,
            instance_admin,
            at
        )
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query!(
            "INSERT INTO identities (user_id, provider, subject, secret, created_at)
             VALUES ($1, 'password', $2, $3, $4)",
            id,
            id.to_string(),
            password_hash,
            at
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Whether a user by this name exists (names are the same whatever their case).
    pub async fn user_id(&self, name: &str) -> sqlx::Result<Option<i64>> {
        sqlx::query_scalar!("SELECT id FROM users WHERE lower(name) = lower($1)", name)
            .fetch_optional(&self.pg().admin)
            .await
    }

    /// The password of the user by this name, to check a sign-in against.
    pub async fn credentials(&self, name: &str) -> sqlx::Result<Option<Credentials>> {
        let row = sqlx::query!(
            "SELECT u.id, i.secret, u.disabled FROM users u
             JOIN identities i ON i.user_id = u.id AND i.provider = 'password'
             WHERE lower(u.name) = lower($1)",
            name
        )
        .fetch_optional(&self.pg().admin)
        .await?;
        Ok(row.map(|r| Credentials {
            user_id: r.id,
            hash: r.secret.unwrap_or_default(),
            disabled: r.disabled,
        }))
    }

    /// A new password for a user; their sessions end, wherever they were signed in.
    pub async fn set_password(&self, user_id: i64, password_hash: &str) -> sqlx::Result<()> {
        let mut tx = self.pg().admin.begin().await?;
        let changed = sqlx::query!(
            "UPDATE identities SET secret = $2 WHERE user_id = $1 AND provider = 'password'",
            user_id,
            password_hash
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed == 0 {
            sqlx::query!(
                "INSERT INTO identities (user_id, provider, subject, secret, created_at)
                 VALUES ($1, 'password', $2, $3, $4)",
                user_id,
                user_id.to_string(),
                password_hash,
                now()
            )
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query!("DELETE FROM sessions WHERE user_id = $1", user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    /// Puts a user in a space with a role (or changes their role there).
    pub async fn set_membership(&self, user_id: i64, space_id: i64, role: &str) -> sqlx::Result<()> {
        let mut tx = self.pg().space(space_id).await?;
        sqlx::query!(
            "INSERT INTO memberships (user_id, space_id, role, created_at) VALUES ($1, $2, $3, $4)
             ON CONFLICT (user_id, space_id) DO UPDATE SET role = excluded.role",
            user_id,
            space_id,
            role,
            now()
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }

    /// The first space a user belongs to, if any.
    pub async fn first_space_of(&self, user_id: i64) -> sqlx::Result<Option<i64>> {
        sqlx::query_scalar!(
            "SELECT space_id FROM memberships WHERE user_id = $1 ORDER BY created_at, space_id LIMIT 1",
            user_id
        )
        .fetch_optional(&self.pg().admin)
        .await
    }

    pub async fn rename_space(&self, space_id: i64, name: &str) -> sqlx::Result<()> {
        let mut tx = self.pg().space(space_id).await?;
        sqlx::query!("UPDATE spaces SET name = $2 WHERE id = $1", space_id, name)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    /// A new session for a user; the token for their cookie (only its hash is kept).
    pub async fn create_session(
        &self,
        user_id: i64,
        space_id: Option<i64>,
        user_agent: &str,
        address: &str,
    ) -> sqlx::Result<String> {
        let token = random_token();
        let at = now();
        let user_agent: String = user_agent.chars().take(200).collect();
        sqlx::query!(
            "INSERT INTO sessions (token_hash, user_id, space_id, created_at, last_seen, expires_at, user_agent, address)
             VALUES ($1, $2, $3, $4, $4, $5, $6, $7)",
            token_hash(&token),
            user_id,
            space_id,
            at,
            at + SESSION_SECONDS,
            user_agent,
            address
        )
        .execute(&self.pg().admin)
        .await?;
        Ok(token)
    }

    /// Who a session token belongs to, while it lasts and its user may sign in. Keeps it
    /// going: each use moves its end on (written at most every few minutes).
    pub async fn session(&self, token: &str) -> sqlx::Result<Option<Signed>> {
        let at = now();
        let found = sqlx::query!(
            "SELECT s.id, s.last_seen, u.id AS user_id, u.name, u.display_name, u.instance_admin, s.space_id
             FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = $1 AND s.expires_at > $2 AND NOT u.disabled",
            token_hash(token),
            at
        )
        .fetch_optional(&self.pg().admin)
        .await?;
        let Some(r) = found else {
            return Ok(None);
        };
        if at - r.last_seen >= SEEN_EVERY {
            sqlx::query!(
                "UPDATE sessions SET last_seen = $2, expires_at = $3 WHERE id = $1",
                r.id,
                at,
                at + SESSION_SECONDS
            )
            .execute(&self.pg().admin)
            .await?;
        }
        Ok(Some(Signed {
            session_id: r.id,
            user_id: r.user_id,
            name: r.name,
            display_name: r.display_name,
            instance_admin: r.instance_admin,
            space_id: r.space_id,
        }))
    }

    pub async fn end_session(&self, session_id: i64) -> sqlx::Result<()> {
        sqlx::query!("DELETE FROM sessions WHERE id = $1", session_id)
            .execute(&self.pg().admin)
            .await?;
        Ok(())
    }

    /// Forgets sessions that ran out.
    pub async fn end_old_sessions(&self) -> sqlx::Result<u64> {
        Ok(sqlx::query!("DELETE FROM sessions WHERE expires_at <= $1", now())
            .execute(&self.pg().admin)
            .await?
            .rows_affected())
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

    #[cfg(feature = "pg-tests")]
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    async fn a_session_is_found_by_its_token_until_it_ends(pool: sqlx::PgPool) {
        let store = Store::for_test(&pool).await;
        assert!(!store.has_users().await.unwrap());
        let id = store
            .create_user("Timo", "Timo", &hash_password("a fine password"), true)
            .await
            .unwrap();
        assert!(store.has_users().await.unwrap());
        assert_eq!(
            store.user_id("timo").await.unwrap(),
            Some(id),
            "names ignore case"
        );
        let token = store
            .create_session(id, None, "test", "127.0.0.1")
            .await
            .unwrap();
        assert_eq!(token.len(), 64);
        let signed = store.session(&token).await.unwrap().unwrap();
        assert_eq!((signed.user_id, signed.name.as_str()), (id, "Timo"));
        assert!(store.session("not the token").await.unwrap().is_none());
        // Only the hash is kept.
        let kept: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE token_hash = $1")
                .bind(&token)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(kept, 0);
        // A new password ends every session.
        store
            .set_password(id, &hash_password("another password"))
            .await
            .unwrap();
        assert!(store.session(&token).await.unwrap().is_none());
        // A disabled user's sessions stop working.
        let token = store.create_session(id, None, "", "").await.unwrap();
        sqlx::query("UPDATE users SET disabled = true")
            .execute(&pool)
            .await
            .unwrap();
        assert!(store.session(&token).await.unwrap().is_none());
    }

    #[cfg(feature = "pg-tests")]
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    async fn an_expired_session_is_not_found(pool: sqlx::PgPool) {
        let store = Store::for_test(&pool).await;
        let id = store.create_user("ann", "Ann", "x", false).await.unwrap();
        let token = store.create_session(id, None, "", "").await.unwrap();
        sqlx::query("UPDATE sessions SET expires_at = 1")
            .execute(&pool)
            .await
            .unwrap();
        assert!(store.session(&token).await.unwrap().is_none());
        assert_eq!(store.end_old_sessions().await.unwrap(), 1);
    }

    #[test]
    fn setup_codes_read_well() {
        let code = setup_code();
        assert_eq!(code.len(), 9);
        assert_eq!(&code[4..5], "-");
        assert!(!code.contains(['0', 'O', '1', 'I']));
    }
}
