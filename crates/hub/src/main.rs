use maker_connect_hub::store::{self, Legacy, Store};
use maker_connect_hub::{
    accounts, app, farm, queue, samples, spoolmandb, updates, virtual_machines,
};

const USAGE: &str = "Maker Connect hub (proof of concept)

Usage:
  maker-connect-hub [--listen 0.0.0.0:8460] [--db maker-connect.db] [--keep-dumps 7]
                    [--pg-command <prefix>]
  maker-connect-hub reset-password <name> [--db maker-connect.db]

Open http://localhost:8460. On a fresh install, make the first account with the setup code
this prints at start; then add machines with the code their agent shows.

reset-password gives an account a new password (printed once) and signs it out everywhere:
for when its owner cannot sign in any more.

The Postgres database comes from the DATABASE_URL environment variable (postgres://…; never a
flag: a command line is there for anyone on the computer to read, password and all), with the
login maker_connect_hub: scripts/db-roles.sql makes the roles once per server (as its superuser),
and the database is owned by maker_connect_owner. A login that is a superuser or owns tables is
refused.

The database is dumped (pg_dump) every night and before an update, into maker-connect-dumps/ beside
maker-connect-files/; the last --keep-dumps of each are kept (7 unless said). pg_dump, pg_restore and
psql must be of the server's version, found on PATH. --pg-command names a prefix that runs them
elsewhere (such as docker exec -i -e PGPASSWORD <container>), which gives the hub Docker's rights.
";

