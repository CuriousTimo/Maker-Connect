//! Times (wiki Design: Devices and Rights, "Times"): when the space is open (its opening hours and
//! closed days) and when a group gives its right (the group's windows), in the space's time zone
//! (`access/migrations/20261127001000_times.sql`).
//!
//! - No opening hours: the space is always open; a closed day closes it all day all the same.
//! - A group without windows gives its right at all times.
//! - The hub checks at Open and Start only ([`Store::not_now`]); owners and admins always pass, and
//!   a running job is never stopped.

use super::{Access, Level, Role};
use crate::store::{BoxError, Store};
use jiff::{Timestamp, civil, tz::TimeZone};

/// The days of the week, Monday first, as the bits of [`Window::days`] count them.
pub const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// A window of the week: days (Monday bit 0) and minutes from midnight, `starts_min` in,
/// `ends_min` out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub id: i64,
    pub days: u8,
    pub starts_min: u16,
    pub ends_min: u16,
}

impl Window {
    /// Whether the window holds this moment of the week.
    pub fn holds(&self, at: civil::DateTime) -> bool {
        let day = at.weekday().to_monday_zero_offset();
        let minute = u16::try_from(i32::from(at.hour()) * 60 + i32::from(at.minute())).unwrap_or(0);
        self.days & (1 << day) != 0 && self.starts_min <= minute && minute < self.ends_min
    }

    /// As the page shows it: "Mon–Fri 09:00–12:00", "Sat, Sun 10:00–16:00".
    pub fn label(&self) -> String {
        format!(
            "{} {}–{}",
            days_label(self.days),
            hhmm(self.starts_min),
            hhmm(self.ends_min)
        )
    }
}

/// "Mon–Fri", "Mon, Wed", "Every day": runs of three or more days with a dash.
pub fn days_label(days: u8) -> String {
    if days & 0x7f == 0x7f {
        return "Every day".to_owned();
    }
    let mut parts = Vec::new();
    let mut d = 0;
    while d < 7 {
        if days & (1 << d) == 0 {
            d += 1;
            continue;
        }
        let first = d;
        while d + 1 < 7 && days & (1 << (d + 1)) != 0 {
            d += 1;
        }
        match d - first {
            0 => parts.push(DAYS[first].to_owned()),
            1 => {
                parts.push(DAYS[first].to_owned());
                parts.push(DAYS[d].to_owned());
            }
            _ => parts.push(format!("{}–{}", DAYS[first], DAYS[d])),
        }
        d += 1;
    }
    parts.join(", ")
}

/// Minutes from midnight as "09:00" (1440 is "24:00").
pub fn hhmm(minutes: u16) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// "09:00" or "24:00" as minutes from midnight.
pub fn parse_hhmm(text: &str) -> Option<u16> {
    let (h, m) = text.trim().split_once(':')?;
    let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
    let minutes = h.checked_mul(60)?.checked_add(m)?;
    (m < 60 && minutes <= 1440).then_some(minutes)
}

/// Days the space is closed, both ends included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedDays {
    pub id: i64,
    pub first_day: civil::Date,
    pub last_day: civil::Date,
    pub name: String,
}

/// A space's times: its zone, opening hours and closed days.
#[derive(Debug, Clone)]
pub struct SpaceTimes {
    pub time_zone: String,
    pub hours: Vec<Window>,
    pub closed: Vec<ClosedDays>,
}

impl SpaceTimes {
    /// The space's local time at `at`; an unknown zone counts as UTC.
    pub fn local(&self, at: Timestamp) -> civil::DateTime {
        let tz = TimeZone::get(&self.time_zone).unwrap_or(TimeZone::UTC);
        at.to_zoned(tz).datetime()
    }

    /// Why the space is closed at `at`, or `None` when it is open.
    pub fn closed_at(&self, at: civil::DateTime) -> Option<String> {
        let today = at.date();
        if let Some(c) = self
            .closed
            .iter()
            .find(|c| c.first_day <= today && today <= c.last_day)
        {
            return Some(format!("The space is closed today ({}).", c.name));
        }
        if !self.hours.is_empty() && !self.hours.iter().any(|w| w.holds(at)) {
            return Some("The space is closed now (outside its opening hours).".to_owned());
        }
        None
    }
}

