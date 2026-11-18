//! The cameras of a space (docs/plans/cameras.md, "Step 2, the module `cameras`"): one table, every
//! query by space. The password is kept as the machines' tokens are (plain, owner 2026-11-16);
//! it is never sent back in a page and never logged.

use crate::access::{Access, Level, Role};
use crate::store::{Legacy, now};
use rusqlite::{Connection, OptionalExtension, params};

pub fn migrate(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS cameras (
             id INTEGER PRIMARY KEY,
             space_id INTEGER NOT NULL,
             name TEXT NOT NULL,
             source TEXT NOT NULL DEFAULT 'rtsp',
             url TEXT NOT NULL,
             username TEXT,
             password TEXT,
             machine_id TEXT,
             view_role TEXT NOT NULL DEFAULT 'admin',
             created_at INTEGER NOT NULL
         );",
    )
}

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

const COLUMNS: &str =
    "id, space_id, name, source, url, username, password, machine_id, view_role, created_at";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Camera> {
    let role: String = r.get(8)?;
    Ok(Camera {
        id: r.get(0)?,
        space_id: r.get(1)?,
        name: r.get(2)?,
        source: r.get(3)?,
        url: r.get(4)?,
        username: r.get(5)?,
        password: r.get(6)?,
        machine_id: r.get(7)?,
        view_role: Role::parse(&role).unwrap_or(Role::Admin),
        created_at: r.get(9)?,
    })
}

impl Legacy {
    /// A space's cameras, by name.
    pub fn cameras(&self, space: i64) -> rusqlite::Result<Vec<Camera>> {
        let mut q = self.db.prepare(&format!(
            "SELECT {COLUMNS} FROM cameras WHERE space_id = ?1 ORDER BY name, id"
        ))?;
        let rows = q.query_map([space], row)?;
        rows.collect()
    }

    /// Every camera of every space (their sessions start with the hub).
    pub fn all_cameras(&self) -> rusqlite::Result<Vec<Camera>> {
        let mut q = self
            .db
            .prepare(&format!("SELECT {COLUMNS} FROM cameras ORDER BY id"))?;
        let rows = q.query_map([], row)?;
        rows.collect()
    }

    pub fn camera(&self, space: i64, id: i64) -> rusqlite::Result<Option<Camera>> {
        self.db
            .query_row(
                &format!("SELECT {COLUMNS} FROM cameras WHERE space_id = ?1 AND id = ?2"),
                params![space, id],
                row,
            )
            .optional()
    }

