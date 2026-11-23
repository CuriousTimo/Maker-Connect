//! The cameras of a space (docs/archive/plans/cameras.md, "Step 2, the module `cameras`"): one table on
//! Postgres (`migrations/`), every query in the space's transaction ([`Work`]). The password is kept
//! as the machines' tokens are (plain, owner 2026-11-16); it is never sent back in a page and never
//! logged.

use crate::Store;
use crate::access::{Access, Level, Role};
use crate::store::pg::Bypass;
use crate::store::{Legacy, Work, now};

/// A camera as stored. `machine_id` empty: a room camera, seen from `view_role` up.
#[derive(Debug, Clone)]
pub struct Camera {
    pub id: i64,
    pub space_id: i64,
    pub name: String,
    pub source: String,
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub machine_id: Option<String>,
    pub view_role: Role,
    pub created_at: i64,
    /// An agent camera's own fingerprint (`source` = `agent`); `None` for a network camera.
    pub fingerprint: Option<String>,
    /// When the newest still of an agent camera arrived.
    pub snapshot_at: Option<i64>,
}

/// What a form gives: the password `None` keeps the one stored.
pub struct CameraForm {
    pub name: String,
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub machine_id: Option<String>,
    pub view_role: Role,
}

const COLUMNS: &str = "id, space_id, name, source, url, username, password, machine_id, view_role,
     unix_seconds(created_at), fingerprint, unix_seconds(snapshot_at)";

type Row = (
    i64,
    i64,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    i64,
    Option<String>,
    Option<i64>,
);

fn camera(
    (
        id,
        space_id,
        name,
        source,
        url,
        username,
        password,
        machine_id,
        role,
        created_at,
        fingerprint,
        snapshot_at,
    ): Row,
) -> Camera {
    Camera {
        id,
        space_id,
        name,
        source,
        url,
        username,
        password,
        machine_id,
        view_role: Role::parse(&role).unwrap_or(Role::Admin),
        created_at,
        fingerprint,
        snapshot_at,
    }
}

