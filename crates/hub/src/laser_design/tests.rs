//! HS13: the served fork, who may open it, the list of lasers, and the send path end to end against the hub's own routes.

use super::*;
use crate::access::{Access, Level};
use crate::store::JobState;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

/// A packed file as the script would make: what is checked here is how the hub serves bytes, not what is in them.
static FAKE: &[Asset] = &[
    Asset {
        path: "index.html",
        content_type: "text/html; charset=utf-8",
        gzip: &[0x1f, 0x8b, 1],
    },
    Asset {
        path: "pkg/laserfoundry_web.js",
        content_type: "text/javascript; charset=utf-8",
        gzip: &[0x1f, 0x8b, 2],
    },
    Asset {
        path: "pkg/laserfoundry_web_bg.wasm",
        content_type: "application/wasm",
        gzip: &[0x1f, 0x8b, 3],
    },
];

/// A laser program: `M4` makes the hub read it as a laser's file; the moves are small and absolute.
const GCODE: &str = "; made by the page\nG21\nG90\nM4 S0\nG0 X10 Y10\nG1 X20 Y10 S300 F1000\nM5\n";

mod prepare_tests {
    use super::*;
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn the_tool_is_open_only_to_a_member_who_may_use_a_laser_of_the_space(
        pool: sqlx::PgPool,
    ) {
        let w = World::new(&pool).await;
        w.person("ann", Role::Member).await;
        w.person("gus", Role::Guest).await;
        let store = w.store.clone();
        // The access first, from Postgres; then the machines, from the store.
        let of = async |name: &str, built: bool| {
            let access = w.access_of(name).await;
            availability_of(&store.lock().unwrap(), &access, built).unwrap()
        };
        assert_eq!(of("ann", false).await, Availability::NotBuilt);
        assert_eq!(of("ann", true).await, Availability::NoLaser);
        w.laser("Ortur", true);
        assert_eq!(
            of("ann", true).await,
            Availability::NotPermitted,
            "a laser that needs permission ann has not got"
        );
        assert_eq!(of("gus", true).await, Availability::Guest);
        w.laser("Open one", false);
        assert_eq!(of("ann", true).await, Availability::Open);
    }

    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn prepare_says_why_the_tool_is_not_there(pool: sqlx::PgPool) {
        let w = World::new(&pool).await;
        let (_, ann) = w.person("ann", Role::Member).await;
        let (_, gus) = w.person("gus", Role::Guest).await;
        let (status, body) = w.get(&ann, "/prepare").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("<h3>Laser design</h3>"), "{body}");
        let expect = if ASSETS.is_empty() {
            "Not in this hub's build"
        } else {
            "There is no laser in this space"
        };
        assert!(body.contains(expect), "{body}");
        let (status, body) = w.get(&gus, "/prepare").await;
        assert_eq!(status, StatusCode::OK, "a guest sees what there is: {body}");
        assert!(!body.contains("Open laser design"), "{body}");
    }
}

