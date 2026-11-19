use crate::{Store, failed, store, updates};
use axum::{
    Form, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use maker_connect_protocol::{self as protocol, Register, Registered, Telemetry, TelemetryReply};

pub(crate) async fn register(State(store): State<Store>, Json(agent): Json<Register>) -> Response {
    if agent.protocol != protocol::VERSION {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "this hub speaks protocol {}, the agent {}",
                protocol::VERSION,
                agent.protocol
            ),
        )
            .into_response();
    }
    match store.register(&agent).await {
        Ok((machine_id, token, pairing_code)) => {
            println!(
                "agent registered: {} ({}), code {pairing_code}",
                agent.name,
                agent.kind.label()
            );
            Json(Registered {
                machine_id,
                token,
                pairing_code,
            })
            .into_response()
        }
        Err(error) => failed(error),
    }
}

pub(crate) async fn telemetry(
    State(store): State<Store>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    Json(report): Json<Telemetry>,
) -> Response {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    // The machine as it was before this report (when it was last seen), from Postgres.
    let caller = match store.caller(token).await {
        Ok(caller) => caller,
        Err(error) => return failed(error),
    };
    {
        let mut store = store.lock().unwrap();
        // K18: an agent not seen for longer than ONLINE_SECONDS: its Sent/Accepted commands failed,
        // before this report (and its Done, which then still wins) is taken in.
        store.fail_commands_of_gone_agent(caller.as_ref());
        for done in &report.done {
            let state = if done.ok {
                store::CommandState::Finished
            } else {
                store::CommandState::Rejected
            };
            store.command_went(
                caller.as_ref(),
                done.id,
                state,
                done.reason.clone(),
                done.output.clone(),
            );
        }
    }
    let (mut reply, space) = {
        // Agents before 2026-11-04 reported a label printer without a cassette as an error, or
        // as idle with the reason: it waits for someone (attention), it is not a fault.
        let mut report = report;
        if report.cassette == Some(false)
            && matches!(
                report.state,
                protocol::MachineState::Error | protocol::MachineState::Idle
            )
            && report.problem.as_deref() == Some("no cassette")
        {
            report.state = protocol::MachineState::Attention;
        }
        if let Some(peer) = peer_address(&headers, &extensions)
            && let Err(error) = store.set_peer(token, &peer).await
        {
            return failed(error);
        }
        let reported = store.report(token, &report).await;
        if matches!(reported, Ok(Some(_)))
            // A print it runs itself: recorded as a job when it ends.
            && let Err(error) = store.follow_own_print(token, &report, None).await
        {
            return failed(error);
        }
        let of = match agent_of(&store, token).await {
            Ok(of) => of,
            Err(error) => return failed(error),
        };
        let update = match &reported {
            Ok(Some(_)) => match updates::order_for(&store, token, &report).await {
                Ok(update) => update,
                Err(error) => return failed(error),
            },
            _ => None,
        };
        // The job's part, on Postgres, before the lock is taken.
        let agent_jobs = match &reported {
            Ok(Some(_)) => match agent_job(&store, &of, &report).await {
                Ok(job) => Some(job),
                Err(error) => return failed(error),
            },
            _ => None,
        };
        // The machine as this report left it (its profile may have been recognised).
        let caller = match &reported {
            Ok(Some(_)) => match store.caller(token).await {
                Ok(caller) => caller,
                Err(error) => return failed(error),
            },
            _ => None,
        };
        let mut store = store.lock().unwrap();
        match reported {
            Ok(Some(space)) => {
                // B3: the hub does not hold or refuse a command for a machine behind an agent, whatever it
                // is doing (busy, waiting for someone): it queues them (one at a time, but for the Marlin port's machines, K18), and the machine's
                // port refuses as its reference does (OctoPrint: `server/api/printer.py`).
                // K18, HS4: a machine on a port that fails its pending commands on close (Marlin/OctoPrint,
                // DYMO PnP, LabelWriter 450, laser) gets every Queued command at once (no gate, no
                // 120 s); the other agent ports (Buddy, Klipper, the MK3 family) keep one at a time.
                let next = if caller.as_ref().is_some_and(|c| c.fails_pending_on_close) {
                    store.next_commands_for_agent(caller.as_ref())
                } else {
                    store.next_command(caller.as_ref()).into_iter().collect()
                };
                // Only typed commands go to an agent (the hub makes no G-code for it).
                let commands = next
                    .into_iter()
                    .filter_map(|(id, body)| {
                        serde_json::from_str::<protocol::Action>(&body)
                            .ok()
                            .map(|action| protocol::MachineCommand { id, action })
                    })
                    .collect();
                let (job, job_control) = agent_jobs.unwrap_or_default();
                // Someone's answer to the question on its screen, once; and pause, resume or stop
                // for a print it started itself (no job of the hub's).
                let (answer, job_control) = match &of.machine {
                    Some(machine) => (
                        store.answers.remove(machine),
                        job_control.or_else(|| store.own_job_control.remove(machine)),
                    ),
                    None => (None, job_control),
                };
                (
                    TelemetryReply {
                        paired: space.is_some(),
                        space: None,
                        commands,
                        job,
                        job_control,
                        update,
                        answer,
                    },
                    space,
                )
            }
            Ok(None) => {
                return (StatusCode::UNAUTHORIZED, "unknown agent: register again").into_response();
            }
            Err(error) => return failed(error),
        }
    };
    // The space's name, from Postgres, once the lock is let go.
    if let Some(space) = space {
        match store.space_name(space).await {
            Ok(name) => reply.space = Some(name),
            Err(error) => return failed(error),
        }
    }
    Json(reply).into_response()
}

