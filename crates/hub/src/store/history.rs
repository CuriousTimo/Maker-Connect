//! A machine's history in TimescaleDB (design database.md "Events and history", migration
//! `20261120000000_history.sql`): one narrow `sample` row per value, by the metrics below
//! ([`METRICS`], ids in its order from 1, as the migration's `metric` table). Written by the history writer
//! on the admin pool (allowlist #4); read in the space's transaction through the `*_in_space` views
//! (decisions/20261120-history-views-not-rls.md).

use super::*;
use crate::store::pg::Bypass;

/// A metric the history keeps: its name and unit, as the migration's `metric` table has them.
struct Metric {
    name: &'static str,
    /// Read by the test that the migration's table holds the same.
    #[cfg_attr(not(test), allow(dead_code))]
    unit: &'static str,
}

/// The history's metrics, ids in this order from 1 (`20261120000000_history.sql`, `metric`).
const METRICS: &[Metric] = &[
    Metric {
        name: "temp_nozzle",
        unit: "celsius",
    },
    Metric {
        name: "target_nozzle",
        unit: "celsius",
    },
    Metric {
        name: "temp_bed",
        unit: "celsius",
    },
    Metric {
        name: "target_bed",
        unit: "celsius",
    },
    Metric {
        name: "fan_print",
        unit: "rpm",
    },
    Metric {
        name: "fan_hotend",
        unit: "rpm",
    },
];

/// At most one sample per machine this many seconds apart, as the graphs drew them before.
const SAMPLE_SECONDS: i64 = 2;
/// Up to this far back the raw samples are read; up to [`MINUTES_UP_TO`] the minute aggregate;
/// beyond, the hour aggregate. So a graph gets a few hundred to a few thousand points.
const RAW_UP_TO: i64 = 2 * 3600;
const MINUTES_UP_TO: i64 = 3 * 86_400;
/// The furthest back a page may ask.
pub const HISTORY_MAX_SECONDS: i64 = 366 * 86_400;

/// A metric's id in the `metric` table: its place in the registry, from 1.
fn metric_id(name: &str) -> i16 {
    METRICS
        .iter()
        .position(|m| m.name == name)
        .map(|i| i as i16 + 1)
        .unwrap_or_else(|| panic!("{name} is not in METRICS"))
}

/// The values of a report that the history keeps, by metric id.
fn values(telemetry: &Telemetry) -> Vec<(i16, f64)> {
    [
        ("temp_nozzle", telemetry.nozzle_c.map(f64::from)),
        ("target_nozzle", telemetry.nozzle_target_c.map(f64::from)),
        ("temp_bed", telemetry.bed_c.map(f64::from)),
        ("target_bed", telemetry.bed_target_c.map(f64::from)),
        ("fan_print", telemetry.print_fan_rpm.map(f64::from)),
        ("fan_hotend", telemetry.hotend_fan_rpm.map(f64::from)),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.map(|v| (metric_id(name), v)))
    .collect()
}

impl Store {
    /// A paired machine's report into its history ([`Store::report`]).
    pub(crate) async fn record_history(
        &self,
        space: i64,
        machine: &str,
        telemetry: &Telemetry,
    ) -> Result<(), BoxError> {
        let t = now();
        {
            let mut legacy = self.lock().unwrap();
            if legacy
                .last_sample
                .get(machine)
                .is_some_and(|last| t - last < SAMPLE_SECONDS)
            {
                return Ok(());
            }
            legacy.last_sample.insert(machine.to_owned(), t);
        }
        let (metrics, values): (Vec<i16>, Vec<f64>) = values(telemetry).into_iter().unzip();
        if metrics.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO sample (time, space_id, device_id, metric, value)
             SELECT at_unix($1), $2, $3, m, v FROM unnest($4::smallint[], $5::float8[]) AS u(m, v)",
        )
        .bind(t)
        .bind(space)
        .bind(machine)
        .bind(metrics)
        .bind(values)
        .execute(self.pg().admin(Bypass::Writer))
        .await?;
        Ok(())
    }

    /// A machine's history over the last `seconds`, oldest first: the raw samples for a short span,
    /// the minute or hour averages for a longer one. Empty for a machine of another space (the
    /// views show only this space's rows).
    pub async fn history(
        &self,
        space: i64,
        machine: &str,
        seconds: i64,
    ) -> Result<Vec<Sample>, BoxError> {
        let seconds = seconds.clamp(60, HISTORY_MAX_SECONDS);
        let (view, time) = if seconds <= RAW_UP_TO {
            ("sample_in_space", "time")
        } else if seconds <= MINUTES_UP_TO {
            ("sample_1m_in_space", "bucket")
        } else {
            ("sample_1h_in_space", "bucket")
        };
        let mut tx = self.pg().space(space).await?;
        let rows: Vec<(i64, i16, f64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT unix_seconds({time}), metric, value FROM {view}
             WHERE device_id = $1 AND {time} > now() - make_interval(secs => $2)
             ORDER BY {time}, metric"
        )))
        .bind(machine)
        .bind(seconds as f64)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(pivot(rows))
    }
}