    pub fn add_camera(&self, space: i64, form: &CameraForm) -> rusqlite::Result<i64> {
        self.db.execute(
            "INSERT INTO cameras (space_id, name, url, username, password, machine_id, view_role, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                space,
                form.name,
                form.url,
                form.username,
                form.password,
                form.machine_id,
                form.view_role.as_str(),
                now()
            ],
        )?;
        Ok(self.db.last_insert_rowid())
    }

    /// Changes a camera; `false` when it is not in the space.
    pub fn change_camera(&self, space: i64, id: i64, form: &CameraForm) -> rusqlite::Result<bool> {
        let changed = self.db.execute(
            "UPDATE cameras SET name = ?3, url = ?4, username = ?5,
                 password = COALESCE(?6, password), machine_id = ?7, view_role = ?8
             WHERE space_id = ?1 AND id = ?2",
            params![
                space,
                id,
                form.name,
                form.url,
                form.username,
                form.password,
                form.machine_id,
                form.view_role.as_str()
            ],
        )?;
        Ok(changed > 0)
    }

    /// Removes a camera; `false` when it is not in the space.
    pub fn remove_camera(&self, space: i64, id: i64) -> rusqlite::Result<bool> {
        let removed = self.db.execute(
            "DELETE FROM cameras WHERE space_id = ?1 AND id = ?2",
            params![space, id],
        )?;
        Ok(removed > 0)
    }

    /// Whether `access` may watch this camera (owner, 2026-11-16): a machine camera whoever
    /// may see the machine; a room camera from its `view_role` up; a camera whose machine is
    /// gone, admins only.
    pub fn may_view_camera(&self, access: &Access, camera: &Camera) -> rusqlite::Result<bool> {
        if camera.space_id != access.space {
            return Ok(false);
        }
        match &camera.machine_id {
            Some(machine) => Ok(match self.may(access, machine, Level::View)? {
                Some(may) => may,
                None => access.role >= Role::Admin,
            }),
            None => Ok(access.role >= camera.view_role),
        }
    }

    /// The cameras on a machine that `access` may watch: for its page's Camera tab.
    pub fn machine_cameras(&self, access: &Access, machine: &str) -> Vec<Camera> {
        self.cameras(access.space)
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.machine_id.as_deref() == Some(machine))
            .filter(|c| self.may_view_camera(access, c).unwrap_or(false))
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

    /// A new store has the table; one opened before it was there gets it.
    #[test]
    fn the_table_is_made_new_and_on_an_older_store() {
        let s = Legacy::open(":memory:").unwrap();
        let n: i64 =
            s.db.query_row("SELECT COUNT(*) FROM cameras", [], |r| r.get(0))
                .unwrap();
        assert_eq!(n, 0);
        let file = std::env::temp_dir().join(format!("cameras-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&file);
        {
            let s = Legacy::open(file.to_str().unwrap()).unwrap();
            s.db.execute_batch("DROP TABLE cameras").unwrap();
        }
        let s = Legacy::open(file.to_str().unwrap()).unwrap();
        assert!(s.cameras(1).unwrap().is_empty());
        let _ = std::fs::remove_file(&file);
    }

    /// Stored, changed (an empty password keeps the old one), removed; another space sees nothing.
    #[test]
    fn a_camera_is_of_its_space() {
        let s = Legacy::open(":memory:").unwrap();
        let space = s.default_space().unwrap().0;
        let id = s
            .add_camera(space, &form("Hall", None, Role::Member))
            .unwrap();
        assert_eq!(s.cameras(space).unwrap().len(), 1);
        assert!(s.cameras(space + 1).unwrap().is_empty());
        assert!(s.camera(space + 1, id).unwrap().is_none());
        assert!(
            !s.change_camera(space + 1, id, &form("x", None, Role::Guest))
                .unwrap()
        );
        let mut f = form("Hall 2", None, Role::Guest);
        f.password = None;
        assert!(s.change_camera(space, id, &f).unwrap());
        let c = s.camera(space, id).unwrap().unwrap();
        assert_eq!(c.name, "Hall 2");
        assert_eq!(c.password.as_deref(), Some("secret"));
        assert_eq!(c.view_role, Role::Guest);
        assert_eq!(c.source, "rtsp");
        assert!(!s.remove_camera(space + 1, id).unwrap());
        assert!(s.remove_camera(space, id).unwrap());
        assert!(s.cameras(space).unwrap().is_empty());
    }

    /// Who may watch (owner, 2026-11-16), per role and kind of camera.
    #[test]
    fn who_may_watch() {
        let mut s = Legacy::open(":memory:").unwrap();
        let space = s.default_space().unwrap().0;
        let open = s
            .add_virtual(space, crate::profiles::find("prusa/mk3s").unwrap())
            .unwrap();
        let inducted = s
            .add_virtual(space, crate::profiles::find("prusa/mk3s").unwrap())
            .unwrap();
        s.db.execute(
            "UPDATE machines SET induction_required = 1 WHERE id = ?1",
            [&inducted],
        )
        .unwrap();
        let on_open = s
            .add_camera(space, &form("a", Some(&open), Role::Admin))
            .unwrap();
        let on_inducted = s
            .add_camera(space, &form("b", Some(&inducted), Role::Admin))
            .unwrap();
        let gone = s
            .add_camera(space, &form("c", Some("no-such-machine"), Role::Guest))
            .unwrap();
        let room_admin = s.add_camera(space, &form("d", None, Role::Admin)).unwrap();
        let room_member = s.add_camera(space, &form("e", None, Role::Member)).unwrap();
        let may = |role: Role, id: i64| {
            let c = s.camera(space, id).unwrap().unwrap();
            s.may_view_camera(&access(space, role), &c).unwrap()
        };
        for role in Role::ALL {
            // Whoever may see the machine (a guest has View), with or without induction.
            assert!(may(role, on_open), "{role:?}");
            assert!(may(role, on_inducted), "{role:?}");
            // A camera whose machine is gone: admins only.
            assert_eq!(may(role, gone), role >= Role::Admin, "{role:?}");
            // A room camera: from its role up.
            assert_eq!(may(role, room_admin), role >= Role::Admin, "{role:?}");
            assert_eq!(may(role, room_member), role >= Role::Member, "{role:?}");
        }
        // Another space sees none of them.
        let c = s.camera(space, on_open).unwrap().unwrap();
        assert!(
            !s.may_view_camera(&access(space + 1, Role::Owner), &c)
                .unwrap()
        );
        assert_eq!(
            s.machine_cameras(&access(space, Role::Guest), &open).len(),
            1
        );
    }
}