/// Where a request came from: the address it reached the hub from, or, behind a reverse
/// proxy, the first address the proxy says it forwarded for.
pub(crate) fn peer_address(
    headers: &HeaderMap,
    extensions: &axum::http::Extensions,
) -> Option<String> {
    let forwarded = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty());
    forwarded.or_else(|| {
        extensions
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|info| info.0.ip().to_string())
    })
}

/// A job for an agent's machine, as its report stands: how one ended, a queued one to
/// fetch, and pause, resume or stop for the one it runs.
/// Why a job is paused that a power cut stopped and its agent kept.
const POWER_CUT: &str = "a power cut stopped it: resume it here or on the printer";

/// What [`agent_job`] needs of the machine from Postgres, read before the lock is taken.
#[derive(Debug, Default)]
pub(crate) struct AgentOf {
    /// The machine, by the token.
    pub machine: Option<String>,
    /// Its space, when it is in one: its jobs' (and every query of them runs in it).
    pub space: Option<i64>,
    /// [`Store::loaded_media_mm`].
    pub media: Option<(u32, Option<u32>)>,
    /// [`Store::loaded_page_size`].
    pub page_size: Option<String>,
}

pub(crate) async fn agent_of(store: &Store, token: &str) -> sqlx::Result<AgentOf> {
    let Some(machine) = store.machine_id_by_token(token).await? else {
        return Ok(AgentOf::default());
    };
    let Some((_, space)) = store.placed(token).await? else {
        return Ok(AgentOf {
            machine: Some(machine),
            ..Default::default()
        });
    };
    Ok(AgentOf {
        media: store.loaded_media_mm(space, &machine).await?,
        page_size: store.loaded_page_size(space, &machine).await?,
        machine: Some(machine),
        space: Some(space),
    })
}

pub(crate) async fn agent_job(
    store: &Store,
    of: &AgentOf,
    report: &Telemetry,
) -> Result<(Option<protocol::JobOrder>, Option<protocol::JobControl>), store::BoxError> {
    use protocol::{JobControl, MachineState};
    use store::JobState;
    // A machine in no space has no jobs.
    let Some(space) = of.space else {
        return Ok((None, None));
    };
    if let Some(end) = &report.job_end {
        let state = match (end.ok, end.stopped) {
            (true, _) => JobState::Finished,
            (false, true) => JobState::Stopped,
            (false, false) => JobState::Failed,
        };
        store
            .set_job(space, end.id, state, end.reason.as_deref())
            .await?;
        if let Some(line) = end.restart_line {
            store.set_restart_line(space, end.id, line).await?;
        }
    }
    let Some(machine) = of.machine.clone() else {
        return Ok((None, None));
    };
    // A job a power cut stopped (Prusa's MK3; the agent kept it): it failed here when the
    // agent no longer ran it, and comes back as the agent reports it again, paused until
    // someone resumes it.
    if let Some(id) = report.job_id
        && matches!(report.state, MachineState::Paused | MachineState::Busy)
        && store.active_job(space, &machine).await?.is_none()
        && let Some(lost) = store.job(space, id).await?
        && lost.machine_id == machine
        && lost.state == JobState::Failed
    {
        let state = if report.state == MachineState::Paused {
            JobState::Paused
        } else {
            JobState::Printing
        };
        store.revive_job(space, id, state, Some(POWER_CUT)).await?;
    }
    let Some(job) = store.active_job(space, &machine).await? else {
        return Ok((None, None));
    };
    // Resumed from here after the power cut: its reason goes (a later pause is an ordinary
    // one); the agent is told to resume below as for any job.
    if job.state == JobState::Printing && job.reason.as_deref() == Some(POWER_CUT) {
        store
            .revive_job(space, job.id, JobState::Printing, None)
            .await?;
    }
    // Taken up on the printer's own screen after the power cut: printing again, not to be
    // paused from here.
    if job.state == JobState::Paused
        && job.reason.as_deref() == Some(POWER_CUT)
        && report.job_id == Some(job.id)
        && report.state == MachineState::Busy
    {
        // Its reason cleared too: a later pause from here is an ordinary one.
        store
            .revive_job(space, job.id, JobState::Printing, None)
            .await?;
        return Ok((None, None));
    }
    let runs = report.job_id == Some(job.id);
    Ok(match job.state {
        JobState::Queued if report.job_id.is_none() && report.state == MachineState::Idle => {
            let size = std::fs::metadata(store.file_path(&job.file_id)).map_or(0, |m| m.len());
            store
                .set_job(space, job.id, JobState::Printing, None)
                .await?;
            let media = of.media;
            let order = protocol::JobOrder {
                id: job.id,
                name: job.file_name.clone(),
                download: format!("/api/agent/v1/jobs/{}/file", job.id),
                size,
                from_line: job.from_line,
                tape_mm: media.map(|(width, _)| width),
                label_length_mm: media.and_then(|(_, length)| length),
                page_size: of.page_size.clone(),
            };
            (Some(order), None)
        }
        JobState::Paused if runs && report.state != MachineState::Paused => {
            (None, Some(JobControl::Pause))
        }
        JobState::Printing if runs && report.state == MachineState::Paused => {
            (None, Some(JobControl::Resume))
        }
        JobState::Stopping if runs => (None, Some(JobControl::Stop)),
        // Given out, but the machine does not run it (the agent restarted, or lost it).
        JobState::Printing | JobState::Paused
            if !runs && job.started_at.is_some_and(|t| store::now() - t > 30) =>
        {
            let reason = "the machine does not run it (the agent restarted?)";
            store
                .set_job(space, job.id, JobState::Failed, Some(reason))
                .await?;
            (None, None)
        }
        // Asked to stop before the agent took it up, or it no longer runs it.
        JobState::Stopping => {
            store
                .set_job(space, job.id, JobState::Stopped, None)
                .await?;
            (None, None)
        }
        _ => (None, None),
    })
}