/// Narrow rows (time, metric, value), in time order, as one [`Sample`] per time.
fn pivot(rows: Vec<(i64, i16, f64)>) -> Vec<Sample> {
    let mut out: Vec<Sample> = Vec::new();
    for (t, metric, value) in rows {
        if out.last().is_none_or(|s| s.t != t) {
            out.push(Sample {
                t,
                nozzle: None,
                nozzle_target: None,
                bed: None,
                bed_target: None,
                print_fan: None,
                hotend_fan: None,
            });
        }
        let s = out.last_mut().unwrap();
        match METRICS.get(metric as usize - 1).map(|m| m.name) {
            Some("temp_nozzle") => s.nozzle = Some(value as f32),
            Some("target_nozzle") => s.nozzle_target = Some(value as f32),
            Some("temp_bed") => s.bed = Some(value as f32),
            Some("target_bed") => s.bed_target = Some(value as f32),
            Some("fan_print") => s.print_fan = Some(value.round() as u32),
            Some("fan_hotend") => s.hotend_fan = Some(value.round() as u32),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `metric` table holds the registry's metrics, ids in its order.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn the_metric_table_is_the_registry(pool: sqlx::PgPool) {
        let rows: Vec<(i16, String, String)> =
            sqlx::query_as("SELECT id, name, unit FROM metric ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        let registry: Vec<(i16, String, String)> = METRICS
            .iter()
            .enumerate()
            .map(|(i, m)| (i as i16 + 1, m.name.to_owned(), m.unit.to_owned()))
            .collect();
        assert_eq!(rows, registry);
    }

    /// The tables are hypertables with compression and retention, and the aggregates are made.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn the_history_is_on_timescale(pool: sqlx::PgPool) {
        let hypertables: Vec<(String, bool)> = sqlx::query_as(
            "SELECT hypertable_name::text, compression_enabled FROM timescaledb_information.hypertables ORDER BY 1",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(
            hypertables.contains(&("sample".to_owned(), true)),
            "{hypertables:?}"
        );
        let aggregates: Vec<String> = sqlx::query_scalar(
            "SELECT view_name::text FROM timescaledb_information.continuous_aggregates ORDER BY 1",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(aggregates, ["sample_1h", "sample_1m"]);
        let jobs: Vec<(String, String)> = sqlx::query_as(
            "SELECT proc_name::text, hypertable_name::text FROM timescaledb_information.jobs
             WHERE hypertable_name IS NOT NULL ORDER BY 1, 2",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let jobs: Vec<(&str, &str)> = jobs.iter().map(|(p, h)| (p.as_str(), h.as_str())).collect();
        assert!(jobs.contains(&("policy_compression", "sample")), "{jobs:?}");
        assert!(jobs.contains(&("policy_retention", "sample")), "{jobs:?}");
        assert_eq!(
            jobs.iter()
                .filter(|(p, _)| *p == "policy_refresh_continuous_aggregate")
                .count(),
            2,
            "{jobs:?}"
        );
    }

    /// A report goes into the machine's history and reads back from the database, by its space
    /// only; a second report within SAMPLE_SECONDS is not kept.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_report_is_kept_in_the_database(pool: sqlx::PgPool) {
        let store = Store::for_test(&pool).await;
        // `sample` has no key to `spaces`: any two ids are two spaces here.
        let (space, other) = (1, 2);
        let telemetry = Telemetry {
            nozzle_c: Some(215.5),
            nozzle_target_c: Some(215.0),
            bed_c: Some(60.0),
            print_fan_rpm: Some(4800),
            ..Default::default()
        };
        store.record_history(space, "m1", &telemetry).await.unwrap();
        store.record_history(space, "m1", &telemetry).await.unwrap();
        let samples = store.history(space, "m1", HISTORY_SECONDS).await.unwrap();
        assert_eq!(samples.len(), 1, "{samples:?}");
        let s = samples[0];
        assert_eq!(
            (
                s.nozzle,
                s.nozzle_target,
                s.bed,
                s.bed_target,
                s.print_fan,
                s.hotend_fan
            ),
            (Some(215.5), Some(215.0), Some(60.0), None, Some(4800), None)
        );
        assert!((now() - s.t).abs() <= 2);
        assert!(
            store
                .history(other, "m1", HISTORY_SECONDS)
                .await
                .unwrap()
                .is_empty()
        );
        // Over a day: the minute averages, made in real time from the newest samples.
        let day = store.history(space, "m1", 86_400).await.unwrap();
        assert_eq!(day.len(), 1);
        assert_eq!(day[0].nozzle, Some(215.5));
        // A restarted hub reads the same: the history is in the database, not in memory.
        let again = Store::for_test(&pool).await;
        assert_eq!(
            again
                .history(space, "m1", HISTORY_SECONDS)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    /// The app role reaches the history only through the views, and only its space's rows.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn the_app_role_reads_only_its_space(pool: sqlx::PgPool) {
        let store = Store::for_test(&pool).await;
        let space = 1;
        let telemetry = Telemetry {
            nozzle_c: Some(20.0),
            ..Default::default()
        };
        store.record_history(space, "m1", &telemetry).await.unwrap();
        let mut tx = store.pg().space(space).await.unwrap();
        let direct = sqlx::query("SELECT 1 FROM sample")
            .fetch_all(&mut *tx)
            .await;
        assert!(direct.is_err(), "the table itself is not the app role's");
        drop(tx);
        let mut tx = store.pg().space(space + 1_000_000).await.unwrap();
        let seen: i64 = sqlx::query_scalar("SELECT count(*) FROM sample_in_space")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(seen, 0);
    }

    #[test]
    fn rows_become_one_sample_per_time() {
        let s = pivot(vec![(10, 1, 200.0), (10, 5, 4799.6), (12, 1, 201.0)]);
        assert_eq!(s.len(), 2);
        assert_eq!(
            (s[0].t, s[0].nozzle, s[0].print_fan),
            (10, Some(200.0), Some(4800))
        );
        assert_eq!(
            (s[1].t, s[1].nozzle, s[1].print_fan),
            (12, Some(201.0), None)
        );
    }
}
