use crate::{Store, failed, store};
use axum::{
    Form,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use maker_connect_protocol::{self as protocol, Telemetry};

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
                download: format!("/files/{}/download", job.file_id),
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
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use maker_connect_protocol::MachineKind;

    use tower::ServiceExt;

    pub(crate) async fn call(app: &Router, request: Request<Body>) -> (StatusCode, String) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
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
        // A Prusa Connect printer keeps its code: the second pairing must find none.
        let link = "prusa-connect";
        {
            let (id, _, code) = store
                .register_linked("test", MachineKind::Fdm, "0.1.0", link)
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
