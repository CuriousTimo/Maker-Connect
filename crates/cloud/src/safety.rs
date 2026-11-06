//! Safety: what someone must have and wear to use a machine (PPE): laser safety glasses at
//! a laser, safety glasses and hearing protection at a CNC. A list per machine (by default
//! what its kind needs); before using it, someone ticks off each item, and that holds for
//! [`CONFIRM_FOR`]. Starting, resuming or framing a job and sending commands wait for it;
//! pausing, stopping, cooling down and switching off never do. A space switches it on
//! (Space → General): a makerspace wants it, a workshop at home does not.

use crate::access::Access;
use crate::store::{Store, now};
use crate::{Shared, failed};
use axum::{
    Form, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};

/// What to choose from, in the machine's settings (one's own can be added).
pub const CATALOGUE: &[&str] = &[
    "Laser safety glasses",
    "Safety glasses",
    "Hearing protection",
    "Gloves",
    "Dust mask",
    "Respirator",
    "Face shield",
    "Apron",
];

/// How long a confirmation holds: a working day.
pub const CONFIRM_FOR: i64 = 12 * 3600;

/// A machine's PPE: what it was given (comma separated; empty: nothing), else what its kind
/// needs. `kind` as kept: JSON, `"laser"` with its quotes.
pub fn ppe_of(kind: &str, stored: Option<&str>) -> Vec<String> {
    match stored {
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        None => match kind.trim_matches('"') {
            "laser" => vec!["Laser safety glasses".to_owned()],
            "cnc" => vec!["Safety glasses".to_owned(), "Hearing protection".to_owned()],
            _ => Vec::new(),
        },
    }
}

impl Store {
    /// Whether a space asks for PPE before use.
    pub fn ppe_checks(&self, space: i64) -> bool {
        self.db
            .query_row(
                "SELECT ppe_checks FROM spaces WHERE id = ?1",
                [space],
                |r| r.get(0),
            )
            .unwrap_or(false)
    }

    pub fn set_ppe_checks(&mut self, space: i64, on: bool) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE spaces SET ppe_checks = ?2 WHERE id = ?1",
            rusqlite::params![space, on],
        )?;
        self.changed();
        Ok(())
    }

    /// Sets a machine's PPE (an empty list: nothing needed). Returns whether it is in the space.
    pub fn set_ppe(
        &mut self,
        space: i64,
        machine: &str,
        list: &[String],
    ) -> rusqlite::Result<bool> {
        let n = self.db.execute(
            "UPDATE machines SET ppe = ?3 WHERE id = ?1 AND space_id = ?2",
            rusqlite::params![machine, space, list.join(", ")],
        )?;
        // What was confirmed was for the old list.
        self.ppe_ok.retain(|(_, m), _| m != machine);
        self.changed();
        Ok(n > 0)
    }

    /// Someone has and wears the machine's PPE: until when that holds.
    pub fn confirm_ppe(&mut self, user: i64, machine: &str) -> i64 {
        let until = now() + CONFIRM_FOR;
        self.ppe_ok.insert((user, machine.to_owned()), until);
        until
    }

    /// Until when someone's confirmation holds, if it still does.
    pub fn ppe_until(&self, user: i64, machine: &str) -> Option<i64> {
        self.ppe_ok
            .get(&(user, machine.to_owned()))
            .copied()
            .filter(|until| *until > now())
    }
}

/// Before using a machine: has this person confirmed its PPE? The refusal otherwise.
pub fn check_ppe(store: &Store, access: &Access, machine: &str) -> Result<(), Box<Response>> {
    if !store.ppe_checks(access.space) {
        return Ok(());
    }
    let Ok(Some(m)) = store.machine(access.space, machine) else {
        return Ok(());
    };
    if m.ppe.is_empty() || store.ppe_until(access.user_id, machine).is_some() {
        return Ok(());
    }
    Err(Box::new(
        (
            StatusCode::PRECONDITION_REQUIRED,
            format!(
                "first confirm on its page that you have and wear: {}",
                m.ppe.join(", ").to_lowercase()
            ),
        )
            .into_response(),
    ))
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/machines/{id}/ppe", post(confirm))
        .route("/machines/{id}/safety", post(set))
        .route("/space/safety", post(switch))
}

#[derive(serde::Deserialize)]
struct SwitchForm {
    on: Option<String>,
}

/// The space asks for PPE before use, or not.
async fn switch(
    access: Access,
    State(store): State<Shared>,
    Form(form): Form<SwitchForm>,
) -> Response {
    match store
        .lock()
        .unwrap()
        .set_ppe_checks(access.space, form.on.is_some())
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => failed(error),
    }
}

/// "I have and wear …": every item ticked (`have=<item>` for each), or nothing.
async fn confirm(
    access: Access,
    State(store): State<Shared>,
    Path(machine): Path<String>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    let mut store = store.lock().unwrap();
    let needed = match store.machine(access.space, &machine) {
        Ok(Some(m)) => m.ppe,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => return failed(error),
    };
    let ticked: Vec<&str> = form
        .iter()
        .filter(|(k, _)| k == "have")
        .map(|(_, v)| v.as_str())
        .collect();
    let missing: Vec<&String> = needed
        .iter()
        .filter(|item| !ticked.contains(&item.as_str()))
        .collect();
    if !missing.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "not without: {}",
                missing
                    .iter()
                    .map(|s| s.to_lowercase())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
            .into_response();
    }
    let until = store.confirm_ppe(access.user_id, &machine);
    until.to_string().into_response()
}

/// A machine's PPE, as its manager sets it: items from the catalogue (`item=`) and one's
/// own (`other=`, comma separated).
async fn set(
    access: Access,
    State(store): State<Shared>,
    Path(machine): Path<String>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    let mut list: Vec<String> = Vec::new();
    for (k, v) in &form {
        let items: Vec<&str> = match k.as_str() {
            "item" => vec![v.as_str()],
            "other" => v.split(',').collect(),
            _ => continue,
        };
        for item in items {
            let item: String = item.trim().chars().take(40).collect();
            if !item.is_empty() && !list.iter().any(|l| l.eq_ignore_ascii_case(&item)) {
                list.push(item);
            }
        }
    }
    match store.lock().unwrap().set_ppe(access.space, &machine, &list) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

#[cfg(test)]
mod tests {
    use super::ppe_of;

    #[test]
    fn a_kind_needs_its_own() {
        assert_eq!(ppe_of("\"laser\"", None), ["Laser safety glasses"]);
        assert_eq!(
            ppe_of("\"cnc\"", None),
            ["Safety glasses", "Hearing protection"]
        );
        assert!(ppe_of("\"fdm\"", None).is_empty());
        assert!(
            ppe_of("\"laser\"", Some("")).is_empty(),
            "set to nothing: nothing"
        );
        assert_eq!(
            ppe_of("\"fdm\"", Some("Gloves, Face shield ")),
            ["Gloves", "Face shield"]
        );
    }
}