impl Work {
    /// The space's cameras, by name.
    pub async fn cameras(&mut self) -> sqlx::Result<Vec<Camera>> {
        let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM cameras WHERE space_id = $1 ORDER BY name COLLATE \"C\", id"
        )))
        .bind(self.space)
        .fetch_all(self.c())
        .await?;
        Ok(rows.into_iter().map(camera).collect())
    }

    pub async fn camera(&mut self, id: i64) -> sqlx::Result<Option<Camera>> {
        let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM cameras WHERE space_id = $1 AND id = $2"
        )))
        .bind(self.space)
        .bind(id)
        .fetch_optional(self.c())
        .await?;
        Ok(row.map(camera))
    }

    pub async fn add_camera(&mut self, form: &CameraForm) -> sqlx::Result<i64> {
        sqlx::query_scalar(
            "INSERT INTO cameras (space_id, name, url, username, password, machine_id, view_role, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, at_unix($8)) RETURNING id",
        )
        .bind(self.space)
        .bind(&form.name)
        .bind(&form.url)
        .bind(&form.username)
        .bind(&form.password)
        .bind(&form.machine_id)
        .bind(form.view_role.as_str())
        .bind(now())
        .fetch_one(self.c())
        .await
    }

    /// Changes a camera; `false` when it is not in the space.
    pub async fn change_camera(&mut self, id: i64, form: &CameraForm) -> sqlx::Result<bool> {
        let changed = sqlx::query(
            "UPDATE cameras SET name = $3, url = $4, username = $5,
                 password = COALESCE($6, password), machine_id = $7, view_role = $8
             WHERE space_id = $1 AND id = $2",
        )
        .bind(self.space)
        .bind(id)
        .bind(&form.name)
        .bind(&form.url)
        .bind(&form.username)
        .bind(&form.password)
        .bind(&form.machine_id)
        .bind(form.view_role.as_str())
        .execute(self.c())
        .await?
        .rows_affected();
        Ok(changed > 0)
    }

    /// A USB camera of an agent registers: one camera per `(space, fingerprint)`, a new token each
    /// time. Returns its id.
    pub async fn register_agent_camera(
        &mut self,
        fingerprint: &str,
        name: &str,
        token: &str,
        registration: &str,
    ) -> sqlx::Result<i64> {
        sqlx::query_scalar(
            "INSERT INTO cameras (space_id, name, source, url, view_role, created_at, fingerprint, token,
                 registration)
             VALUES ($1, $2, 'agent', '', 'admin', at_unix($3), $4, $5, $6)
             ON CONFLICT (space_id, fingerprint) WHERE fingerprint IS NOT NULL
             DO UPDATE SET token = EXCLUDED.token, registration = EXCLUDED.registration
             RETURNING id",
        )
        .bind(self.space)
        .bind(name)
        .bind(now())
        .bind(fingerprint)
        .bind(token)
        .bind(registration)
        .fetch_one(self.c())
        .await
    }

    /// What the admin sets on an agent camera: its name and what it watches. `false` when it is not
    /// an agent camera of the space.
    pub async fn change_agent_camera(
        &mut self,
        id: i64,
        name: &str,
        machine_id: Option<&str>,
        view_role: Role,
    ) -> sqlx::Result<bool> {
        let changed = sqlx::query(
            "UPDATE cameras SET name = $3, machine_id = $4, view_role = $5
             WHERE space_id = $1 AND id = $2 AND source = 'agent'",
        )
        .bind(self.space)
        .bind(id)
        .bind(name)
        .bind(machine_id)
        .bind(view_role.as_str())
        .execute(self.c())
        .await?
        .rows_affected();
        Ok(changed > 0)
    }

    /// Keeps a still an agent camera sent as its newest; the bytes as they came.
    pub async fn keep_snapshot(&mut self, id: i64, jpeg: &[u8]) -> sqlx::Result<()> {
        sqlx::query(
            "INSERT INTO camera_snapshots (camera_id, space_id, jpeg) VALUES ($2, $1, $3)
             ON CONFLICT (camera_id) DO UPDATE SET jpeg = EXCLUDED.jpeg",
        )
        .bind(self.space)
        .bind(id)
        .bind(jpeg)
        .execute(self.c())
        .await?;
        sqlx::query("UPDATE cameras SET snapshot_at = at_unix($3) WHERE space_id = $1 AND id = $2")
            .bind(self.space)
            .bind(id)
            .bind(now())
            .execute(self.c())
            .await?;
        Ok(())
    }

    /// The newest still of a camera, as it was sent.
    pub async fn snapshot(&mut self, id: i64) -> sqlx::Result<Option<Vec<u8>>> {
        sqlx::query_scalar(
            "SELECT jpeg FROM camera_snapshots WHERE space_id = $1 AND camera_id = $2",
        )
        .bind(self.space)
        .bind(id)
        .fetch_optional(self.c())
        .await
    }

    /// Removes a camera; `false` when it is not in the space.
    pub async fn remove_camera(&mut self, id: i64) -> sqlx::Result<bool> {
        let removed = sqlx::query("DELETE FROM cameras WHERE space_id = $1 AND id = $2")
            .bind(self.space)
            .bind(id)
            .execute(self.c())
            .await?
            .rows_affected();
        Ok(removed > 0)
    }
}

impl Store {
    /// A space's cameras, by name ([`Work::cameras`]).
    pub async fn cameras(&self, space: i64) -> sqlx::Result<Vec<Camera>> {
        crate::stock::in_space!(self, space, |w| w.cameras()).await
    }

    /// The camera a token belongs to, with the space and the fingerprint it registered with: what
    /// `PUT /c/snapshot` is checked against.
    pub(crate) async fn camera_by_token(
        &self,
        token: &str,
    ) -> sqlx::Result<Option<(i64, i64, Option<String>)>> {
        sqlx::query_as("SELECT id, space_id, fingerprint FROM cameras WHERE token = $1")
            .bind(token)
            .fetch_optional(self.pg().admin(Bypass::Secret))
            .await
    }