#[derive(serde::Deserialize)]
pub(crate) struct PairForm {
    code: String,
    /// The name it gets here; empty or absent: the name it registered with.
    #[serde(default)]
    name: String,
}

pub(crate) async fn pair(
    access: crate::access::Access,
    State(store): State<Store>,
    headers: HeaderMap,
    Form(form): Form<PairForm>,
) -> Response {
    let mut paired = store.pair(access.space, &form.code).await;
    let wanted = form.name.trim();
    if let Ok(Some((id, name))) = &mut paired
        && !wanted.is_empty()
        && wanted != name
    {
        match store.rename(access.space, id, wanted).await {
            Ok(_) => *name = wanted.to_owned(),
            Err(error) => return failed(error),
        }
    }
    let fetched = headers.contains_key("x-requested-with");
    match paired {
        Ok(Some((id, name))) if fetched => {
            axum::Json(serde_json::json!({ "id": id, "name": name })).into_response()
        }
        Ok(None) if fetched => {
            (StatusCode::NOT_FOUND, "no machine waits with that code").into_response()
        }
        Ok(Some(_)) => Redirect::to("/").into_response(),
        Ok(None) => Redirect::to("/?unknown-code").into_response(),
        Err(error) => failed(error),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::store::{self};
    use crate::{label_page, machine_page};
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use maker_connect_protocol::{MachineKind, MachineState};

    use tower::ServiceExt;
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn an_agent_runs_a_job(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        // The agent's own requests carry its token, no session: as from a real agent.
        let agent = crate::app(store.clone());
        let laser = Register {
            protocol: protocol::VERSION,
            name: "Ortur".into(),
            kind: MachineKind::Laser,
            agent_version: "0.1.0".into(),
        };
        let (_, body) = call(
            &agent,
            json(
                protocol::REGISTER,
                None,
                serde_json::to_string(&laser).unwrap(),
            ),
        )
        .await;
        let registered: Registered = serde_json::from_str(&body).unwrap();
        let space = store.default_space().await.unwrap().0;
        store.pair(space, &registered.pairing_code).await.unwrap();
        store
            .set_profile(space, &registered.machine_id, Some("ortur/lm2-pro-s2"))
            .await
            .unwrap();
        crate::auth::tester_may_operate(&store, &registered.machine_id).await;
        let report = |telemetry: Telemetry| {
            let app = agent.clone();
            let token = registered.token.clone();
            async move {
                let (_, body) = call(
                    &app,
                    json(
                        protocol::TELEMETRY,
                        Some(&token),
                        serde_json::to_string(&telemetry).unwrap(),
                    ),
                )
                .await;
                serde_json::from_str::<TelemetryReply>(&body).unwrap()
            }
        };
        let idle = || Telemetry {
            state: MachineState::Idle,
            ..Telemetry::default()
        };
        assert_eq!(report(idle()).await.job, None);

        let upload = Request::post("/api/files?name=sign.gcode")
            .body(Body::from("G90\nM4 S0\nG1 X10 Y10 S800 F3000\nM5\n"))
            .unwrap();
        let (_, body) = call(&app, upload).await;
        let file: serde_json::Value = serde_json::from_str(&body).unwrap();
        // A laser: the tester has and wears the glasses (crate::safety).
        let user = store.user_id("tester").await.unwrap().unwrap();
        store
            .lock()
            .unwrap()
            .confirm_ppe(user, &registered.machine_id);
        let print = Request::post(format!("/files/{}/print", file["id"].as_str().unwrap()))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("x-requested-with", "fetch")
            .body(Body::from(format!("machine={}", registered.machine_id)))
            .unwrap();
        let (status, body) = call(&app, print).await;
        // HS7b: a laser's job is loaded, not started: nothing goes to the laser yet
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::OK, r#"{"loaded":"sign.gcode"}"#)
        );
        // another file sent to it replaces the loaded one only after the person confirms
        let again = |replace: bool| {
            Request::post(format!("/files/{}/print", file["id"].as_str().unwrap()))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .body(Body::from(format!(
                    "machine={}{}",
                    registered.machine_id,
                    if replace { "&replace=true" } else { "" }
                )))
                .unwrap()
        };
        let (status, body) = call(&app, again(false)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body.contains("replace it with"), "{body}");
        let (status, body) = call(&app, again(true)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let stopped: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE state = 'stopped' AND reason LIKE 'replaced%'",
        )
        .fetch_one(store.pg().admin_in_tests())
        .await
        .unwrap();
        assert_eq!(stopped, 1, "the replaced job ends stopped, never started");
        assert_eq!(
            report(idle()).await.job,
            None,
            "a loaded job is not given out"
        );
        let start_burn = |confirm: bool| {
            Request::post(format!("/machines/{}/start-burn", registered.machine_id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .body(Body::from(if confirm { "confirm=true" } else { "" }))
                .unwrap()
        };
        let laser_idle = || Telemetry {
            state: MachineState::Idle,
            grbl: Some(protocol::GrblSettings {
                laser_mode: Some(true),
                ..Default::default()
            }),
            ..Telemetry::default()
        };
        // Start burn needs a frame first
        let (status, body) = call(&app, start_burn(false)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(
            body.contains("$32"),
            "laser mode unknown: refused first, as LaserFoundry: {body}"
        );
        report(laser_idle()).await;
        let (status, body) = call(&app, start_burn(false)).await;
        assert_eq!(
            (status, body.as_str()),
            (
                StatusCode::CONFLICT,
                "frame the loaded job first (after a Set origin it is framed again)"
            )
        );
        // the frame: queued as LaserFoundry's frame of the loaded file, done by the laser
        let frame = Request::post(format!("/files/{}/frame", file["id"].as_str().unwrap()))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("x-requested-with", "fetch")
            .body(Body::from(format!("machine={}", registered.machine_id)))
            .unwrap();
        let (status, body) = call(&app, frame).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        let reply = report(laser_idle()).await;
        let command = reply.commands.first().expect("the frame goes to the laser");
        assert!(matches!(command.action, protocol::Action::Frame { .. }));
        assert_eq!(reply.job, None);
        report(Telemetry {
            done: vec![finished(command.id)],
            ..laser_idle()
        })
        .await;
        // a Set origin after the frame: framed again first, then the person confirms
        let set_origin = Request::post(format!("/machines/{}/command", registered.machine_id))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("x-requested-with", "fetch")
            .body(Body::from("action=set-origin"))
            .unwrap();
        let (status, body) = call(&app, set_origin).await;
        assert!(status.is_success(), "{status} {body}");
        let reply = report(laser_idle()).await;
        let command = reply
            .commands
            .first()
            .expect("Set origin goes to the laser");
        assert!(matches!(command.action, protocol::Action::SetOrigin));
        report(Telemetry {
            done: vec![finished(command.id)],
            ..laser_idle()
        })
        .await;
        let (status, body) = call(&app, start_burn(true)).await;
        assert_eq!(
            (status, body.as_str()),
            (
                StatusCode::CONFLICT,
                "frame the loaded job first (after a Set origin it is framed again)"
            )
        );
        let frame = Request::post(format!("/files/{}/frame", file["id"].as_str().unwrap()))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("x-requested-with", "fetch")
            .body(Body::from(format!("machine={}", registered.machine_id)))
            .unwrap();
        assert_eq!(call(&app, frame).await.0, StatusCode::NO_CONTENT);
        let reply = report(laser_idle()).await;
        let command = reply.commands.first().expect("the frame again");
        report(Telemetry {
            done: vec![finished(command.id)],
            ..laser_idle()
        })
        .await;
        let (status, body) = call(&app, start_burn(false)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(
            body.contains(crate::files::ORIGIN_CONFIRM),
            "after a Set origin the person confirms: {body}"
        );
        // framed and confirmed: Start burn goes, and the job is given out once
        let (status, body) = call(&app, start_burn(true)).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        let (status, body) = call(&app, start_burn(true)).await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::CONFLICT, "busy with sign.gcode"),
            "once"
        );

        let order = report(idle()).await.job.expect("the job is given out");
        assert_eq!(order.name, "sign.gcode");
        let fetch = |token: &str| {
            Request::get(&order.download)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap()
        };
        let (status, gcode) = call(&agent, fetch(&registered.token)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(gcode.contains("M4 S0"));
        // Not with another token, nor without one.
        assert_eq!(
            call(&agent, fetch("not-its-token")).await.0,
            StatusCode::NOT_FOUND
        );
        let bare = Request::get(&order.download).body(Body::empty()).unwrap();
        assert_eq!(call(&agent, bare).await.0, StatusCode::NOT_FOUND);
        // The page's download still needs someone signed in.
        let page_download = format!("/files/{}/download", file["id"].as_str().unwrap());
        let bare = Request::get(&page_download).body(Body::empty()).unwrap();
        assert_eq!(call(&agent, bare).await.0, StatusCode::UNAUTHORIZED);

        let running = || Telemetry {
            state: MachineState::Busy,
            job_id: Some(order.id),
            ..Telemetry::default()
        };
        assert_eq!(report(running()).await.job, None, "given out once");
        store
            .set_job(space, order.id, store::JobState::Paused, None)
            .await
            .unwrap();
        assert_eq!(
            report(running()).await.job_control,
            Some(protocol::JobControl::Pause)
        );

        let ended = Telemetry {
            job_end: Some(protocol::JobEnd {
                id: order.id,
                ok: true,
                stopped: false,
                reason: None,
                restart_line: None,
            }),
            ..idle()
        };
        report(ended).await;
        let job = store.job(space, order.id).await.unwrap().unwrap();
        assert_eq!(job.state, store::JobState::Finished);
        assert!(
            store
                .machine(space, &registered.machine_id)
                .await
                .unwrap()
                .unwrap()
                .job
                .is_none()
        );
    }

    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_cassette_out_needs_attention_not_an_error(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let register = Register {
            protocol: protocol::VERSION,
            name: "Dymo".into(),
            kind: MachineKind::Label,
            agent_version: "0.1.0".into(),
        };
        let (_, body) = call(
            &app,
            json(
                protocol::REGISTER,
                None,
                serde_json::to_string(&register).unwrap(),
            ),
        )
        .await;
        let registered: Registered = serde_json::from_str(&body).unwrap();
        let space = store.default_space().await.unwrap().0;
        store.pair(space, &registered.pairing_code).await.unwrap();
        let old_agent = Telemetry {
            state: MachineState::Error,
            cassette: Some(false),
            problem: Some("no cassette".into()),
            ..Telemetry::default()
        };
        call(
            &app,
            json(
                protocol::TELEMETRY,
                Some(&registered.token),
                serde_json::to_string(&old_agent).unwrap(),
            ),
        )
        .await;
        let machine = store
            .machine(space, &registered.machine_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            machine.telemetry.as_ref().unwrap().state,
            MachineState::Attention
        );
        assert_eq!(
            machine_page::shown_state(&machine),
            ("no cassette".to_owned(), "attention".to_owned())
        );
        let page = label_page::render("My space", &machine, &[]);
        assert!(page.contains(r#"class="state attention""#));
    }

    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn an_agent_registers_is_paired_and_listed(pool: sqlx::PgPool) {
        let app = crate::auth::signed_in_app(crate::store::Store::for_test(&pool).await).await;
        let register = Register {
            protocol: protocol::VERSION,
            name: "MK3S Gonzo".into(),
            kind: MachineKind::Fdm,
            agent_version: "0.1.0".into(),
        };
        let (status, body) = call(
            &app,
            json(
                protocol::REGISTER,
                None,
                serde_json::to_string(&register).unwrap(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let registered: Registered = serde_json::from_str(&body).unwrap();
        assert_eq!(registered.pairing_code.len(), 6);

        let report = Telemetry {
            state: MachineState::Idle,
            firmware: Some("3.14.0".into()),
            nozzle_c: Some(24.0),
            bed_c: Some(23.0),
            job: None,
            identity: None,
            ..Telemetry::default()
        };
        let telemetry = || {
            json(
                protocol::TELEMETRY,
                Some(&registered.token),
                serde_json::to_string(&report).unwrap(),
            )
        };
        let (_, body) = call(&app, telemetry()).await;
        assert!(
            !serde_json::from_str::<TelemetryReply>(&body)
                .unwrap()
                .paired
        );
        let (_, body) = call(
            &app,
            Request::get("/api/machines").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(body, "[]", "not listed before it is paired");

        let form = Request::post("/pair")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(format!(
                "code={}",
                registered.pairing_code.to_lowercase()
            )))
            .unwrap();
        let (status, _) = call(&app, form).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        let (_, body) = call(&app, telemetry()).await;
        let reply: TelemetryReply = serde_json::from_str(&body).unwrap();
        assert!(reply.paired);
        assert_eq!(reply.space.as_deref(), Some("My space"));

        let (_, body) = call(
            &app,
            Request::get("/api/machines").body(Body::empty()).unwrap(),
        )
        .await;
        let machines: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(machines[0]["name"], "MK3S Gonzo");
        assert_eq!(machines[0]["online"], true);
        assert_eq!(machines[0]["telemetry"]["state"], "idle");
    }

    /// An unknown token is refused, and an agent of another protocol version is told so.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn unknown_agents_and_versions_are_refused(pool: sqlx::PgPool) {
        let app = crate::auth::signed_in_app(crate::store::Store::for_test(&pool).await).await;
        let report = r#"{"state":"idle"}"#.to_owned();
        let (status, _) = call(&app, json(protocol::TELEMETRY, Some("nope"), report)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let old = r#"{"protocol":0,"name":"x","kind":"fdm","agent_version":"0"}"#.to_owned();
        let (status, body) = call(&app, json(protocol::REGISTER, None, old)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("protocol 1"));
    }

    pub(crate) async fn call(app: &Router, request: Request<Body>) -> (StatusCode, String) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    pub(crate) fn json(path: &str, token: Option<&str>, body: String) -> Request<Body> {
        let mut request = Request::post(path).header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        request.body(Body::from(body)).unwrap()
    }

    // ---- K18: no gate for the Marlin port's machines; the agent-gone rule ----

    /// An agent machine, registered and paired into the default space: (id, token, space).
    async fn agent_machine(
        store: &crate::Store,
        kind: MachineKind,
        profile: Option<&str>,
    ) -> (String, String, i64) {
        let (space, _) = store.default_space().await.unwrap();
        let (id, token, code) = store
            .register(&Register {
                protocol: protocol::VERSION,
                name: "test".into(),
                kind,
                agent_version: "0.1.0".into(),
            })
            .await
            .unwrap();
        store.pair(space, &code).await.unwrap().unwrap();
        if profile.is_some() {
            store.set_profile(space, &id, profile).await.unwrap();
        }
        (id, token, space)
    }

    async fn queue_gcode(store: &crate::Store, space: i64, machine: &str, text: &str) -> u32 {
        let link = store.link_of(space, machine).await.unwrap();
        store
            .lock()
            .unwrap()
            .queue_action(
                link.as_deref(),
                machine,
                text,
                &protocol::Action::Gcode { text: text.into() },
            )
            .unwrap()
    }

    /// A report with these results, the reply's command ids.
    async fn report(app: &Router, token: &str, done: Vec<protocol::CommandDone>) -> Vec<u32> {
        let report = Telemetry {
            state: MachineState::Idle,
            done,
            ..Telemetry::default()
        };
        let (status, body) = call(
            app,
            json(
                protocol::TELEMETRY,
                Some(token),
                serde_json::to_string(&report).unwrap(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        serde_json::from_str::<TelemetryReply>(&body)
            .unwrap()
            .commands
            .iter()
            .map(|c| c.id)
            .collect()
    }

    async fn state_of(
        store: &crate::Store,
        space: i64,
        machine: &str,
        id: u32,
    ) -> (store::CommandState, Option<String>) {
        let machine = store.machine(space, machine).await.unwrap().unwrap();
        let c = machine.commands.iter().find(|c| c.id == id).unwrap();
        (c.state, c.reason.clone())
    }

    fn finished(id: u32) -> protocol::CommandDone {
        protocol::CommandDone {
            id,
            ok: true,
            reason: None,
            output: Vec::new(),
        }
    }

    /// Two commands for a Marlin agent machine go out together, and a typed `M112` goes out while an
    /// `M109` is still Sent (B7).
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn k18_a_marlin_agent_machine_has_no_gate(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let (id, token, space) = agent_machine(&store, MachineKind::Fdm, None).await;
        let a = queue_gcode(&store, space, &id, "G28").await;
        let b = queue_gcode(&store, space, &id, "M105").await;
        assert_eq!(
            report(&app, &token, vec![]).await,
            vec![a, b],
            "both at once"
        );
        assert_eq!(
            state_of(&store, space, &id, a).await.0,
            store::CommandState::Sent
        );
        assert_eq!(
            report(&app, &token, vec![]).await,
            Vec::<u32>::new(),
            "nothing queued, nothing sent twice"
        );

        let m109 = queue_gcode(&store, space, &id, "M109 S200").await;
        assert_eq!(report(&app, &token, vec![]).await, vec![m109]);
        let m112 = queue_gcode(&store, space, &id, "M112").await;
        assert_eq!(
            report(&app, &token, vec![]).await,
            vec![m112],
            "the M112 goes while the M109 is Sent"
        );
        assert_eq!(
            state_of(&store, space, &id, m109).await.0,
            store::CommandState::Sent
        );
        // no 120 s: the commands that were Sent long ago are not given up by the hub
        let old = store::now() - 3600;
        {
            store.lock().unwrap().set_commands_at(&id, old);
        }
        report(&app, &token, vec![]).await;
        assert_eq!(
            state_of(&store, space, &id, m109).await.0,
            store::CommandState::Sent
        );
    }

    /// The agent ports that do not fail pending commands on close (a Prusa profile, Alfawise's U30) keep one
    /// at a time and 120 s.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn k18_the_other_agent_machines_keep_their_gate(pool: sqlx::PgPool) {
        for (kind, profile) in [
            (MachineKind::Fdm, Some("prusa/mini")),
            (MachineKind::Fdm, Some("prusa/mk4s")),
            (MachineKind::Fdm, Some("alfawise/u30")),
        ] {
            // A new hub for each (the database emptied again).
            let store = crate::store::Store::fresh_for_test(&pool).await;
            let app = crate::auth::signed_in_app(store.clone()).await;
            let (id, token, space) = agent_machine(&store, kind, profile).await;
            let a = queue_gcode(&store, space, &id, "G28").await;
            let b = queue_gcode(&store, space, &id, "M105").await;
            assert_eq!(
                report(&app, &token, vec![]).await,
                vec![a],
                "{kind:?} {profile:?}"
            );
            assert_eq!(
                report(&app, &token, vec![]).await,
                Vec::<u32>::new(),
                "held while {a} is Sent"
            );
            assert_eq!(report(&app, &token, vec![finished(a)]).await, vec![b]);
            // and the 120 s
            let c = queue_gcode(&store, space, &id, "M104 S0").await;
            report(&app, &token, vec![finished(b)]).await;
            assert_eq!(
                state_of(&store, space, &id, c).await.0,
                store::CommandState::Sent
            );
            {
                store
                    .lock()
                    .unwrap()
                    .set_commands_at(&id, store::now() - 121);
            }
            report(&app, &token, vec![]).await;
            let (state, reason) = state_of(&store, space, &id, c).await;
            assert_eq!(state, store::CommandState::Rejected);
            assert_eq!(reason.as_deref(), Some("no answer from the machine"));
        }
    }

    /// HS4: the label printers (DYMO PnP, LabelWriter 450) and the laser have no gate either: every Queued
    /// command goes at once, none is given up after 120 s, and the agent-gone rule applies to them.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn hs4_label_and_laser_agent_machines_have_no_gate(pool: sqlx::PgPool) {
        for kind in [MachineKind::Label, MachineKind::Laser] {
            // A new hub for each (the database emptied again).
            let store = crate::store::Store::fresh_for_test(&pool).await;
            let app = crate::auth::signed_in_app(store.clone()).await;
            let (id, token, space) = agent_machine(&store, kind, None).await;
            let a = queue_gcode(&store, space, &id, "G28").await;
            let b = queue_gcode(&store, space, &id, "M105").await;
            assert_eq!(
                report(&app, &token, vec![]).await,
                vec![a, b],
                "{kind:?}: both at once"
            );
            // no 120 s
            store
                .lock()
                .unwrap()
                .set_commands_at(&id, store::now() - 3600);
            report(&app, &token, vec![]).await;
            assert_eq!(
                state_of(&store, space, &id, a).await.0,
                store::CommandState::Sent,
                "{kind:?}"
            );
            // the agent-gone rule: Sent fails, Queued does not
            let c = queue_gcode(&store, space, &id, "M104 S0").await;
            store
                .set_last_seen(&id, store::now() - store::ONLINE_SECONDS - 1)
                .await;
            let caller = store.caller(&token).await.unwrap();
            store
                .lock()
                .unwrap()
                .fail_commands_of_gone_agent(caller.as_ref());
            assert_eq!(
                state_of(&store, space, &id, a).await,
                (
                    store::CommandState::Rejected,
                    Some("the agent was gone".to_owned())
                ),
                "{kind:?}"
            );
            assert_eq!(
                state_of(&store, space, &id, c).await.0,
                store::CommandState::Queued,
                "{kind:?}"
            );
        }
    }

    /// K18's agent-gone rule: only the Sent and Accepted commands, not the Queued, only of that machine.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn k18_agent_gone_fails_sent_and_accepted_of_that_machine_only(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let (id, token, space) = agent_machine(&store, MachineKind::Fdm, None).await;
        let (other, other_token, _) = agent_machine(&store, MachineKind::Fdm, None).await;
        let (prusa, prusa_token, _) =
            agent_machine(&store, MachineKind::Fdm, Some("prusa/mk4s")).await;
        let sent = queue_gcode(&store, space, &id, "M109 S200").await;
        let accepted = queue_gcode(&store, space, &id, "G28").await;
        let other_sent = queue_gcode(&store, space, &other, "M109 S200").await;
        let prusa_sent = queue_gcode(&store, space, &prusa, "M109 S200").await;
        report(&app, &token, vec![]).await;
        report(&app, &other_token, vec![]).await;
        report(&app, &prusa_token, vec![]).await;
        let caller = store.caller(&token).await.unwrap();
        store
            .lock()
            .unwrap()
            .command_still_running(caller.as_ref(), accepted);
        let queued = queue_gcode(&store, space, &id, "M105").await;
        // seen just now: nothing happens
        let caller = store.caller(&token).await.unwrap();
        store
            .lock()
            .unwrap()
            .fail_commands_of_gone_agent(caller.as_ref());
        assert_eq!(
            state_of(&store, space, &id, sent).await.0,
            store::CommandState::Sent
        );
        // not seen for longer than ONLINE_SECONDS: checked on the next report
        let gone = store::now() - store::ONLINE_SECONDS - 1;
        for m in [&id, &prusa] {
            store.set_last_seen(m, gone).await;
        }
        let caller = store.caller(&token).await.unwrap();
        store
            .lock()
            .unwrap()
            .fail_commands_of_gone_agent(caller.as_ref());
        for c in [sent, accepted] {
            assert_eq!(
                state_of(&store, space, &id, c).await,
                (
                    store::CommandState::Rejected,
                    Some("the agent was gone".to_owned())
                )
            );
        }
        assert_eq!(
            state_of(&store, space, &id, queued).await.0,
            store::CommandState::Queued,
            "not the Queued"
        );
        assert_eq!(
            state_of(&store, space, &other, other_sent).await.0,
            store::CommandState::Sent
        );
        let caller = store.caller(&prusa_token).await.unwrap();
        store
            .lock()
            .unwrap()
            .fail_commands_of_gone_agent(caller.as_ref());
        assert_eq!(
            state_of(&store, space, &prusa, prusa_sent).await.0,
            store::CommandState::Sent,
            "not a Marlin machine"
        );
        // through the report: the Queued command still goes out, as the agent is back
        assert_eq!(report(&app, &token, vec![]).await, vec![queued]);
    }

    /// The late Done, one order (K18): a Done from the port that comes after "the agent was gone" wins,
    /// as `command_went` overwrites; it is the printer's own result. A Done in the same report as the
    /// first one after the gap wins too.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn k18_a_late_done_wins_over_the_agent_was_gone(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let (id, token, space) = agent_machine(&store, MachineKind::Fdm, None).await;
        let a = queue_gcode(&store, space, &id, "M109 S200").await;
        let b = queue_gcode(&store, space, &id, "G28").await;
        report(&app, &token, vec![]).await;
        store
            .set_last_seen(&id, store::now() - store::ONLINE_SECONDS - 1)
            .await;
        // the agent is back and says a is done, in its first report
        report(&app, &token, vec![finished(a)]).await;
        assert_eq!(
            state_of(&store, space, &id, a).await,
            (store::CommandState::Finished, None)
        );
        assert_eq!(
            state_of(&store, space, &id, b).await,
            (
                store::CommandState::Rejected,
                Some("the agent was gone".to_owned())
            )
        );
        // b's Done comes in a later report
        report(&app, &token, vec![finished(b)]).await;
        assert_eq!(
            state_of(&store, space, &id, b).await,
            (store::CommandState::Finished, None)
        );
    }

    /// Two pairings of one code at once (design decisions/20261119-pairing-in-one-space-transaction.md):
    /// one wins, the other finds no machine, also for a Prusa Connect printer, which keeps its code.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn two_pairings_of_one_code_end_with_one_winner(pool: sqlx::PgPool) {
        let store = crate::Store::for_test(&pool).await;
        let (first, _) = store.default_space().await.unwrap();
        let second = store.create_space(first, "Lab").await.unwrap();
        for link in ["agent", "prusa-connect"] {
            let (id, _, code) = store
                .register_linked(
                    &Register {
                        protocol: protocol::VERSION,
                        name: "test".into(),
                        kind: MachineKind::Fdm,
                        agent_version: "0.1.0".into(),
                    },
                    link,
                )
                .await
                .unwrap();
            // The machine's row held, so both pairings are under way before either can take it.
            let mut hold = store.pg().admin_in_tests().begin().await.unwrap();
            sqlx::query("SELECT id FROM machines WHERE id = $1 FOR UPDATE")
                .bind(&id)
                .execute(&mut *hold)
                .await
                .unwrap();
            let pairing = |space: i64| {
                let (store, code) = (store.clone(), code.clone());
                tokio::spawn(async move { store.pair(space, &code).await.unwrap() })
            };
            let (a, b) = (pairing(first), pairing(second));
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            hold.commit().await.unwrap();
            let won: Vec<_> = [a.await.unwrap(), b.await.unwrap()]
                .into_iter()
                .flatten()
                .collect();
            assert_eq!(won.len(), 1, "{link}: {won:?}");
            let space: Option<i64> =
                sqlx::query_scalar("SELECT space_id FROM machines WHERE id = $1")
                    .bind(&id)
                    .fetch_one(store.pg().admin_in_tests())
                    .await
                    .unwrap();
            assert!(
                matches!(space, Some(s) if s == first || s == second),
                "{link}"
            );
            assert_eq!(
                store.pair(first, &code).await.unwrap(),
                None,
                "{link}: used once"
            );
        }
    }
}