/// What a person is about to do, for [`Store::not_now`].
pub enum Right<'a> {
    /// Start a job on a machine (`Level::Use`), or operate it.
    Machine(&'a str, Level),
    /// Open lock `1` of door controller `0`.
    Lock(&'a crate::store::Machine, u32),
}

/// A window from the database: days and minutes as kept.
fn window(id: i64, days: i32, starts: i32, ends: i32) -> Window {
    Window {
        id,
        days: u8::try_from(days & 0x7f).unwrap_or(0),
        starts_min: u16::try_from(starts).unwrap_or(0),
        ends_min: u16::try_from(ends).unwrap_or(0),
    }
}

impl Store {
    /// A space's time zone, opening hours and closed days (those not over yet, first ones first).
    pub async fn space_times(&self, space: i64) -> Result<SpaceTimes, BoxError> {
        let mut tx = self.pg().space(space).await?;
        let time_zone: Option<String> =
            sqlx::query_scalar("SELECT time_zone FROM spaces WHERE id = $1")
                .bind(space)
                .fetch_optional(&mut *tx)
                .await?;
        let hours: Vec<(i64, i32, i32, i32)> = sqlx::query_as(
            "SELECT id, days, starts_min, ends_min FROM time_windows
              WHERE space_id = $1 AND group_id IS NULL ORDER BY starts_min, days DESC, id",
        )
        .bind(space)
        .fetch_all(&mut *tx)
        .await?;
        let closed: Vec<(i64, String, String, String)> = sqlx::query_as(
            "SELECT id, to_char(first_day, 'YYYY-MM-DD'), to_char(last_day, 'YYYY-MM-DD'), name
               FROM closed_days WHERE space_id = $1 ORDER BY first_day, id",
        )
        .bind(space)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(SpaceTimes {
            time_zone: time_zone.unwrap_or_else(|| "UTC".to_owned()),
            hours: hours
                .into_iter()
                .map(|(id, d, s, e)| window(id, d, s, e))
                .collect(),
            closed: closed
                .into_iter()
                .filter_map(|(id, first, last, name)| {
                    Some(ClosedDays {
                        id,
                        first_day: first.parse().ok()?,
                        last_day: last.parse().ok()?,
                        name,
                    })
                })
                .collect(),
        })
    }

    /// Sets the space's time zone; false when the zone is not known.
    pub async fn set_time_zone(&self, space: i64, zone: &str) -> Result<bool, BoxError> {
        let Ok(tz) = TimeZone::get(zone.trim()) else {
            return Ok(false);
        };
        let name = tz.iana_name().unwrap_or(zone.trim()).to_owned();
        let mut tx = self.pg().space(space).await?;
        sqlx::query("UPDATE spaces SET time_zone = $2 WHERE id = $1")
            .bind(space)
            .bind(name)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.changed();
        Ok(true)
    }

    /// The windows of the space's groups, by group.
    pub async fn group_windows(
        &self,
        space: i64,
    ) -> Result<std::collections::HashMap<i64, Vec<Window>>, BoxError> {
        let mut tx = self.pg().space(space).await?;
        let rows: Vec<(i64, i64, i32, i32, i32)> = sqlx::query_as(
            "SELECT group_id, id, days, starts_min, ends_min FROM time_windows
              WHERE space_id = $1 AND group_id IS NOT NULL ORDER BY starts_min, days DESC, id",
        )
        .bind(space)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let mut by_group: std::collections::HashMap<i64, Vec<Window>> =
            std::collections::HashMap::new();
        for (group, id, d, s, e) in rows {
            by_group.entry(group).or_default().push(window(id, d, s, e));
        }
        Ok(by_group)
    }

    /// Adds a window: to the space's opening hours (`group` `None`) or to a group of the space.
    /// False when the group is not of the space or the window is empty.
    pub async fn add_window(
        &self,
        space: i64,
        group: Option<i64>,
        days: u8,
        starts_min: u16,
        ends_min: u16,
    ) -> Result<bool, BoxError> {
        let days = days & 0x7f;
        if days == 0 || ends_min > 1440 || ends_min <= starts_min {
            return Ok(false);
        }
        if let Some(group) = group
            && self.group_name(space, group).await?.is_none()
        {
            return Ok(false);
        }
        let mut tx = self.pg().space(space).await?;
        sqlx::query(
            "INSERT INTO time_windows (space_id, group_id, days, starts_min, ends_min)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(space)
        .bind(group)
        .bind(i32::from(days))
        .bind(i32::from(starts_min))
        .bind(i32::from(ends_min))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Takes a window away: one of the space's opening hours (`group` `None`) or of that group.
    pub async fn remove_window(
        &self,
        space: i64,
        group: Option<i64>,
        id: i64,
    ) -> Result<bool, BoxError> {
        let mut tx = self.pg().space(space).await?;
        let n = sqlx::query(
            "DELETE FROM time_windows WHERE space_id = $1 AND id = $2
                AND group_id IS NOT DISTINCT FROM $3",
        )
        .bind(space)
        .bind(id)
        .bind(group)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n > 0)
    }

    /// Closes the space from `first` to `last`, both included; false when `last` is before `first`
    /// or the name is empty.
    pub async fn add_closed_days(
        &self,
        space: i64,
        first: civil::Date,
        last: civil::Date,
        name: &str,
    ) -> Result<bool, BoxError> {
        let name = name.trim();
        if last < first || name.is_empty() {
            return Ok(false);
        }
        let mut tx = self.pg().space(space).await?;
        sqlx::query(
            "INSERT INTO closed_days (space_id, first_day, last_day, name)
             VALUES ($1, $2::date, $3::date, $4)",
        )
        .bind(space)
        .bind(first.to_string())
        .bind(last.to_string())
        .bind(name)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn remove_closed_days(&self, space: i64, id: i64) -> Result<bool, BoxError> {
        let mut tx = self.pg().space(space).await?;
        let n = sqlx::query("DELETE FROM closed_days WHERE space_id = $1 AND id = $2")
            .bind(space)
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(n > 0)
    }

    /// Why `access` may not do `right` now, for its times, or `None` (wiki Design: Devices and
    /// Rights, "What a time window does"). The rights themselves are checked as before; this only
    /// adds the times: an owner or admin always passes; else the space must be open, and a right
    /// that comes only through groups needs one of them whose windows hold now.
    pub async fn not_now(
        &self,
        access: &Access,
        right: Right<'_>,
    ) -> Result<Option<String>, BoxError> {
        self.not_at(access, right, Timestamp::now()).await
    }

    pub(crate) async fn not_at(
        &self,
        access: &Access,
        right: Right<'_>,
        at: Timestamp,
    ) -> Result<Option<String>, BoxError> {
        if access.role >= Role::Admin {
            return Ok(None);
        }
        let times = self.space_times(access.space).await?;
        let local = times.local(at);
        if let Some(why) = times.closed_at(local) {
            return Ok(Some(why));
        }
        let now = at.as_second();
        let mut tx = self.pg().space(access.space).await?;
        // The groups that give the right: None when the right does not come from groups alone.
        let groups: Option<Vec<i64>> = match right {
            Right::Machine(machine, need) => {
                let open: Option<i32> =
                    sqlx::query_scalar("SELECT induction_required FROM machines WHERE id = $1")
                        .bind(machine)
                        .fetch_optional(&mut *tx)
                        .await?;
                let direct: Vec<String> = sqlx::query_scalar(
                    "SELECT level FROM machine_grants WHERE space_id = $1 AND machine_id = $2
                        AND user_id = $3 AND (expires_at IS NULL OR expires_at > at_unix($4))",
                )
                .bind(access.space)
                .bind(machine)
                .bind(access.user_id)
                .bind(now)
                .fetch_all(&mut *tx)
                .await?;
                let direct = direct
                    .iter()
                    .filter_map(|l| Level::parse(l))
                    .any(|l| l >= need);
                if open == Some(0) || direct {
                    None
                } else {
                    let rows: Vec<(i64, String)> = sqlx::query_as(
                        "SELECT g.group_id, g.level FROM group_grants g
                           JOIN machine_group_members m ON m.group_id = g.group_id
                          WHERE g.space_id = $1 AND m.machine_id = $2 AND g.user_id = $3
                            AND (g.expires_at IS NULL OR g.expires_at > at_unix($4))",
                    )
                    .bind(access.space)
                    .bind(machine)
                    .bind(access.user_id)
                    .bind(now)
                    .fetch_all(&mut *tx)
                    .await?;
                    Some(
                        rows.into_iter()
                            .filter(|(_, l)| Level::parse(l).is_some_and(|l| l >= need))
                            .map(|(g, _)| g)
                            .collect(),
                    )
                }
            }
            Right::Lock(board, lock) => {
                if board.locks_open.contains(&lock) {
                    None
                } else {
                    Some(
                        sqlx::query_scalar(
                            "SELECT l.group_id FROM group_locks l
                               JOIN group_grants g ON g.group_id = l.group_id
                              WHERE l.space_id = $1 AND l.machine_id = $2 AND l.lock_no = $3
                                AND g.user_id = $4
                                AND (g.expires_at IS NULL OR g.expires_at > at_unix($5))",
                        )
                        .bind(access.space)
                        .bind(&board.id)
                        .bind(i32::try_from(lock).unwrap_or(i32::MAX))
                        .bind(access.user_id)
                        .bind(now)
                        .fetch_all(&mut *tx)
                        .await?,
                    )
                }
            }
        };
        tx.commit().await?;
        // No group gives it at all: the rights check refuses it, not the times.
        let Some(groups) = groups.filter(|g| !g.is_empty()) else {
            return Ok(None);
        };
        let windows = self.group_windows(access.space).await?;
        let now_ok = groups.iter().any(|g| {
            windows
                .get(g)
                .is_none_or(|ws| ws.is_empty() || ws.iter().any(|w| w.holds(local)))
        });
        if now_ok {
            return Ok(None);
        }
        let mut when: Vec<String> = groups
            .iter()
            .filter_map(|g| windows.get(g))
            .flatten()
            .map(Window::label)
            .collect();
        when.sort();
        when.dedup();
        Ok(Some(format!(
            "Your group gives this right only at its times: {}.",
            when.join("; ")
        )))
    }
}

/// [`Store::not_now`] as a route's refusal: 403 with the reason, or 500 when it could not be read.
#[allow(clippy::result_large_err)]
pub async fn check_times(
    store: &Store,
    access: &Access,
    right: Right<'_>,
) -> Result<(), axum::response::Response> {
    use axum::{http::StatusCode, response::IntoResponse};
    match store.not_now(access, right).await {
        Ok(None) => Ok(()),
        Ok(Some(why)) => Err((StatusCode::FORBIDDEN, why).into_response()),
        Err(error) => Err(crate::failed(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> civil::DateTime {
        text.parse().unwrap()
    }

    #[test]
    fn a_window_holds_its_days_and_minutes_end_out() {
        // Mon–Fri 09:00–12:00; 2026-11-27 is a Friday, 2026-11-28 a Saturday.
        let w = window(1, 0b1_1111, 9 * 60, 12 * 60);
        assert!(w.holds(at("2026-11-27T09:00")));
        assert!(w.holds(at("2026-11-27T11:59")));
        assert!(!w.holds(at("2026-11-27T12:00")));
        assert!(!w.holds(at("2026-11-27T08:59")));
        assert!(!w.holds(at("2026-11-28T10:00")));
        // Till midnight.
        let late = window(2, 0b100_0000, 19 * 60, 1440);
        assert!(late.holds(at("2026-11-29T23:59")));
    }

    #[test]
    fn labels() {
        assert_eq!(window(1, 0b1_1111, 540, 720).label(), "Mon–Fri 09:00–12:00");
        assert_eq!(days_label(0b110_0000), "Sat, Sun");
        assert_eq!(days_label(0b101), "Mon, Wed");
        assert_eq!(days_label(0x7f), "Every day");
        assert_eq!(days_label(0b100_1101), "Mon, Wed, Thu, Sun");
        assert_eq!(hhmm(1440), "24:00");
        assert_eq!(parse_hhmm("09:30"), Some(570));
        assert_eq!(parse_hhmm("24:00"), Some(1440));
        assert_eq!(parse_hhmm("24:01"), None);
        assert_eq!(parse_hhmm("9:60"), None);
    }

    #[test]
    fn the_space_is_closed_on_closed_days_and_outside_its_hours() {
        let mut t = SpaceTimes {
            time_zone: "Europe/Amsterdam".to_owned(),
            hours: Vec::new(),
            closed: vec![ClosedDays {
                id: 1,
                first_day: "2026-12-24".parse().unwrap(),
                last_day: "2026-12-26".parse().unwrap(),
                name: "Christmas".to_owned(),
            }],
        };
        // No hours: always open, but not on a closed day.
        assert_eq!(t.closed_at(at("2026-11-27T03:00")), None);
        assert!(
            t.closed_at(at("2026-12-26T12:00"))
                .unwrap()
                .contains("Christmas")
        );
        assert_eq!(t.closed_at(at("2026-12-27T12:00")), None);
        t.hours.push(window(1, 0b1_1111, 540, 1320));
        assert_eq!(t.closed_at(at("2026-11-27T10:00")), None);
        assert!(t.closed_at(at("2026-11-28T10:00")).is_some());
        // 08:30 UTC is 09:30 in Amsterdam in November.
        let local = t.local("2026-11-27T08:30:00Z".parse().unwrap());
        assert_eq!(local, at("2026-11-27T09:30"));
    }
}