    /// Every camera of every space (their sessions start with the hub): a background task lists its
    /// work, which camera in which space (`Bypass::TaskList`), and reads each camera in its space.
    pub async fn all_cameras(&self) -> sqlx::Result<Vec<Camera>> {
        let ids: Vec<(i64, i64)> = sqlx::query_as("SELECT space_id, id FROM cameras ORDER BY id")
            .fetch_all(self.pg().admin(Bypass::TaskList))
            .await?;
        let mut all = Vec::with_capacity(ids.len());
        for (space, id) in ids {
            if let Some(camera) = crate::stock::in_space!(self, space, |w| w.camera(id)).await? {
                all.push(camera);
            }
        }
        Ok(all)
    }
}

impl Legacy {
    /// Whether `access` may watch this camera (owner, 2026-11-16): a machine camera whoever
    /// may see the machine; a room camera from its `view_role` up; a camera whose machine is
    /// gone, admins only. `grants`: the space's machines ([`crate::Store::grants_for`], View).
    pub fn may_view_camera(
        &self,
        access: &Access,
        camera: &Camera,
        grants: &crate::access::Grants,
    ) -> bool {
        if camera.space_id != access.space {
            return false;
        }
        match &camera.machine_id {
            Some(machine) => match self.may(access, machine, Level::View, grants) {
                Some(may) => may,
                None => access.role >= Role::Admin,
            },
            None => access.role >= camera.view_role,
        }
    }

    /// The cameras on a machine that `access` may watch, of the space's `cameras`: for its page's
    /// Camera tab.
    pub fn machine_cameras(
        &self,
        access: &Access,
        machine: &str,
        grants: &crate::access::Grants,
        cameras: Vec<Camera>,
    ) -> Vec<Camera> {
        cameras
            .into_iter()
            .filter(|c| c.machine_id.as_deref() == Some(machine))
            .filter(|c| self.may_view_camera(access, c, grants))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access(space: i64, role: Role) -> Access {
        Access {
            user_id: 1,
            actor: "test".to_owned(),
            space,
            role,
            token_id: None,
            session_id: None,
            instance_admin: false,
        }
    }

    fn form(name: &str, machine: Option<&str>, view_role: Role) -> CameraForm {
        CameraForm {
            name: name.to_owned(),
            url: "rtsp://192.0.2.10:554/stream".to_owned(),
            username: Some("admin".to_owned()),
            password: Some("secret".to_owned()),
            machine_id: machine.map(str::to_owned),
            view_role,
        }
    }

    /// Stored, changed (an empty password keeps the old one), removed; another space sees nothing.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_camera_is_of_its_space(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let space = store.default_space().await.unwrap().0;
        let other = {
            let owner = store.create_user("olga", "Olga", "x", false).await.unwrap();
            store
                .create_organisation("Fablab", "Werkplaats", owner)
                .await
                .unwrap()
                .1
        };
        let mut w = store.work(space).await.unwrap();
        let id = w
            .add_camera(&form("Hall", None, Role::Member))
            .await
            .unwrap();
        w.commit(&store).await.unwrap();
        assert_eq!(store.cameras(space).await.unwrap().len(), 1);
        let mut o = store.work(other).await.unwrap();
        assert!(o.cameras().await.unwrap().is_empty());
        assert!(o.camera(id).await.unwrap().is_none());
        assert!(
            !o.change_camera(id, &form("x", None, Role::Guest))
                .await
                .unwrap()
        );
        assert!(!o.remove_camera(id).await.unwrap());
        o.commit(&store).await.unwrap();
        let mut w = store.work(space).await.unwrap();
        let mut f = form("Hall 2", None, Role::Guest);
        f.password = None;
        assert!(w.change_camera(id, &f).await.unwrap());
        let c = w.camera(id).await.unwrap().unwrap();
        assert_eq!(c.name, "Hall 2");
        assert_eq!(c.password.as_deref(), Some("secret"));
        assert_eq!(c.view_role, Role::Guest);
        assert_eq!(c.source, "rtsp");
        assert!(w.remove_camera(id).await.unwrap());
        assert!(w.cameras().await.unwrap().is_empty());
        w.commit(&store).await.unwrap();
        assert!(store.all_cameras().await.unwrap().is_empty());
    }

