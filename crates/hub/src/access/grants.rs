//! Permissions on machines, on Postgres (`access/migrations/`): grants per machine, machine groups with
//! their machines and grants, and requests for permission. Moved from crate::access's SQLite queries (hub
//! core v2 step 2, lane 2); each runs in its space's transaction ([`crate::store::pg::Pg::space`]).
//!
//! Where a list shows a machine's name or needs it to be of the space, the space's machines are read with
//! it ([`Store::machine_modes`]); rows whose machine is not in the space are left out, as the joins on
//! SQLite left them out.

use super::{AskedFor, Grant, Group, Level};
use crate::store::{BoxError, Store, now};
use std::collections::HashMap;

/// A member's live permissions in one space, by machine: given on the machine or on a group it is in, the
/// highest counting; expired ones left out. Read before the SQLite lock ([`Store::grants_of`]) and handed
/// to [`crate::store::Legacy::may`] and [`crate::store::Legacy::machine_level`].
///
/// With them, the space's machines and who may use each (`MachineMode`): a machine not among them is not
/// of the space.
#[derive(Debug, Clone, Default)]
pub struct Grants {
    levels: HashMap<String, Level>,
    /// The person's levels were read; not for view and manage ([`Store::grants_for`]).
    read: bool,
    modes: HashMap<String, MachineMode>,
}

impl Grants {
    pub(crate) fn read(&self) -> bool {
        self.read
    }

    pub(crate) fn of(&self, machine: &str) -> Option<Level> {
        self.levels.get(machine).copied()
    }

    /// A machine of the space: its name, kind and who may use it.
    pub(crate) fn machine(&self, machine: &str) -> Option<&MachineMode> {
        self.modes.get(machine)
    }
}

/// A machine of a space as the access rules read it: its name, kind (as kept: JSON, `"fdm"`) and who may
/// use it (`induction_required` 0: every member; 1: with permission; 2: with training).
#[derive(Debug, Clone)]
pub struct MachineMode {
    pub name: String,
    pub kind: String,
    pub induction_required: i32,
}

/// A permission as the Members tab's window sets it: given (level, until) or taken away (`None`).
pub type Given = Option<(Level, Option<i64>)>;

/// SQLite's `COLLATE NOCASE` order: ASCII letters folded, bytes compared.
fn nocase(text: &str) -> Vec<u8> {
    text.as_bytes().to_ascii_lowercase()
}

