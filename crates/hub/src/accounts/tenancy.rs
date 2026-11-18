//! Organisations, spaces, memberships, invites, API tokens and the audit log, on Postgres (moved from
//! crate::access's SQLite queries, hub core v2 step 2). Request work for one space runs in that space's
//! transaction ([`crate::store::pg::Pg::space`]); what crosses spaces by nature (who someone is and where
//! they may be, sign-in by invite or token, instance admin, organisation-wide changes) runs as admin.
//!
//! Transitional (lanes 1-4): the access tables still on SQLite (grants, groups, requests) are reached
//! through [`Store::lock`]; names of people come from here.

use crate::access::INVITE_SECONDS;
use crate::access::{Access, AuditLine, Invite, Member, Role, SpaceOf, TOKEN_PREFIX, Token};
use crate::accounts::{random_token, token_hash};
use crate::store::{Store, now};
use std::collections::HashMap;

impl Store {
    /// The space a request acts in, with its name: from who asks.
    pub async fn space_of(&self, access: &Access) -> sqlx::Result<(i64, String)> {
        Ok((access.space, self.space_name(access.space).await?))
    }

    /// Someone's role in a space: the higher of their role in its organisation (which
    /// counts in all its spaces) and in the space itself.
    pub async fn role_of(&self, user: i64, space: i64) -> sqlx::Result<Option<Role>> {
        let roles = sqlx::query_scalar!(
            r#"SELECT role AS "role!" FROM memberships WHERE user_id = $1 AND space_id = $2
               UNION ALL
               SELECT o.role FROM org_memberships o JOIN spaces s ON s.organisation_id = o.organisation_id
               WHERE o.user_id = $1 AND s.id = $2"#,
            user,
            space
        )
        .fetch_all(&self.pg().admin)
        .await?;
        Ok(roles.iter().filter_map(|r| Role::parse(r)).max())
    }

    /// Someone's role in an organisation itself (not through one of its spaces).
    pub async fn org_role_of(&self, user: i64, organisation: i64) -> sqlx::Result<Option<Role>> {
        Ok(sqlx::query_scalar!(
            "SELECT role FROM org_memberships WHERE user_id = $1 AND organisation_id = $2",
            user,
            organisation
        )
        .fetch_optional(&self.pg().admin)
        .await?
        .and_then(|r| Role::parse(&r)))
    }

    pub async fn organisation_of(&self, space: i64) -> sqlx::Result<i64> {
        sqlx::query_scalar!("SELECT organisation_id FROM spaces WHERE id = $1", space)
            .fetch_one(&self.pg().admin)
            .await
    }

    pub async fn organisation_name(&self, organisation: i64) -> sqlx::Result<String> {
        sqlx::query_scalar!("SELECT name FROM organisations WHERE id = $1", organisation)
            .fetch_one(&self.pg().admin)
            .await
    }

    pub async fn rename_organisation(&self, organisation: i64, name: &str) -> sqlx::Result<()> {
        sqlx::query!(
            "UPDATE organisations SET name = $2 WHERE id = $1",
            organisation,
            name
        )
        .execute(&self.pg().admin)
        .await?;
        Ok(())
    }

    /// Puts a user in an organisation (all its spaces) with a role, or changes it.
    pub async fn set_org_membership(
        &self,
        user: i64,
        organisation: i64,
        role: &str,
    ) -> sqlx::Result<()> {
        sqlx::query!(
            "INSERT INTO org_memberships (user_id, organisation_id, role, created_at) VALUES ($1, $2, $3, at_unix($4))
             ON CONFLICT (user_id, organisation_id) DO UPDATE SET role = excluded.role",
            user,
            organisation,
            role,
            now()
        )
        .execute(&self.pg().admin)
        .await?;
        Ok(())
    }

