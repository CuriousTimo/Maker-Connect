//! Safety: what someone must have and wear to use a machine (PPE): laser safety glasses at
//! a laser, safety glasses and hearing protection at a CNC. A list per machine (by default
//! what its kind needs); before using it, someone ticks off each item, and that holds for
//! [`CONFIRM_FOR`]. Starting, resuming or framing a job and sending commands wait for it;
//! pausing, stopping, cooling down and switching off never do. A space switches it on
//! (Space → General): a makerspace wants it, a workshop at home does not.

use crate::access::Access;
use crate::store::{Legacy, now};
use crate::{Store, failed};
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

impl Legacy {
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

/// What the PPE rule needs of a machine, read on Postgres before the lock ([`Store::ppe_rule`]):
/// whether its space asks for PPE, and what the machine needs (None: not a machine of the space).
#[derive(Debug, Clone, Default)]
pub struct PpeRule {
    pub checks: bool,
    pub needed: Option<Vec<String>>,
}

impl Store {
    /// The PPE rule for a machine of a space (see [`PpeRule`]).
    pub async fn ppe_rule(&self, space: i64, machine: &str) -> PpeRule {
        let checks = self.ppe_checks(space).await.unwrap_or(false);
        if !checks {
            return PpeRule::default();
        }
        let needed = self.machine_ppe(space, machine).await.ok().flatten();
        PpeRule { checks, needed }
    }

    /// What a machine of the space needs (its own list, or what its kind needs); None when it is
    /// not of the space.
    pub async fn machine_ppe(
        &self,
        space: i64,
        machine: &str,
    ) -> sqlx::Result<Option<Vec<String>>> {
        let mut tx = self.pg().space(space).await?;
        let row = sqlx::query!(
            "SELECT kind, ppe FROM machines WHERE id = $1 AND space_id = $2",
            machine,
            space
        )
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row.map(|r| ppe_of(&r.kind, r.ppe.as_deref())))
    }
}

/// Before using a machine: has this person confirmed its PPE? The refusal otherwise.
/// `rule`: whether the space asks for it and what the machine needs ([`Store::ppe_rule`]).
pub fn check_ppe(
    store: &Legacy,
    rule: &PpeRule,
    access: &Access,
    machine: &str,
) -> Result<(), Box<Response>> {
    if !rule.checks {
        return Ok(());
    }
    let Some(ppe) = &rule.needed else {
        return Ok(());
    };
    if ppe.is_empty() || store.ppe_until(access.user_id, machine).is_some() {
        return Ok(());
    }
    Err(Box::new(
        (
            StatusCode::PRECONDITION_REQUIRED,
            format!(
                "first confirm on its page that you have and wear: {}",
                ppe.join(", ").to_lowercase()
            ),
        )
            .into_response(),
    ))
}

pub fn routes() -> Router<Store> {
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
    State(store): State<Store>,
    Form(form): Form<SwitchForm>,
) -> Response {
    match store.set_ppe_checks(access.space, form.on.is_some()).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => failed(error),
    }
}

/// "I have and wear …": every item ticked (`have=<item>` for each), or nothing.
async fn confirm(
    access: Access,
    State(store): State<Store>,
    Path(machine): Path<String>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    let needed = match store.machine_ppe(access.space, &machine).await {
        Ok(Some(ppe)) => ppe,
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
    let until = store.lock().unwrap().confirm_ppe(access.user_id, &machine);
    until.to_string().into_response()
}

/// A machine's PPE, as its manager sets it: items from the catalogue (`item=`) and one's
/// own (`other=`, comma separated).
async fn set(
    access: Access,
    State(store): State<Store>,
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
    match store.set_ppe(access.space, &machine, &list).await {
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