/// What a release's archive brings beside the hub, into the folders beside its database.
/// Never the other way: the archive holds no data, so unpacking a new release over an old
/// one leaves the database, the files and the backups as they were.
///
/// - `agents/`: the agents of this release go to the downloads when they are another build
///   than those offered (`.build` last, so a copy cut short is done again next time).
/// - `spoolmandb/`: SpoolmanDB as of the release, only when there is no copy yet.
fn from_release(store: &store::Legacy) {
    let Some(here) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
    else {
        return;
    };
    let agents = here.join("agents");
    let build = |dir: &std::path::Path| std::fs::read(dir.join(".build")).ok();
    if let Some(new) = build(&agents)
        && build(&store.downloads_dir).as_ref() != Some(&new)
    {
        let copied = std::fs::read_dir(&agents)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with("maker-connect-agent-"))
            })
            .all(|e| std::fs::copy(e.path(), store.downloads_dir.join(e.file_name())).is_ok());
        if copied && std::fs::write(store.downloads_dir.join(".build"), &new).is_ok() {
            println!(
                "offering this release's agents: {}",
                String::from_utf8_lossy(&new).lines().next().unwrap_or("")
            );
        }
    }
    let seed = here.join("spoolmandb");
    let kept = store.files_dir.join("spoolmandb");
    if seed.join("filaments.json").is_file() && !kept.join("filaments.json").is_file() {
        let _ = std::fs::create_dir_all(&kept);
        for name in ["materials.json", "filaments.json"] {
            let _ = std::fs::copy(seed.join(name), kept.join(name));
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut listen = "0.0.0.0:8460".to_owned();
    let mut db = "maker-connect.db".to_owned();
    let mut reset: Option<String> = None;
    let database_url = std::env::var("DATABASE_URL").ok();
    // A new build tried before it is put in place (crate::updates): it opens the database (a
    // copy), builds its pages, takes a port, and ends.
    let mut trial = false;
    // The copy of the database a trial runs on (crate::updates::prepare_hub).
    let mut trial_database: Option<String> = None;
    let mut keep_dumps = maker_connect_hub::backup::KEEP;
    let mut pg_command = String::new();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--listen" => listen = args.next().ok_or("expected an address after --listen")?,
            "--db" => db = args.next().ok_or("expected a file after --db")?,
            "--trial" => trial = true,
            "--trial-database" => {
                trial_database = Some(
                    args.next()
                        .ok_or("expected a name after --trial-database")?,
                )
            }
            "--keep-dumps" => {
                keep_dumps = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .filter(|n| *n > 0)
                    .ok_or("expected a number (1 or more) after --keep-dumps")?
            }
            "--pg-command" => {
                pg_command = args.next().ok_or("expected a command after --pg-command")?
            }
            "reset-password" => {
                reset = Some(
                    args.next()
                        .ok_or("expected a sign-in name after reset-password")?,
                )
            }
            _ => {
                print!("{USAGE}");
                return Ok(());
            }
        }
    }
    let mut database_url = database_url.ok_or("expected DATABASE_URL (postgres://…)")?;
    if trial {
        if let Some(copy) = &trial_database {
            database_url = maker_connect_hub::backup::with_database(&database_url, copy)?;
        }
        let opened = Store::open(&db, &database_url)
            .await
            .map_err(|e| e.to_string())?;
        let _app = app(opened);
        let _port = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        println!("trial: {} opens its database and starts", store::BUILD);
        return Ok(());
    }
    let dumps = maker_connect_hub::backup::Dumps::new(
        maker_connect_hub::backup::dir_beside(&Legacy::files_dir_of(&db)),
        keep_dumps,
        &pg_command,
        &database_url,
    )?;
    let _ = maker_connect_hub::backup::DUMPS.set(dumps);
    // An update under way: this build on trial, or one to go back from.
    let exe = std::env::current_exe()?;
    let again: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let on_trial = if reset.is_some() {
        None
    } else {
        match maker_connect_update::on_start(&exe) {
            maker_connect_update::Start::Plain => None,
            maker_connect_update::Start::OnTrial(t) => {
                println!("updated from {} to {}: on trial", t.from, t.to);
                Some(t)
            }
            maker_connect_update::Start::RollBack(t, why) => go_back(&exe, &again, &t, &why),
        }
    };
    let opened = match Legacy::open(&db) {
        Ok(store) => store,
        Err(error) => match &on_trial {
            Some(t) => go_back(
                &exe,
                &again,
                t,
                &format!("it could not open the database: {error}"),
            ),
            None => return Err(error.into()),
        },
    };
    let pg = match store::pg::Pg::connect(&database_url).await {
        Ok(pg) => pg,
        Err(error) => match &on_trial {
            Some(t) => go_back(
                &exe,
                &again,
                t,
                &format!("it could not open the database: {error}"),
            ),
            None => return Err(error.into()),
        },
    };
    if let Some(name) = reset {
        let store = Store::new(opened, pg).await?;
        let Some(user) = store.user_id(&name).await? else {
            return Err(format!("no account named {name:?}").into());
        };
        let password = accounts::random_token()[..20].to_owned();
        store
            .set_password(user, &accounts::hash_password(&password))
            .await?;
        println!(
            "{name}'s new password: {password}\n(signed out everywhere; change it after signing in)"
        );
        return Ok(());
    }
    from_release(&opened);
    // SpoolmanDB, kept beside the uploaded files and fetched every hour.
    spoolmandb::start(opened.files_dir.join("spoolmandb"));
    let store = Store::new(opened, pg).await?;
    let reread = store.reread_files().await?;
    if reread > 0 {
        println!("read {reread} kept files again");
    }
    store.end_old_sessions().await?;
    // The work on its own (stopped when a new build takes over, below).
    let mut tasks = vec![
        tokio::spawn(virtual_machines::run(store.clone())),
        tokio::spawn(farm::run(store.clone())),
        tokio::spawn(samples::run(store.clone())),
        tokio::spawn(queue::run(store.clone())),
    ];
    if let Some(dumps) = maker_connect_hub::backup::DUMPS.get() {
        tasks.push(tokio::spawn(maker_connect_hub::backup::nightly(dumps)));
    }
    // The cameras' sessions (crate::cameras), when the hub is built with them.
    #[cfg(feature = "cameras")]
    maker_connect_hub::cameras::start_all(&store).await;
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(listener) => listener,
        Err(error) => match &on_trial {
            Some(t) => go_back(
                &exe,
                &again,
                t,
                &format!("it could not listen on {listen}: {error}"),
            ),
            None => return Err(error.into()),
        },
    };
    println!("Maker Connect hub on http://{listen} (data in {db})");
    // A new build that serves for a while works: it stays.
    if let Some(t) = on_trial {
        let exe = exe.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            maker_connect_update::confirm(&exe, &t);
            println!("{} works: the update is done", t.to);
        });
    }
    if let Some(code) = store.setup_code() {
        println!("no accounts yet: open it in a browser and make the first with setup code {code}");
    }
    let serving = axum::serve(
        listener,
        app(store.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(stop_asked());
    tokio::select! {
        served = serving => return Ok(served?),
        // A new build is in place (crate::updates): this one stops serving (its port free).
        () = updates::UPDATE.notified() => {}
    }
    // The new build runs as this one's child, on trial, on the database (migrated by it): nothing of
    // this one may write meanwhile. Its own work stops, and its database closes, so what still runs
    // (an agent's open connection, the cameras) gets errors, not rows. Then the new build for good,
    // or this one back.
    for task in &tasks {
        task.abort();
    }
    if tokio::time::timeout(std::time::Duration::from_secs(5), store.close())
        .await
        .is_err()
    {
        eprintln!(
            "the database is closed for this build; a connection still in use is left to end"
        );
    }
    let (program, args) = (exe.clone(), again.clone());
    let tried = tokio::task::spawn_blocking(move || {
        let _held = store.lock().unwrap();
        maker_connect_update::supervise(&program, &args)
    })
    .await?;
    match tried {
        Ok(maker_connect_update::Supervised::Confirmed) => {
            println!("the new build works: starting it for good");
            maker_connect_update::restart(&exe, &again)
        }
        Ok(maker_connect_update::Supervised::Failed(t, why)) => go_back(&exe, &again, &t, &why),
        Err(error) => {
            eprintln!("could not try the new build: {error}");
            maker_connect_update::restart(&exe, &again)
        }
    }
}

/// Back to the build before an update that did not work: its database as it was, then that
/// build in this one's place.
fn go_back(
    exe: &std::path::Path,
    again: &[std::ffi::OsString],
    trial: &maker_connect_update::Trial,
    why: &str,
) -> ! {
    eprintln!(
        "the update to {} goes back to {}: {why}",
        trial.to, trial.from
    );
    // The database as the dump before the update holds it, then the build before.
    let mut why = why.to_owned();
    if let Some(dumps) = maker_connect_hub::backup::DUMPS.get() {
        match maker_connect_hub::backup::back_as_before(dumps, &trial.note) {
            Ok(Some(dump)) => println!("the database is back as {} holds it", dump.display()),
            Ok(None) => {}
            Err(error) => {
                eprintln!("could not bring the database back: {error}");
                why = format!("{why} (the database could not be brought back: {error})");
            }
        }
    }
    let why = why.as_str();
    if let Err(error) = maker_connect_update::roll_back(exe, trial, why) {
        eprintln!("could not go back: {error}");
        std::process::exit(1);
    }
    maker_connect_update::restart(exe, again)
}

/// Ends when someone stops the hub (Ctrl-C, or `kill`/systemd/Docker's SIGTERM): no new
/// connections from then on. Open pages keep their live streams open for ever, so waiting
/// for them would never end: after a few seconds, or a second Ctrl-C, the process ends
/// regardless (SQLite's journal keeps the database whole).
async fn stop_asked() {
    stop_signal().await;
    println!("stopping (Ctrl-C again to stop at once)");
    tokio::spawn(async {
        tokio::select! {
            _ = stop_signal() => {}
            _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {}
        }
        println!("stopped");
        std::process::exit(0);
    });
}

async fn stop_signal() {
    #[cfg(unix)]
    {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async {
                match term.as_mut() {
                    Some(term) => { term.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