    /// The spaces a user can be in (through an organisation or of their own), by
    /// organisation and name, with their role in each. SQLite's `COLLATE NOCASE` folded ASCII letters
    /// only, `lower()` folds all: names with other letters may sort otherwise (accepted,
    /// journal:V2-STORE-1:0008); ties by id, as before.
    pub async fn spaces_of(&self, user: i64) -> sqlx::Result<Vec<SpaceOf>> {
        let rows = sqlx::query!(
            r#"SELECT s.id, s.uid, s.name, o.name AS organisation, r.role AS "role!" FROM (
                   SELECT space_id AS id, role FROM memberships WHERE user_id = $1
                   UNION ALL
                   SELECT s.id, m.role FROM org_memberships m JOIN spaces s ON s.organisation_id = m.organisation_id
                   WHERE m.user_id = $1
               ) r JOIN spaces s ON s.id = r.id JOIN organisations o ON o.id = s.organisation_id
               ORDER BY lower(o.name), lower(s.name), s.id"#,
            user
        )
        .fetch_all(&self.pg().admin)
        .await?;
        let mut out: Vec<SpaceOf> = Vec::new();
        for r in rows {
            let Some(role) = Role::parse(&r.role) else {
                continue;
            };
            match out.iter_mut().find(|s| s.id == r.id) {
                Some(seen) => seen.role = seen.role.max(role),
                None => out.push(SpaceOf {
                    id: r.id,
                    uid: r.uid,
                    name: r.name,
                    organisation: r.organisation,
                    role,
                }),
            }
        }
        Ok(out)
    }

    pub async fn space_name(&self, space: i64) -> sqlx::Result<String> {
        let mut tx = self.pg().space(space).await?;
        let name = sqlx::query_scalar!("SELECT name FROM spaces WHERE id = $1", space)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(name)
    }

    /// A space by its id under the hood.
    pub async fn space_by_uid(&self, uid: &str) -> sqlx::Result<Option<i64>> {
        sqlx::query_scalar!("SELECT id FROM spaces WHERE uid = $1", uid)
            .fetch_optional(&self.pg().admin)
            .await
    }

    pub async fn space_uid(&self, space: i64) -> sqlx::Result<String> {
        sqlx::query_scalar!("SELECT uid FROM spaces WHERE id = $1", space)
            .fetch_one(&self.pg().admin)
            .await
    }

    /// The space a self-hosted hub shows: its first.
    pub async fn default_space(&self) -> sqlx::Result<(i64, String)> {
        let r = sqlx::query!("SELECT id, name FROM spaces ORDER BY id LIMIT 1")
            .fetch_one(&self.pg().admin)
            .await?;
        Ok((r.id, r.name))
    }

    /// Every space's id (the hub's own tasks that go over all spaces).
    pub async fn space_ids(&self) -> sqlx::Result<Vec<i64>> {
        sqlx::query_scalar!("SELECT id FROM spaces ORDER BY id")
            .fetch_all(&self.pg().admin)
            .await
    }

    /// A new space in an organisation.
    pub async fn create_space(&self, organisation: i64, name: &str) -> sqlx::Result<i64> {
        let space = sqlx::query_scalar!(
            "INSERT INTO spaces (name, uid, organisation_id) VALUES ($1, $2, $3) RETURNING id",
            name,
            crate::accounts::new_uuid(),
            organisation
        )
        .fetch_one(&self.pg().admin)
        .await?;
        // Transitional (lanes 1-4): the SQLite tables still point at their space by id.
        self.lock()
            .unwrap()
            .db
            .execute(
                "INSERT OR IGNORE INTO spaces (id, name) VALUES (?1, ?2)",
                rusqlite::params![space, name],
            )
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        Ok(space)
    }

    /// A new organisation with its first space, its maker the owner of both.
    pub async fn create_organisation(
        &self,
        name: &str,
        space_name: &str,
        owner: i64,
    ) -> sqlx::Result<(i64, i64)> {
        let organisation = sqlx::query_scalar!(
            "INSERT INTO organisations (uid, name, created_at) VALUES ($1, $2, at_unix($3)) RETURNING id",
            crate::accounts::new_uuid(),
            name,
            now()
        )
        .fetch_one(&self.pg().admin)
        .await?;
        self.set_org_membership(owner, organisation, Role::Owner.as_str())
            .await?;
        let space = self.create_space(organisation, space_name).await?;
        Ok((organisation, space))
    }

    /// The session goes on in another space.
    pub async fn set_session_space(&self, session: i64, space: i64) -> sqlx::Result<()> {
        sqlx::query!(
            "UPDATE sessions SET space_id = $2 WHERE id = $1",
            session,
            space
        )
        .execute(&self.pg().admin)
        .await?;
        Ok(())
    }

    /// Everyone who can be in a space: the organisation's members (in all its spaces), and
    /// those of this space only; each with the role that counts here.
    pub async fn members(&self, space: i64) -> sqlx::Result<Vec<Member>> {
        let mut tx = self.pg().space(space).await?;
        let rows = sqlx::query!(
            r#"SELECT u.id AS "id!", u.name AS "name!", u.display_name AS "display_name!",
                   m.role AS "role!", unix_seconds(m.created_at) AS "created_at!", true AS "whole!"
               FROM org_memberships m
               JOIN users u ON u.id = m.user_id JOIN spaces s ON s.organisation_id = m.organisation_id
               WHERE s.id = $1
               UNION ALL
               SELECT u.id, u.name, u.display_name, m.role, unix_seconds(m.created_at), false FROM memberships m
               JOIN users u ON u.id = m.user_id WHERE m.space_id = $1"#,
            space
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let mut out: Vec<Member> = Vec::new();
        for r in rows {
            let Some(role) = Role::parse(&r.role) else {
                continue;
            };
            let whole = r.whole;
            match out.iter_mut().find(|m| m.user_id == r.id) {
                Some(seen) => {
                    if whole {
                        seen.org_role = Some(role);
                    } else {
                        seen.space_role = Some(role);
                    }
                    seen.role = seen.role.max(role);
                }
                None => out.push(Member {
                    user_id: r.id,
                    name: r.name,
                    display_name: r.display_name,
                    role,
                    org_role: whole.then_some(role),
                    space_role: (!whole).then_some(role),
                    since: r.created_at,
                }),
            }
        }
        // Ties by id, as SQLite gave them (in the order they were made).
        out.sort_by_key(|m| (m.display_name.to_lowercase(), m.user_id));
        Ok(out)
    }

    /// Owners of the organisation a space is in.
    pub async fn owners(&self, space: i64) -> sqlx::Result<i64> {
        let mut tx = self.pg().space(space).await?;
        let n = sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "n!" FROM org_memberships m JOIN spaces s ON s.organisation_id = m.organisation_id
               WHERE s.id = $1 AND m.role = 'owner'"#,
            space
        )
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(n)
    }

    /// Takes someone out of a space (`whole`: out of its organisation, so of all its
    /// spaces), with their inductions there; their sessions find another space.
    pub async fn remove_member(&self, space: i64, user: i64, whole: bool) -> sqlx::Result<bool> {
        let mut tx = self.pg().admin.begin().await?;
        let (n, spaces) = if whole {
            let organisation =
                sqlx::query_scalar!("SELECT organisation_id FROM spaces WHERE id = $1", space)
                    .fetch_one(&mut *tx)
                    .await?;
            let n = sqlx::query!(
                "DELETE FROM org_memberships WHERE organisation_id = $1 AND user_id = $2",
                organisation,
                user
            )
            .execute(&mut *tx)
            .await?
            .rows_affected();
            let spaces = sqlx::query_scalar!(
                "SELECT id FROM spaces WHERE organisation_id = $1",
                organisation
            )
            .fetch_all(&mut *tx)
            .await?;
            (n, spaces)
        } else {
            let n = sqlx::query!(
                "DELETE FROM memberships WHERE space_id = $1 AND user_id = $2",
                space,
                user
            )
            .execute(&mut *tx)
            .await?
            .rows_affected();
            (n, vec![space])
        };
        sqlx::query!(
            "UPDATE sessions SET space_id = NULL WHERE user_id = $1",
            user
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        // Transitional: their inductions, still on SQLite (crate::access).
        self.lock()
            .unwrap()
            .remove_grants_of(user, &spaces)
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        Ok(n > 0)
    }

    /// People's display names, by id (for lists whose rows are still on SQLite: transitional).
    pub async fn display_names(&self, ids: &[i64]) -> sqlx::Result<HashMap<i64, String>> {
        let rows = sqlx::query!("SELECT id, display_name FROM users WHERE id = ANY($1)", ids)
            .fetch_all(&self.pg().admin)
            .await?;
        Ok(rows.into_iter().map(|r| (r.id, r.display_name)).collect())
    }

    /// What the audit log says of a person and a machine: their names.
    pub async fn names_for_log(
        &self,
        user: Option<i64>,
        machine: Option<&str>,
    ) -> (String, String) {
        let user = match user {
            Some(u) => self
                .display_names(&[u])
                .await
                .ok()
                .and_then(|mut n| n.remove(&u))
                .unwrap_or_default(),
            None => String::new(),
        };
        let machine = machine
            .and_then(|m| self.lock().unwrap().machine_name_any_space(m))
            .unwrap_or_default();
        (user, machine)
    }

    /// A new invite link into a space; the token for its address (only its hash is kept).
    pub async fn create_invite(
        &self,
        space: i64,
        role: Role,
        by: i64,
        whole_organisation: bool,
    ) -> sqlx::Result<String> {
        let token = random_token();
        let mut tx = self.pg().space(space).await?;
        sqlx::query!(
            "INSERT INTO invites (token_hash, space_id, role, created_by, created_at, expires_at, whole_organisation)
             VALUES ($1, $2, $3, $4, at_unix($5), at_unix($6), $7)",
            token_hash(&token),
            space,
            role.as_str(),
            by,
            now(),
            now() + INVITE_SECONDS,
            whole_organisation
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(token)
    }

    /// A space's invite links, newest first.
    pub async fn invites(&self, space: i64) -> sqlx::Result<Vec<Invite>> {
        let mut tx = self.pg().space(space).await?;
        let rows = sqlx::query!(
            r#"SELECT i.id, i.space_id, s.name AS space_name, i.role, b.display_name AS by_name,
                 unix_seconds(i.created_at) AS "created_at!", unix_seconds(i.expires_at) AS "expires_at!",
                 u.display_name AS "used_by?", i.revoked, i.whole_organisation
             FROM invites i JOIN spaces s ON s.id = i.space_id JOIN users b ON b.id = i.created_by
             LEFT JOIN users u ON u.id = i.used_by WHERE i.space_id = $1 ORDER BY i.created_at DESC, i.id DESC"#,
            space
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                Some(Invite {
                    id: r.id,
                    space: r.space_id,
                    space_name: r.space_name,
                    role: Role::parse(&r.role)?,
                    by: r.by_name,
                    created_at: r.created_at,
                    expires_at: r.expires_at,
                    used_by: r.used_by,
                    revoked: r.revoked,
                    whole_organisation: r.whole_organisation,
                })
            })
            .collect())
    }

    /// The invite an address's token is for, open or not (sign-in by invite: admin).
    pub async fn invite(&self, token: &str) -> sqlx::Result<Option<Invite>> {
        let r = sqlx::query!(
            r#"SELECT i.id, i.space_id, s.name AS space_name, i.role, b.display_name AS by_name,
                 unix_seconds(i.created_at) AS "created_at!", unix_seconds(i.expires_at) AS "expires_at!",
                 u.display_name AS "used_by?", i.revoked, i.whole_organisation
             FROM invites i JOIN spaces s ON s.id = i.space_id JOIN users b ON b.id = i.created_by
             LEFT JOIN users u ON u.id = i.used_by WHERE i.token_hash = $1"#,
            token_hash(token)
        )
        .fetch_optional(&self.pg().admin)
        .await?;
        Ok(r.and_then(|r| {
            Some(Invite {
                id: r.id,
                space: r.space_id,
                space_name: r.space_name,
                role: Role::parse(&r.role)?,
                by: r.by_name,
                created_at: r.created_at,
                expires_at: r.expires_at,
                used_by: r.used_by,
                revoked: r.revoked,
                whole_organisation: r.whole_organisation,
            })
        }))
    }

    pub async fn revoke_invite(&self, space: i64, id: i64) -> sqlx::Result<bool> {
        let mut tx = self.pg().space(space).await?;
        let n = sqlx::query!(
            "UPDATE invites SET revoked = true WHERE id = $1 AND space_id = $2",
            id,
            space
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n > 0)
    }

    /// Uses an open invite: the user joins its space with its role (or keeps a higher one).
    /// False when it was taken meanwhile.
    pub async fn use_invite(&self, invite: &Invite, user: i64) -> sqlx::Result<bool> {
        let taken = sqlx::query!(
            "UPDATE invites SET used_by = $2, used_at = at_unix($3)
             WHERE id = $1 AND used_by IS NULL AND NOT revoked AND expires_at > at_unix($3)",
            invite.id,
            user,
            now()
        )
        .execute(&self.pg().admin)
        .await?
        .rows_affected();
        if taken == 0 {
            return Ok(false);
        }
        if invite.whole_organisation {
            let organisation = self.organisation_of(invite.space).await?;
            if self
                .org_role_of(user, organisation)
                .await?
                .is_none_or(|r| r < invite.role)
            {
                self.set_org_membership(user, organisation, invite.role.as_str())
                    .await?;
            }
        } else if self
            .role_of(user, invite.space)
            .await?
            .is_none_or(|r| r < invite.role)
        {
            self.set_membership(user, invite.space, invite.role.as_str())
                .await?;
        }
        Ok(true)
    }

    /// A new API token of a space; the secret, to show once (only its hash is kept).
    pub async fn create_token(
        &self,
        space: i64,
        by: i64,
        name: &str,
        role: Role,
    ) -> sqlx::Result<String> {
        let token = format!("{TOKEN_PREFIX}{}", random_token());
        let mut tx = self.pg().space(space).await?;
        sqlx::query!(
            "INSERT INTO api_tokens (token_hash, space_id, created_by, name, role, created_at)
             VALUES ($1, $2, $3, $4, $5, at_unix($6))",
            token_hash(&token),
            space,
            by,
            name,
            role.as_str(),
            now()
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(token)
    }

    pub async fn tokens(&self, space: i64) -> sqlx::Result<Vec<Token>> {
        let mut tx = self.pg().space(space).await?;
        let rows = sqlx::query!(
            r#"SELECT t.id, t.name, t.role, u.display_name, unix_seconds(t.created_at) AS "created_at!",
                 unix_seconds(t.last_used) AS "last_used?"
             FROM api_tokens t JOIN users u ON u.id = t.created_by
             WHERE t.space_id = $1 AND NOT t.revoked ORDER BY t.created_at DESC, t.id DESC"#,
            space
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                Some(Token {
                    id: r.id,
                    name: r.name,
                    role: Role::parse(&r.role)?,
                    by: r.display_name,
                    created_at: r.created_at,
                    last_used: r.last_used,
                })
            })
            .collect())
    }

    pub async fn revoke_token(&self, space: i64, id: i64) -> sqlx::Result<bool> {
        let mut tx = self.pg().space(space).await?;
        let n = sqlx::query!(
            "UPDATE api_tokens SET revoked = true WHERE id = $1 AND space_id = $2",
            id,
            space
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n > 0)
    }

    /// Who an API token acts for: its space, its role (never more than its maker has there
    /// now; a maker who left the space takes the token's rights with them).
    pub async fn token_access(&self, token: &str) -> sqlx::Result<Option<Access>> {
        let found = sqlx::query!(
            r#"SELECT t.id, t.space_id, t.role, t.name, t.created_by, unix_seconds(t.last_used) AS "last_used?"
             FROM api_tokens t
             JOIN users u ON u.id = t.created_by AND NOT u.disabled
             WHERE t.token_hash = $1 AND NOT t.revoked"#,
            token_hash(token)
        )
        .fetch_optional(&self.pg().admin)
        .await?;
        let Some(r) = found else {
            return Ok(None);
        };
        // The maker's role there now, through the organisation or the space.
        let (Some(role), Some(maker_role)) = (
            Role::parse(&r.role),
            self.role_of(r.created_by, r.space_id).await?,
        ) else {
            return Ok(None);
        };
        if r.last_used.is_none_or(|t| now() - t >= 60) {
            sqlx::query!(
                "UPDATE api_tokens SET last_used = at_unix($2) WHERE id = $1",
                r.id,
                now()
            )
            .execute(&self.pg().admin)
            .await?;
        }
        Ok(Some(Access {
            user_id: r.created_by,
            actor: format!("token \"{}\"", r.name),
            space: r.space_id,
            role: role.min(maker_role),
            token_id: Some(r.id),
            session_id: None,
            instance_admin: false,
        }))
    }

    /// A line in a space's audit log.
    pub async fn audit(
        &self,
        access: &Access,
        action: &str,
        target: &str,
        detail: &str,
    ) -> sqlx::Result<()> {
        let detail: String = detail.chars().take(500).collect();
        let mut tx = self.pg().space(access.space).await?;
        sqlx::query!(
            "INSERT INTO audit_log (space_id, at, user_id, token_id, actor, action, target, detail)
             VALUES ($1, at_unix($2), $3, $4, $5, $6, $7, $8)",
            access.space,
            now(),
            access.user_id,
            access.token_id,
            access.actor,
            action,
            target,
            detail
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }

    /// A space's audit log, newest first; a machine's id as its name where it still has one.
    pub async fn audit_log(&self, space: i64, limit: usize) -> sqlx::Result<Vec<AuditLine>> {
        let mut tx = self.pg().space(space).await?;
        let rows = sqlx::query!(
            r#"SELECT unix_seconds(at) AS "at!", actor, action, target, detail FROM audit_log
             WHERE space_id = $1 ORDER BY at DESC, id DESC LIMIT $2"#,
            space,
            limit as i64
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        // Transitional: machines are still on SQLite.
        let legacy = self.lock().unwrap();
        Ok(rows
            .into_iter()
            .map(|r| AuditLine {
                at: r.at,
                actor: r.actor,
                action: r.action,
                target: legacy.machine_name_in(space, &r.target).unwrap_or(r.target),
                detail: r.detail,
            })
            .collect())
    }

    /// Whether anyone signed in may make a space (a hosted service), or only who runs the
    /// server (the default).
    pub async fn open_spaces(&self) -> bool {
        sqlx::query_scalar!("SELECT value FROM meta WHERE key = 'open_spaces'")
            .fetch_optional(&self.pg().admin)
            .await
            .ok()
            .flatten()
            .is_some_and(|v| v == "1")
    }

    pub async fn set_open_spaces(&self, on: bool) -> sqlx::Result<()> {
        sqlx::query!(
            "INSERT INTO meta (key, value) VALUES ('open_spaces', $1)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            if on { "1" } else { "0" }
        )
        .execute(&self.pg().admin)
        .await?;
        Ok(())
    }

    /// Whether the space asks for PPE before a machine is used (crate::safety).
    pub async fn ppe_checks(&self, space: i64) -> sqlx::Result<bool> {
        let mut tx = self.pg().space(space).await?;
        let on = sqlx::query_scalar!("SELECT ppe_checks FROM spaces WHERE id = $1", space)
            .fetch_optional(&mut *tx)
            .await?
            .unwrap_or(false);
        tx.commit().await?;
        Ok(on)
    }

    pub async fn set_ppe_checks(&self, space: i64, on: bool) -> sqlx::Result<()> {
        let mut tx = self.pg().space(space).await?;
        sqlx::query!("UPDATE spaces SET ppe_checks = $2 WHERE id = $1", space, on)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.changed();
        Ok(())
    }

    /// Spaces that did not get the example files yet (crate::samples).
    pub async fn spaces_without_samples(&self) -> sqlx::Result<Vec<i64>> {
        sqlx::query_scalar!("SELECT id FROM spaces WHERE NOT samples_given ORDER BY id")
            .fetch_all(&self.pg().admin)
            .await
    }

    pub async fn samples_given(&self, space: i64) -> sqlx::Result<()> {
        let mut tx = self.pg().space(space).await?;
        sqlx::query!(
            "UPDATE spaces SET samples_given = true WHERE id = $1",
            space
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }
}

/// Transitional (lanes 1-4): the access lists whose rows are still on SQLite, with people's names
/// from Postgres, in the order SQLite's `ORDER BY display_name COLLATE NOCASE` gave.
impl Store {
    fn name_grants(grants: &mut Vec<crate::access::Grant>, names: &HashMap<i64, String>) {
        for g in grants.iter_mut() {
            g.display_name = names.get(&g.user_id).cloned().unwrap_or_default();
            g.by =
                g.by.as_deref()
                    .and_then(|b| b.parse::<i64>().ok())
                    .and_then(|b| names.get(&b).cloned());
        }
        grants.retain(|g| names.contains_key(&g.user_id));
        // SQLite's NOCASE folds ASCII letters only, as here; ties by id (the order SQLite gave them).
        // Names with other letters may sort otherwise than NOCASE did (accepted, journal:V2-STORE-1:0008).
        grants.sort_by_key(|g| (g.display_name.to_ascii_lowercase(), g.user_id));
    }

    fn grant_people(grants: &[crate::access::Grant]) -> Vec<i64> {
        grants
            .iter()
            .flat_map(|g| {
                std::iter::once(g.user_id).chain(g.by.as_deref().and_then(|b| b.parse().ok()))
            })
            .collect()
    }

    /// Who has an induction on a machine of the space.
    pub async fn grants(
        &self,
        space: i64,
        machine: &str,
    ) -> Result<Vec<crate::access::Grant>, Box<dyn std::error::Error + Send + Sync>> {
        let mut grants = self.lock().unwrap().grants_unnamed(space, machine)?;
        let names = self.display_names(&Self::grant_people(&grants)).await?;
        Self::name_grants(&mut grants, &names);
        Ok(grants)
    }

    /// The machine groups of a space, with their machines and who has permission on them.
    pub async fn groups(
        &self,
        space: i64,
    ) -> Result<Vec<crate::access::Group>, Box<dyn std::error::Error + Send + Sync>> {
        let mut groups = self.lock().unwrap().groups_unnamed(space)?;
        let people: Vec<i64> = groups
            .iter()
            .flat_map(|g| Self::grant_people(&g.grants))
            .collect();
        let names = self.display_names(&people).await?;
        for g in &mut groups {
            Self::name_grants(&mut g.grants, &names);
        }
        Ok(groups)
    }

    /// A space's open requests, oldest first.
    pub async fn requests(
        &self,
        space: i64,
    ) -> Result<Vec<crate::access::AskedFor>, Box<dyn std::error::Error + Send + Sync>> {
        let mut requests = self.lock().unwrap().requests_unnamed(space)?;
        let people: Vec<i64> = requests.iter().map(|r| r.user_id).collect();
        let names = self.display_names(&people).await?;
        for r in &mut requests {
            r.who = names.get(&r.user_id).cloned().unwrap_or_default();
        }
        requests.retain(|r| names.contains_key(&r.user_id));
        Ok(requests)
    }
}