mod serving_tests {
    use super::*;
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn the_files_are_gzip_with_their_type_and_the_policy_that_keeps_serial_out(
        pool: sqlx::PgPool,
    ) {
        let w = World::new(&pool).await;
        w.person("ann", Role::Member).await;
        w.laser("Ortur", false);
        let store = w.store.clone();
        for asset in FAKE {
            let access = w.access_of("ann").await;
            let response = serve(&access, &store, FAKE, asset.path);
            assert_eq!(response.status(), StatusCode::OK, "{}", asset.path);
            let h = response.headers();
            assert_eq!(h[header::CONTENT_ENCODING], "gzip");
            assert_eq!(h[header::CONTENT_TYPE], asset.content_type);
            assert_eq!(h["permissions-policy"], "serial=(), usb=(), hid=()");
            assert_eq!(h[header::CACHE_CONTROL], "no-cache");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(
                body.as_ref(),
                asset.gzip,
                "the packed bytes as they are: the browser unpacks them"
            );
        }
        assert_eq!(
            serve(
                &w.access_of("ann").await,
                &store,
                FAKE,
                "pkg/../../etc/passwd"
            )
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            serve(&w.access_of("ann").await, &store, FAKE, "nothing").status(),
            StatusCode::NOT_FOUND
        );
    }

    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn only_a_member_who_may_use_a_laser_opens_it(pool: sqlx::PgPool) {
        let w = World::new(&pool).await;
        let (_, ann) = w.person("ann", Role::Member).await;
        let (_, gus) = w.person("gus", Role::Guest).await;
        // no laser yet
        let (status, body) = w.get(&ann, "/laser-design/").await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(body.contains("no laser"), "{body}");
        let laser = w.laser("Ortur", false);
        // open to members: ann may use it; a guest may not
        let ok = if ASSETS.is_empty() {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::OK
        };
        assert_eq!(w.get(&ann, "/laser-design/").await.0, ok);
        assert_eq!(w.get(&gus, "/laser-design/").await.0, StatusCode::FORBIDDEN);
        assert_eq!(
            w.get(&gus, "/api/laser-design/lasers").await.0,
            StatusCode::FORBIDDEN
        );
        // without a session: the hub's sign-in, never the page
        let anonymous = w
            .app
            .clone()
            .oneshot(Request::get("/laser-design/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(
            anonymous.status().is_redirection() || anonymous.status() == StatusCode::UNAUTHORIZED,
            "{}",
            anonymous.status()
        );
        // a laser that needs permission: not usable by ann until she is given it
        w.store
            .lock()
            .unwrap()
            .set_access_mode(w.space, &laser, 1)
            .unwrap();
        assert_eq!(w.get(&ann, "/laser-design/").await.0, StatusCode::FORBIDDEN);
        let (user, _) = (w.user("ann").await, ());
        w.store
            .lock()
            .unwrap()
            .set_grant(w.space, &laser, user, Level::Use, user, None)
            .unwrap();
        assert_eq!(w.get(&ann, "/laser-design/").await.0, ok);
    }

    /// Without the feature the hub says so (and still builds); with it the real bundle is served and holds no Web Serial.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn the_built_in_bundle_is_served_gzip_and_holds_no_web_serial(pool: sqlx::PgPool) {
        let w = World::new(&pool).await;
        let (_, ann) = w.person("ann", Role::Member).await;
        w.laser("Ortur", false);
        if ASSETS.is_empty() {
            let (status, body) = w.get(&ann, "/laser-design/").await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert!(body.contains("laser-design"), "{body}");
            return;
        }
        let response = w
            .app
            .clone()
            .oneshot(
                Request::get("/laser-design/pkg/laserfoundry_web.js")
                    .header("cookie", &ann)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(response.headers()["permissions-policy"], PERMISSIONS_POLICY);
        for asset in ASSETS {
            assert_eq!(&asset.gzip[..2], &[0x1f, 0x8b], "{} is gzip", asset.path);
            let mut text = Vec::new();
            std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(asset.gzip), &mut text)
                .unwrap();
            for needle in [&b"requestPort"[..], b"navigator.serial", b"getPorts"] {
                assert!(
                    !text.windows(needle.len()).any(|w| w == needle),
                    "{} holds {}",
                    asset.path,
                    String::from_utf8_lossy(needle)
                );
            }
        }
    }
}

mod lasers_tests {
    use super::*;
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn the_list_has_the_lasers_the_person_may_use_in_the_apps_shape(pool: sqlx::PgPool) {
        let w = World::new(&pool).await;
        let (user, ann) = w.person("ann", Role::Member).await;
        let open = w.laser("Open one", false);
        let closed = w.laser("Closed one", true);
        w.store
            .lock()
            .unwrap()
            .add_virtual(w.space, crate::profiles::find("prusa/mk3s").unwrap())
            .unwrap();
        let (status, body) = w.get(&ann, "/api/laser-design/lasers").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let list: serde_json::Value = serde_json::from_str(&body).unwrap();
        let ids: Vec<&str> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec![open.as_str()],
            "no printer, no laser that needs permission"
        );
        assert_eq!(list[0]["name"], "Open one");
        assert_eq!(list[0]["kind"], "laser");
        assert_eq!(list[0]["may"], "operate");
        assert!(list[0]["work_area_mm"].is_array() && list[0]["laser"].is_object());
        w.store
            .lock()
            .unwrap()
            .set_grant(w.space, &closed, user, Level::Use, user, None)
            .unwrap();
        let (_, body) = w.get(&ann, "/api/laser-design/lasers").await;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}

mod send_tests {
    use super::*;

