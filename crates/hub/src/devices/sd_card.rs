//! A printer's own SD card, through its agent. The hub makes no G-code for an agent's machine
//! (owner, 2026-11-12): listing and printing the card are typed commands (`Action::SdList`,
//! `Action::SdPrint`), answered by the machine's port. Until a port answers them, the SD tab
//! says it is not available yet (owner's answer to the step 2 review).

use crate::access::Access;
use crate::{Store, failed};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};

/// What the SD tab says until a port lists the card.
pub const NOT_YET: &str = "Not available yet: the SD card comes with this printer's port.";

pub fn routes() -> Router<Store> {
    Router::new()
        .route("/api/machines/{id}/sd", get(look))
        .route("/machines/{id}/sd/print", post(print))
}

/// What is on its card: not available yet.
async fn look(access: Access, State(store): State<Store>, Path(id): Path<String>) -> Response {
    match store.machine(access.space, &id).await {
        Ok(Some(machine)) => Json(serde_json::json!({
            "files": [],
            "online": machine.online,
            "unavailable": NOT_YET,
        }))
        .into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

/// Printing from its card: not available yet.
async fn print() -> Response {
    (StatusCode::CONFLICT, NOT_YET).into_response()
}