    /// The camera task's list (allowlist #5): every space's cameras, each read in its own space.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn the_camera_task_reads_each_camera_in_its_space(pool: sqlx::PgPool) {
        let store = crate::Store::for_test(&pool).await;
        let (first, _) = store.default_space().await.unwrap();
        let second = store.create_space(first, "Lab").await.unwrap();
        for (space, name) in [(first, "Hall"), (second, "Lab")] {
            let mut w = store.work(space).await.unwrap();
            w.add_camera(&form(name, None, Role::Admin)).await.unwrap();
            w.commit(&store).await.unwrap();
        }
        let all: Vec<_> = store
            .all_cameras()
            .await
            .unwrap()
            .into_iter()
            .map(|c| (c.space_id, c.name, c.password))
            .collect();
        let secret = Some("secret".to_owned());
        assert_eq!(
            all,
            [
                (first, "Hall".to_owned(), secret.clone()),
                (second, "Lab".to_owned(), secret)
            ]
        );
    }

    /// Who may watch (owner, 2026-11-16), per role and kind of camera.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn who_may_watch(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let space = store.default_space().await.unwrap().0;
        let mk3s = crate::profiles::find("prusa/mk3s").unwrap();
        let open = store.add_virtual(space, mk3s).await.unwrap();
        let inducted = store.add_virtual(space, mk3s).await.unwrap();
        let removed = store.add_virtual(space, mk3s).await.unwrap();
        store
            .set_induction_required(space, &inducted, true)
            .await
            .unwrap();
        let grants = store
            .grants_for(&access(space, Role::Owner), Level::View)
            .await
            .unwrap();
        let elsewhere = store
            .grants_for(&access(space + 1, Role::Owner), Level::View)
            .await
            .unwrap();
        let mut w = store.work(space).await.unwrap();
        let mut ids = Vec::new();
        for (name, machine, role) in [
            ("a", Some(open.as_str()), Role::Admin),
            ("b", Some(inducted.as_str()), Role::Admin),
            ("c", Some(removed.as_str()), Role::Admin),
            ("d", None, Role::Admin),
            ("e", None, Role::Member),
        ] {
            ids.push(w.add_camera(&form(name, machine, role)).await.unwrap());
        }
        let [on_open, on_inducted, gone, room_admin, room_member] = ids[..] else {
            unreachable!()
        };
        let mut cameras = std::collections::HashMap::new();
        w.commit(&store).await.unwrap();
        // Its machine removed: the camera stays, watching nothing (its key clears).
        assert!(store.remove_machine(space, &removed).await.unwrap());
        let mut w = store.work(space).await.unwrap();
        for id in &ids {
            cameras.insert(*id, w.camera(*id).await.unwrap().unwrap());
        }
        w.commit(&store).await.unwrap();
        assert_eq!(cameras[&gone].machine_id, None);
        let all = store.cameras(space).await.unwrap();
        let s = store.lock().unwrap();
        let may =
            |role: Role, id: i64| s.may_view_camera(&access(space, role), &cameras[&id], &grants);
        for role in Role::ALL {
            // Whoever may see the machine (a guest has View), with or without induction.
            assert!(may(role, on_open), "{role:?}");
            assert!(may(role, on_inducted), "{role:?}");
            // A camera whose machine is gone: a room camera, from its role up (admin here).
            assert_eq!(may(role, gone), role >= Role::Admin, "{role:?}");
            // A room camera: from its role up.
            assert_eq!(may(role, room_admin), role >= Role::Admin, "{role:?}");
            assert_eq!(may(role, room_member), role >= Role::Member, "{role:?}");
        }
        // Another space sees none of them.
        assert!(!s.may_view_camera(
            &access(space + 1, Role::Owner),
            &cameras[&on_open],
            &elsewhere
        ));
        assert_eq!(
            s.machine_cameras(&access(space, Role::Guest), &open, &grants, all)
                .len(),
            1
        );
    }
}