    /// What the page does, against the hub's real routes with a person's session: upload the file, load it on the laser;
    /// a loaded job asks first and is replaced only when asked again with `replace`. Nothing starts.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn the_page_loads_a_job_asks_before_replacing_and_starts_nothing(pool: sqlx::PgPool) {
        let w = World::new(&pool).await;
        let (_, ann) = w.person("ann", Role::Member).await;
        let laser = w.laser("Ortur", false);
        let upload = |name: &str| {
            Request::post(format!("/api/files?name={name}"))
                .header("cookie", &ann)
                .header("x-requested-with", "laserfoundry")
                .header("content-type", "text/plain")
                .body(Body::from(GCODE))
                .unwrap()
        };
        let load = |file: &str, extra: &str| {
            Request::post(format!("/files/{file}/print"))
                .header("cookie", &ann)
                .header("x-requested-with", "laserfoundry")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("machine={laser}{extra}")))
                .unwrap()
        };
        let (status, body) = w.send(upload("design.gcode")).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let first = serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let (status, body) = w.send(load(&first, "")).await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::OK, r#"{"loaded":"design.gcode"}"#)
        );
        let state = |w: &World| {
            let mut s = w.store.lock().unwrap();
            let job = s
                .active_job(&laser)
                .unwrap()
                .map(|j| (j.file_name, j.state));
            (job, s.start_queued().unwrap(), s.dispatch().unwrap())
        };
        assert_eq!(
            state(&w),
            (Some(("design.gcode".to_owned(), JobState::Loaded)), 0, 0),
            "loaded, not started"
        );
        // a second one: the hub asks, and changes nothing
        let (_, body) = w.send(upload("second.gcode")).await;
        let second = serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let (status, body) = w.send(load(&second, "")).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        let ask: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(ask["loaded"], "design.gcode");
        assert!(
            ask["confirm"]
                .as_str()
                .unwrap()
                .contains("replace it with second.gcode"),
            "{body}"
        );
        assert_eq!(
            state(&w).0,
            Some(("design.gcode".to_owned(), JobState::Loaded)),
            "nothing was replaced by the question"
        );
        // replace, as the person agreed
        let (status, body) = w.send(load(&second, "&replace=true")).await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::OK, r#"{"loaded":"second.gcode"}"#)
        );
        assert_eq!(
            state(&w),
            (Some(("second.gcode".to_owned(), JobState::Loaded)), 0, 0)
        );
    }

    /// The page may use only these calls; none of the hub's other routes (frame, start burn, queue) is on its list.
    #[test]
    fn what_the_page_sends_is_what_the_hub_expects() {
        let page = [
            ("GET", "/api/apps/v1/info"),
            ("GET", "/api/laser-design/lasers"),
            ("POST", "/api/files"),
            ("POST", "/files/{id}/print"),
        ];
        for (method, route) in page {
            assert!(
                crate::access::need(
                    &axum::http::Method::from_bytes(method.as_bytes()).unwrap(),
                    route
                )
                .is_some(),
                "{method} {route}"
            );
        }
        for never in [
            "/files/{id}/frame",
            "/machines/{id}/start-burn",
            "/machines/{id}/command",
        ] {
            assert!(!page.iter().any(|(_, r)| *r == never));
        }
    }
}

struct World {
    store: Store,
    app: Router,
    space: i64,
    org: i64,
}

impl World {
    async fn new(pool: &sqlx::PgPool) -> Self {
        let store = crate::Store::for_test(pool).await;
        let owner = store.create_user("olga", "Olga", "x", false).await.unwrap();
        let (org, space) = store
            .create_organisation("Fablab", "Werkplaats", owner)
            .await
            .unwrap();
        let app = crate::app(store.clone());
        World {
            store,
            app,
            space,
            org,
        }
    }

    /// A user in the organisation with a role; their id and cookie.
    async fn person(&self, name: &str, role: Role) -> (i64, String) {
        let s = &self.store;
        let user = s.create_user(name, name, "x", false).await.unwrap();
        s.set_org_membership(user, self.org, role.as_str())
            .await
            .unwrap();
        let token = s
            .create_session(user, Some(self.space), "", "")
            .await
            .unwrap();
        (user, format!("{}={token}", crate::auth::COOKIE))
    }

    async fn user(&self, name: &str) -> i64 {
        self.store.user_id(name).await.unwrap().unwrap()
    }

    async fn access_of(&self, name: &str) -> Access {
        let user = self.user(name).await;
        let role = self
            .store
            .org_role_of(user, self.org)
            .await
            .unwrap()
            .unwrap();
        Access {
            user_id: user,
            actor: name.to_owned(),
            space: self.space,
            role,
            token_id: None,
            session_id: None,
            instance_admin: false,
        }
    }

    /// A laser (a virtual one), open to members or, when `needs_permission`, closed to them until given.
    fn laser(&self, name: &str, needs_permission: bool) -> String {
        let mut s = self.store.lock().unwrap();
        let id = s
            .add_virtual(
                self.space,
                crate::profiles::find("ortur/lm2-pro-s2").unwrap(),
            )
            .unwrap();
        s.rename(self.space, &id, name).unwrap();
        // a new machine is closed until permission is given: open it to members, or keep it closed
        s.set_access_mode(self.space, &id, u8::from(needs_permission))
            .unwrap();
        // online, as a laser whose agent reports
        let token = s
            .virtual_machines()
            .unwrap()
            .into_iter()
            .find(|(m, ..)| *m == id)
            .unwrap()
            .1;
        s.report(&token, &Default::default()).unwrap();
        id
    }

    async fn send(&self, request: Request<Body>) -> (StatusCode, String) {
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    async fn get(&self, cookie: &str, path: &str) -> (StatusCode, String) {
        self.send(
            Request::get(path)
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }
}