impl Store {
    /// Someone's permissions on the machines of their space (see [`Grants`]).
    pub async fn grants_of(&self, access: &super::Access) -> sqlx::Result<Grants> {
        let modes = self.machine_modes(access.space).await?;
        let mut tx = self.pg().space(access.space).await?;
        let rows = sqlx::query!(
            r#"SELECT machine_id AS "machine_id!", level AS "level!" FROM machine_grants
               WHERE space_id = $1 AND user_id = $2 AND (expires_at IS NULL OR expires_at > at_unix($3))
               UNION ALL
               SELECT m.machine_id, g.level FROM group_grants g
               JOIN machine_group_members m ON m.group_id = g.group_id
               WHERE g.space_id = $1 AND g.user_id = $2 AND (g.expires_at IS NULL OR g.expires_at > at_unix($3))"#,
            access.space,
            access.user_id,
            now()
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let mut levels: HashMap<String, Level> = HashMap::new();
        for r in rows {
            if let Some(level) = Level::parse(&r.level) {
                let at = levels.entry(r.machine_id).or_insert(level);
                *at = (*at).max(level);
            }
        }
        Ok(Grants {
            levels,
            read: true,
            modes,
        })
    }

    /// What asking for `need` takes: the person's grants for use and operate; for view and manage the
    /// space's machines only (no grant changes those: view, the machine is in the space; manage, by role).
    pub async fn grants_for(&self, access: &super::Access, need: Level) -> sqlx::Result<Grants> {
        match need {
            Level::View | Level::Manage => Ok(Grants {
                modes: self.machine_modes(access.space).await?,
                ..Grants::default()
            }),
            Level::Use | Level::Operate => self.grants_of(access).await,
        }
    }

    /// The machines of a space, by id, as the access rules read them.
    pub async fn machine_modes(&self, space: i64) -> sqlx::Result<HashMap<String, MachineMode>> {
        let mut tx = self.pg().space(space).await?;
        let rows = sqlx::query!(
            "SELECT id, name, kind, induction_required FROM machines WHERE space_id = $1",
            space
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.id,
                    MachineMode {
                        name: r.name,
                        kind: r.kind,
                        induction_required: r.induction_required,
                    },
                )
            })
            .collect())
    }

    /// A machine's name in the space (the audit log's names).
    pub(crate) async fn machine_name(
        &self,
        space: i64,
        machine: &str,
    ) -> sqlx::Result<Option<String>> {
        let mut tx = self.pg().space(space).await?;
        let name = sqlx::query_scalar!("SELECT name FROM machines WHERE id = $1", machine)
            .fetch_optional(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(name)
    }

    /// Who has permission on a machine of the space, by name.
    pub async fn grants(&self, space: i64, machine: &str) -> sqlx::Result<Vec<Grant>> {
        let mut tx = self.pg().space(space).await?;
        let rows = sqlx::query!(
            r#"SELECT g.user_id, u.display_name, g.level, b.display_name AS "by?",
                   unix_seconds(g.granted_at) AS "granted_at!", unix_seconds(g.expires_at) AS expires_at
               FROM machine_grants g JOIN users u ON u.id = g.user_id
               LEFT JOIN users b ON b.id = g.granted_by
               WHERE g.space_id = $1 AND g.machine_id = $2"#,
            space,
            machine
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let mut grants: Vec<Grant> = rows
            .into_iter()
            .filter_map(|r| {
                Some(Grant {
                    user_id: r.user_id,
                    display_name: r.display_name,
                    level: Level::parse(&r.level)?,
                    by: r.by,
                    granted_at: r.granted_at,
                    expires_at: r.expires_at,
                })
            })
            .collect();
        // `ORDER BY u.display_name COLLATE NOCASE`; ties by user (the primary key's order).
        grants.sort_by_key(|g| (nocase(&g.display_name), g.user_id));
        Ok(grants)
    }

    /// Gives a member of the space permission on a machine of it (or changes it).
    pub async fn set_grant(
        &self,
        space: i64,
        machine: &str,
        user: i64,
        level: Level,
        by: i64,
        expires_at: Option<i64>,
    ) -> sqlx::Result<()> {
        let mut tx = self.pg().space(space).await?;
        Self::set_grant_in(&mut tx, space, machine, user, level, by, expires_at).await?;
        tx.commit().await
    }

    async fn set_grant_in(
        tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
        space: i64,
        machine: &str,
        user: i64,
        level: Level,
        by: i64,
        expires_at: Option<i64>,
    ) -> sqlx::Result<()> {
        sqlx::query!(
            "INSERT INTO machine_grants (space_id, machine_id, user_id, level, granted_by, granted_at, expires_at)
             VALUES ($1, $2, $3, $4, $5, at_unix($6), at_unix($7))
             ON CONFLICT (machine_id, user_id) DO UPDATE SET level = excluded.level,
                 granted_by = excluded.granted_by, granted_at = excluded.granted_at,
                 expires_at = excluded.expires_at",
            space,
            machine,
            user,
            level.as_str(),
            by,
            now(),
            expires_at
        )
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    pub async fn remove_grant(&self, space: i64, machine: &str, user: i64) -> sqlx::Result<bool> {
        let mut tx = self.pg().space(space).await?;
        let n = Self::remove_grant_in(&mut tx, space, machine, user).await?;
        tx.commit().await?;
        Ok(n)
    }

    async fn remove_grant_in(
        tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
        space: i64,
        machine: &str,
        user: i64,
    ) -> sqlx::Result<bool> {
        Ok(sqlx::query!(
            "DELETE FROM machine_grants WHERE space_id = $1 AND machine_id = $2 AND user_id = $3",
            space,
            machine,
            user
        )
        .execute(&mut **tx)
        .await?
        .rows_affected()
            > 0)
    }

    /// Someone's permissions at once (the Members tab's window): per group and per machine, `Some((level,
    /// until))` given, `None` taken away; in one transaction.
    pub async fn set_permissions(
        &self,
        space: i64,
        user: i64,
        by: i64,
        groups: &[(i64, Given)],
        machines: &[(String, Given)],
    ) -> sqlx::Result<()> {
        let mut tx = self.pg().space(space).await?;
        for (group, given) in groups {
            match given {
                Some((level, until)) => {
                    Self::set_group_grant_in(&mut tx, space, *group, user, *level, by, *until)
                        .await?
                }
                None => {
                    Self::remove_group_grant_in(&mut tx, space, *group, user).await?;
                }
            }
        }
        for (machine, given) in machines {
            match given {
                Some((level, until)) => {
                    Self::set_grant_in(&mut tx, space, machine, user, *level, by, *until).await?
                }
                None => {
                    Self::remove_grant_in(&mut tx, space, machine, user).await?;
                }
            }
        }
        tx.commit().await
    }

    /// Someone's permissions in these spaces go (they left them); in the caller's transaction.
    pub(crate) async fn remove_grants_of_in(
        tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
        user: i64,
        spaces: &[i64],
    ) -> sqlx::Result<()> {
        sqlx::query!(
            "DELETE FROM machine_grants WHERE space_id = ANY($1) AND user_id = $2",
            spaces,
            user
        )
        .execute(&mut **tx)
        .await?;
        sqlx::query!(
            "DELETE FROM group_grants WHERE space_id = ANY($1) AND user_id = $2",
            spaces,
            user
        )
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    // ---- machine groups ----------------------------------------------------------------------------

    /// The machine groups of a space, with their machines and who has permission on them.
    pub async fn groups(&self, space: i64) -> Result<Vec<Group>, BoxError> {
        let mut tx = self.pg().space(space).await?;
        let heads = sqlx::query!(
            "SELECT id, name FROM machine_groups WHERE space_id = $1",
            space
        )
        .fetch_all(&mut *tx)
        .await?;
        let members = sqlx::query!(
            "SELECT group_id, machine_id FROM machine_group_members WHERE space_id = $1",
            space
        )
        .fetch_all(&mut *tx)
        .await?;
        let grants = sqlx::query!(
            r#"SELECT g.group_id, g.user_id, u.display_name, g.level, b.display_name AS "by?",
                   unix_seconds(g.granted_at) AS "granted_at!", unix_seconds(g.expires_at) AS expires_at
               FROM group_grants g JOIN users u ON u.id = g.user_id
               LEFT JOIN users b ON b.id = g.granted_by
               WHERE g.space_id = $1"#,
            space
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        // The machines' names, of the space.
        let names: HashMap<String, String> = self
            .machines(space)
            .await?
            .into_iter()
            .map(|m| (m.id, m.name))
            .collect();
        let mut groups: Vec<Group> = heads
            .into_iter()
            .map(|h| {
                let mut machines: Vec<(String, String)> = members
                    .iter()
                    .filter(|m| m.group_id == h.id)
                    .filter_map(|m| {
                        names
                            .get(&m.machine_id)
                            .map(|n| (m.machine_id.clone(), n.clone()))
                    })
                    .collect();
                // `ORDER BY m.name COLLATE NOCASE`; ties by id (the members' primary key order).
                machines.sort_by(|a, b| (nocase(&a.1), &a.0).cmp(&(nocase(&b.1), &b.0)));
                let mut grants: Vec<Grant> = grants
                    .iter()
                    .filter(|g| g.group_id == h.id)
                    .map(|g| Grant {
                        user_id: g.user_id,
                        display_name: g.display_name.clone(),
                        level: Level::parse(&g.level).unwrap_or(Level::View),
                        by: g.by.clone(),
                        granted_at: g.granted_at,
                        expires_at: g.expires_at,
                    })
                    .collect();
                grants.sort_by_key(|g| (nocase(&g.display_name), g.user_id));
                Group {
                    id: h.id,
                    name: h.name,
                    machines,
                    grants,
                }
            })
            .collect();
        // `ORDER BY name COLLATE NOCASE`; ties by id (the order they were made).
        groups.sort_by_key(|g| (nocase(&g.name), g.id));
        Ok(groups)
    }

    /// A new group of machines; `kind` fills it with the space's machines of that kind (fdm, laser,
    /// label), or with all of them (`all`).
    pub async fn create_group(
        &self,
        space: i64,
        name: &str,
        kind: Option<&str>,
    ) -> Result<i64, BoxError> {
        let machines = match kind {
            None => Vec::new(),
            // The kind is kept as JSON: "fdm", with its quotes.
            Some(kind) => self
                .machine_modes(space)
                .await?
                .into_iter()
                .filter(|(_, m)| kind == "all" || m.kind == format!("\"{kind}\""))
                .map(|(id, _)| id)
                .collect(),
        };
        let mut tx = self.pg().space(space).await?;
        let id = sqlx::query_scalar!(
            "INSERT INTO machine_groups (space_id, name, created_at) VALUES ($1, $2, at_unix($3)) RETURNING id",
            space,
            name,
            now()
        )
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query!(
            "INSERT INTO machine_group_members (space_id, group_id, machine_id)
             SELECT $1, $2, unnest($3::text[]) ON CONFLICT DO NOTHING",
            space,
            id,
            &machines
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    /// A group's name, when it is of the space.
    pub async fn group_name(&self, space: i64, group: i64) -> sqlx::Result<Option<String>> {
        let mut tx = self.pg().space(space).await?;
        let name = sqlx::query_scalar!(
            "SELECT name FROM machine_groups WHERE id = $1 AND space_id = $2",
            group,
            space
        )
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(name)
    }

    pub async fn remove_group(&self, space: i64, group: i64) -> sqlx::Result<bool> {
        let mut tx = self.pg().space(space).await?;
        sqlx::query!(
            "DELETE FROM group_grants WHERE group_id = $1 AND space_id = $2",
            group,
            space
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            "DELETE FROM machine_group_members WHERE group_id = $1 AND space_id = $2",
            group,
            space
        )
        .execute(&mut *tx)
        .await?;
        let n = sqlx::query!(
            "DELETE FROM machine_groups WHERE id = $1 AND space_id = $2",
            group,
            space
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n > 0)
    }

    /// Puts a machine of the space in a group of it, or takes it out.
    pub async fn set_group_machine(
        &self,
        space: i64,
        group: i64,
        machine: &str,
        member: bool,
    ) -> Result<bool, BoxError> {
        let machine_here = self.machine_modes(space).await?.contains_key(machine);
        if self.group_name(space, group).await?.is_none() || !machine_here {
            return Ok(false);
        }
        let mut tx = self.pg().space(space).await?;
        if member {
            sqlx::query!(
                "INSERT INTO machine_group_members (space_id, group_id, machine_id) VALUES ($1, $2, $3)
                 ON CONFLICT DO NOTHING",
                space,
                group,
                machine
            )
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query!(
                "DELETE FROM machine_group_members WHERE space_id = $1 AND group_id = $2 AND machine_id = $3",
                space,
                group,
                machine
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Gives a member permission on every machine of a group of the space (or changes it).
    pub async fn set_group_grant(
        &self,
        space: i64,
        group: i64,
        user: i64,
        level: Level,
        by: i64,
        expires_at: Option<i64>,
    ) -> sqlx::Result<()> {
        let mut tx = self.pg().space(space).await?;
        Self::set_group_grant_in(&mut tx, space, group, user, level, by, expires_at).await?;
        tx.commit().await
    }

    async fn set_group_grant_in(
        tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
        space: i64,
        group: i64,
        user: i64,
        level: Level,
        by: i64,
        expires_at: Option<i64>,
    ) -> sqlx::Result<()> {
        sqlx::query!(
            "INSERT INTO group_grants (space_id, group_id, user_id, level, granted_by, granted_at, expires_at)
             VALUES ($1, $2, $3, $4, $5, at_unix($6), at_unix($7))
             ON CONFLICT (group_id, user_id) DO UPDATE SET level = excluded.level,
                 granted_by = excluded.granted_by, granted_at = excluded.granted_at,
                 expires_at = excluded.expires_at",
            space,
            group,
            user,
            level.as_str(),
            by,
            now(),
            expires_at
        )
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    pub async fn remove_group_grant(
        &self,
        space: i64,
        group: i64,
        user: i64,
    ) -> sqlx::Result<bool> {
        let mut tx = self.pg().space(space).await?;
        let n = Self::remove_group_grant_in(&mut tx, space, group, user).await?;
        tx.commit().await?;
        Ok(n)
    }

    async fn remove_group_grant_in(
        tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
        space: i64,
        group: i64,
        user: i64,
    ) -> sqlx::Result<bool> {
        Ok(sqlx::query!(
            "DELETE FROM group_grants WHERE space_id = $1 AND group_id = $2 AND user_id = $3",
            space,
            group,
            user
        )
        .execute(&mut **tx)
        .await?
        .rows_affected()
            > 0)
    }

    // ---- requests ----------------------------------------------------------------------------------

    /// Someone asks the staff for permission on a machine: false when they asked already and nobody
    /// answered yet.
    pub async fn ask_permission(&self, space: i64, user: i64, machine: &str) -> sqlx::Result<bool> {
        if self.asked(space, user, machine).await?.is_some() {
            return Ok(false);
        }
        let mut tx = self.pg().space(space).await?;
        sqlx::query!(
            "INSERT INTO access_requests (space_id, user_id, machine_id, asked_at) VALUES ($1, $2, $3, at_unix($4))",
            space,
            user,
            machine,
            now()
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// When someone asked for a machine of the space, while nobody answered.
    pub async fn asked(&self, space: i64, user: i64, machine: &str) -> sqlx::Result<Option<i64>> {
        let mut tx = self.pg().space(space).await?;
        let at = sqlx::query_scalar!(
            r#"SELECT unix_seconds(asked_at) AS "at!" FROM access_requests
               WHERE user_id = $1 AND machine_id = $2 AND state = 'open'
               ORDER BY id LIMIT 1"#,
            user,
            machine
        )
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(at)
    }

    /// A space's open requests, oldest first.
    pub async fn requests(&self, space: i64) -> Result<Vec<AskedFor>, BoxError> {
        let mut tx = self.pg().space(space).await?;
        let rows = sqlx::query!(
            r#"SELECT r.id, r.user_id, u.display_name, r.machine_id, unix_seconds(r.asked_at) AS "asked_at!"
               FROM access_requests r JOIN users u ON u.id = r.user_id
               WHERE r.space_id = $1 AND r.state = 'open' ORDER BY r.asked_at, r.id"#,
            space
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        // The machine of the space, its name and whether it needs a training.
        let machines = self.machine_modes(space).await?;
        let mut out = Vec::new();
        for r in rows {
            let Some(machine) = machines.get(&r.machine_id) else {
                continue;
            };
            out.push(AskedFor {
                id: r.id,
                user_id: r.user_id,
                who: r.display_name,
                training: machine.induction_required == 2,
                machine: machine.name.clone(),
                machine_id: r.machine_id,
                asked_at: r.asked_at,
            });
        }
        Ok(out)
    }

    /// Answers a request: given (`use` on the machine) or declined. The request, if it was open and of
    /// the space.
    pub async fn answer_request(
        &self,
        space: i64,
        id: i64,
        give: bool,
        by: i64,
    ) -> Result<Option<AskedFor>, BoxError> {
        let Some(request) = self.requests(space).await?.into_iter().find(|r| r.id == id) else {
            return Ok(None);
        };
        let mut tx = self.pg().space(space).await?;
        if give {
            Self::set_grant_in(
                &mut tx,
                space,
                &request.machine_id,
                request.user_id,
                Level::Use,
                by,
                None,
            )
            .await?;
        }
        sqlx::query!(
            "UPDATE access_requests SET state = $2, handled_by = $3, handled_at = at_unix($4) WHERE id = $1",
            id,
            if give { "given" } else { "declined" },
            by,
            now()
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(request))
    }
}

#[cfg(test)]
mod tests {
    use crate::store::pg::{APP_ROLE, Pg};
    use sqlx::PgPool;

    const TABLES: [&str; 5] = [
        "machine_grants",
        "machine_groups",
        "machine_group_members",
        "access_requests",
        "group_grants",
    ];

    /// Row-level security on the five access tables, as the app role: no space, no rows; a space, its own
    /// rows only; writing into another space refused.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn the_access_tables_show_only_their_space(pool: PgPool) {
        // As the owner: a second space, and a row in every table in both.
        sqlx::raw_sql(
            "INSERT INTO organisations (uid, name, created_at) VALUES ('o2', 'Other', at_unix(0));
             INSERT INTO spaces (name, uid, organisation_id)
                 SELECT 'Other', 's2', id FROM organisations WHERE uid = 'o2';
             INSERT INTO machines (id, space_id, name, kind, agent_version, token, registered_at)
                 SELECT 'm' || id, id, 'M', '\"fdm\"', '0.1.0', 't' || id, at_unix(0) FROM spaces;
             INSERT INTO machine_grants (space_id, machine_id, user_id, level, granted_at)
                 SELECT id, 'm' || id, 1, 'use', at_unix(0) FROM spaces;
             INSERT INTO machine_groups (id, space_id, name, created_at)
                 SELECT id, id, 'g', at_unix(0) FROM spaces;
             INSERT INTO machine_group_members (space_id, group_id, machine_id)
                 SELECT id, id, 'm' || id FROM spaces;
             INSERT INTO access_requests (space_id, user_id, machine_id, asked_at)
                 SELECT id, 1, 'm' || id, at_unix(0) FROM spaces;
             INSERT INTO group_grants (space_id, group_id, user_id, level, granted_at)
                 SELECT id, id, 1, 'use', at_unix(0) FROM spaces;",
        )
        .execute(&pool)
        .await
        .unwrap();
        let pg = Pg::with_options(&Pg::hub_login(&pool.connect_options()), 1);
        let role: String = sqlx::query_scalar("SELECT current_user::text")
            .fetch_one(&pg.app)
            .await
            .unwrap();
        assert_eq!(role, APP_ROLE);
        for table in TABLES {
            let all =
                || sqlx::AssertSqlSafe(format!("SELECT space_id FROM {table} ORDER BY space_id"));
            let none: Vec<i64> = sqlx::query_scalar(all()).fetch_all(&pg.app).await.unwrap();
            assert!(none.is_empty(), "{table} without a space: {none:?}");
            let mut tx = pg.space(1).await.unwrap();
            let seen: Vec<i64> = sqlx::query_scalar(all()).fetch_all(&mut *tx).await.unwrap();
            assert_eq!(seen, vec![1], "{table} in space 1");
            tx.commit().await.unwrap();
            let admin: Vec<i64> = sqlx::query_scalar(all())
                .fetch_all(pg.admin_in_tests())
                .await
                .unwrap();
            assert_eq!(admin, vec![1, 2], "{table} as admin");
        }
        // Writing into another space, from space 1: refused for every table.
        let writes = [
            "INSERT INTO machine_grants (space_id, machine_id, user_id, level, granted_at) VALUES (2, 'x', 2, 'use', now())",
            "INSERT INTO machine_groups (space_id, name, created_at) VALUES (2, 'x', now())",
            "INSERT INTO machine_group_members (space_id, group_id, machine_id) VALUES (2, 2, 'x')",
            "INSERT INTO access_requests (space_id, user_id, machine_id, asked_at) VALUES (2, 2, 'x', now())",
            "INSERT INTO group_grants (space_id, group_id, user_id, level, granted_at) VALUES (2, 2, 2, 'use', now())",
        ];
        for write in writes {
            let mut tx = pg.space(1).await.unwrap();
            let refused = sqlx::query(sqlx::AssertSqlSafe(write))
                .execute(&mut *tx)
                .await;
            assert!(refused.is_err(), "{write}");
        }
        // And a group's member or grant cannot claim another space than its group's.
        let mut tx = pg.space(1).await.unwrap();
        let mismatched = sqlx::query(
            "INSERT INTO machine_group_members (space_id, group_id, machine_id) VALUES (1, 2, 'y')",
        )
        .execute(&mut *tx)
        .await;
        assert!(mismatched.is_err());
    }
}
