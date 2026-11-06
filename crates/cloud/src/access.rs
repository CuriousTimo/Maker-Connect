//! Who may do what, in which space: roles, rights per machine, invites, API tokens and the
//! audit log. The model of the wiki's *Design: Multi-Tenant Cloud*:
//!
//! - A **membership** puts a user in a space with a role: *owner* (everything, and owners),
//!   *admin* (machines, members, tokens), *member* (files, materials, machines as far as
//!   their rights go), *guest* (looks only).
//! - A machine is open to members (they may run jobs and operate it), or needs an
//!   **induction**: then a member may use it only as far as a grant says (*use*: jobs and
//!   labels; *operate*: also move, heat, send G-code), until the grant's end if it has one.
//!   Owners and admins manage every machine; guests only look.
//! - **API tokens** belong to a space and act with a role (at most their maker's); Moonraker
//!   and the OctoPrint plugin, which send no headers, put theirs in the address
//!   (`http://host:8460/t/<token>`), other clients send `Authorization: Bearer`.
//! - Every route says what it needs ([`RULES`]); the guard applies it after the route is
//!   matched ([`permit`]), so a route without a rule is refused, never let through. What
//!   changed something goes into the space's **audit log**, with who did it.
//!
//! The space is the boundary: a request carries one [`Access`], and handlers take the space
//! from it, nowhere else.

use crate::Shared;
use crate::accounts::{random_token, token_hash};
use crate::store::{Store, now};
use axum::{
    extract::{FromRequestParts, MatchedPath, RawPathParams, Request, State},
    http::{Method, StatusCode, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

/// A role in a space, from least to most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Guest,
    Member,
    Admin,
    Owner,
}

impl Role {
    pub fn parse(text: &str) -> Option<Role> {
        Some(match text {
            "guest" => Role::Guest,
            "member" => Role::Member,
            "admin" => Role::Admin,
            "owner" => Role::Owner,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Guest => "guest",
            Role::Member => "member",
            Role::Admin => "admin",
            Role::Owner => "owner",
        }
    }

    pub const ALL: [Role; 4] = [Role::Owner, Role::Admin, Role::Member, Role::Guest];
}

/// What someone may do with one machine, from least to most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// See it.
    View,
    /// Run jobs on it, print labels, say what is loaded.
    Use,
    /// Also move it, heat it, send it G-code.
    Operate,
    /// Also its settings, its access, removing it.
    Manage,
}

impl Level {
    pub fn parse(text: &str) -> Option<Level> {
        Some(match text {
            "view" => Level::View,
            "use" => Level::Use,
            "operate" => Level::Operate,
            "manage" => Level::Manage,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Level::View => "view",
            Level::Use => "use",
            Level::Operate => "operate",
            Level::Manage => "manage",
        }
    }
}

/// Who a request acts for, in which space.
#[derive(Debug, Clone, Serialize)]
pub struct Access {
    /// The person (for a token: who made it).
    pub user_id: i64,
    /// As the audit log says it: their name, or the token's.
    pub actor: String,
    pub space: i64,
    pub role: Role,
    #[serde(skip)]
    pub token_id: Option<i64>,
    #[serde(skip)]
    pub session_id: Option<i64>,
    pub instance_admin: bool,
}

impl<S: Send + Sync> FromRequestParts<S> for Access {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Access>()
            .cloned()
            .ok_or_else(|| (StatusCode::FORBIDDEN, "not in a space").into_response())
    }
}

/// What a route needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Machines with their own tokens, and the pages to sign in: the guard lets them by.
    Public,
    /// Signed in, in a space or not (signing out, one's own account, choosing a space).
    Signed,
    /// A role in the space, at least.
    Role(Role),
    /// A right on the machine in the path (`{id}`), at least; it must be in the space.
    Machine(Level),
    /// Runs the server.
    InstanceAdmin,
}

/// Every route, by method and path as the router has it, with what it needs and, for what
/// changes something, how the audit log says it (`None`: not logged). A route missing here
/// is refused. GET and HEAD not listed need [`Role::Guest`].
pub const RULES: &[(&str, &str, Need, Option<&str>)] = &[
    // Signing in, and machines that bring their own tokens.
    ("*", "/login", Need::Public, None),
    ("*", "/setup", Need::Public, None),
    ("*", "/logo.svg", Need::Public, None),
    ("*", maker_connect_protocol::REGISTER, Need::Public, None),
    ("*", maker_connect_protocol::TELEMETRY, Need::Public, None),
    ("*", "/api/agent/v1/jobs/{id}/file", Need::Public, None),
    ("*", "/p/register", Need::Public, None),
    ("*", "/p/telemetry", Need::Public, None),
    ("*", "/p/events", Need::Public, None),
    // A Prusa fetches a file sent to it: the transfer's iv in the address is its key.
    ("GET", "/f/{iv}/raw", Need::Public, None),
    ("*", "/simplyprint/{token}", Need::Public, None),
    (
        "*",
        "/simplyprint/{token}/jobs/{job}/file",
        Need::Public,
        None,
    ),
    ("*", "/simplyprint/{token}/{*rest}", Need::Public, None),
    ("*", "/invite/{token}", Need::Public, None),
    ("GET", "/api/apps/v1/info", Need::Public, None),
    // Programs with an API token (crate::apps).
    (
        "POST",
        "/api/apps/v1/files",
        Need::Role(Role::Member),
        Some("sent a file from a program"),
    ),
    // One's own.
    ("POST", "/logout", Need::Signed, None),
    ("GET", "/api/me", Need::Signed, None),
    ("GET", "/welcome", Need::Signed, None),
    ("POST", "/spaces/switch", Need::Signed, None),
    ("POST", "/spaces", Need::Signed, None),
    ("POST", "/account/password", Need::Signed, None),
    ("POST", "/instance/open-spaces", Need::InstanceAdmin, None),
    // The space.
    (
        "POST",
        "/space/name",
        Need::Role(Role::Admin),
        Some("renamed the space"),
    ),
    (
        "POST",
        "/organisation/name",
        Need::Role(Role::Admin),
        Some("renamed the organisation"),
    ),
    (
        "POST",
        "/space/invites",
        Need::Role(Role::Admin),
        Some("made an invite link"),
    ),
    (
        "POST",
        "/space/invites/{id}/revoke",
        Need::Role(Role::Admin),
        Some("revoked an invite link"),
    ),
    (
        "POST",
        "/space/members/{user}/role",
        Need::Role(Role::Admin),
        Some("changed a member's role"),
    ),
    (
        "POST",
        "/space/members/{user}/remove",
        Need::Role(Role::Admin),
        Some("removed a member"),
    ),
    (
        "POST",
        "/space/tokens",
        Need::Role(Role::Admin),
        Some("made an API token"),
    ),
    (
        "POST",
        "/space/tokens/{id}/revoke",
        Need::Role(Role::Admin),
        Some("revoked an API token"),
    ),
    ("GET", "/api/space/audit", Need::Role(Role::Admin), None),
    // Machines.
    (
        "POST",
        "/pair",
        Need::Role(Role::Admin),
        Some("added a machine"),
    ),
    (
        "POST",
        "/virtual",
        Need::Role(Role::Admin),
        Some("added a virtual machine"),
    ),
    (
        "POST",
        "/klipper",
        Need::Role(Role::Admin),
        Some("added a Klipper printer"),
    ),
    (
        "POST",
        "/machines/{id}/profile",
        Need::Machine(Level::Manage),
        Some("chose the profile of"),
    ),
    (
        "POST",
        "/machines/{id}/name",
        Need::Machine(Level::Manage),
        Some("renamed"),
    ),
    (
        "POST",
        "/machines/{id}/labels",
        Need::Machine(Level::Manage),
        Some("labelled"),
    ),
    // Also needs owner or admin in the space it goes to; the handler checks that, and writes
    // the audit log of both spaces.
    (
        "POST",
        "/machines/{id}/move",
        Need::Machine(Level::Manage),
        None,
    ),
    (
        "POST",
        "/machines/{id}/remove",
        Need::Machine(Level::Manage),
        Some("removed"),
    ),
    (
        "POST",
        "/machines/{id}/out-of-use",
        Need::Machine(Level::Manage),
        Some("changed whether in use"),
    ),
    (
        "POST",
        "/machines/{id}/access",
        Need::Machine(Level::Manage),
        Some("changed who may use"),
    ),
    // The handlers write the audit log: on which group or machine, for whom.
    (
        "POST",
        "/permissions/people/{user}",
        Need::Role(Role::Admin),
        None,
    ),
    (
        "POST",
        "/permissions/groups/{group}/people/{user}",
        Need::Role(Role::Admin),
        None,
    ),
    (
        "POST",
        "/permissions/people/{user}/permission",
        Need::Role(Role::Admin),
        None,
    ),
    ("POST", "/permissions/groups", Need::Role(Role::Admin), None),
    (
        "POST",
        "/permissions/groups/{group}/remove",
        Need::Role(Role::Admin),
        None,
    ),
    (
        "POST",
        "/permissions/groups/{group}/machines",
        Need::Role(Role::Admin),
        None,
    ),
    (
        "POST",
        "/permissions/groups/{group}/machines/{machine}/remove",
        Need::Role(Role::Admin),
        None,
    ),
    (
        "POST",
        "/permissions/groups/{group}/grants",
        Need::Role(Role::Admin),
        None,
    ),
    (
        "POST",
        "/permissions/groups/{group}/grants/{user}/remove",
        Need::Role(Role::Admin),
        None,
    ),
    (
        "POST",
        "/machines/{id}/grants",
        Need::Machine(Level::Manage),
        Some("gave permission on"),
    ),
    (
        "POST",
        "/machines/{id}/grants/{user}/remove",
        Need::Machine(Level::Manage),
        Some("took permission away on"),
    ),
    (
        "POST",
        "/machines/{id}/laser",
        Need::Machine(Level::Manage),
        Some("set the laser module of"),
    ),
    (
        "POST",
        "/machines/{id}/command",
        Need::Machine(Level::Operate),
        Some("sent a command to"),
    ),
    (
        "POST",
        "/machines/{id}/job",
        Need::Machine(Level::Use),
        Some("paused, resumed or stopped the job on"),
    ),
    (
        "POST",
        "/machines/{id}/taken",
        Need::Machine(Level::Use),
        Some("took the print off"),
    ),
    (
        "POST",
        "/machines/{id}/media",
        Need::Machine(Level::Use),
        Some("said what is loaded in"),
    ),
    // A Prusa's USB stick (crate::printer_files).
    (
        "GET",
        "/api/machines/{id}/usb",
        Need::Machine(Level::View),
        None,
    ),
    (
        "GET",
        "/machines/{id}/usb/preview",
        Need::Machine(Level::View),
        None,
    ),
    (
        "POST",
        "/machines/{id}/usb/print",
        Need::Machine(Level::Use),
        Some("printed from the USB stick of"),
    ),
    (
        "POST",
        "/machines/{id}/usb/delete",
        Need::Machine(Level::Use),
        Some("deleted from the USB stick of"),
    ),
    (
        "POST",
        "/machines/{id}/usb/folder",
        Need::Machine(Level::Use),
        Some("made a folder on the USB stick of"),
    ),
    (
        "POST",
        "/machines/{id}/usb/send",
        Need::Machine(Level::Use),
        Some("sent a file to the USB stick of"),
    ),
    // The farm (crate::farm): projects and parts are work; confirming a plate is using the
    // machine; a machine joins the queue by who manages it; a count corrected by an admin.
    (
        "POST",
        "/api/projects",
        Need::Role(Role::Member),
        Some("made a project"),
    ),
    (
        "POST",
        "/api/projects/{id}",
        Need::Role(Role::Member),
        Some("changed a project"),
    ),
    (
        "POST",
        "/api/projects/{id}/remove",
        Need::Role(Role::Member),
        Some("removed a project"),
    ),
    (
        "POST",
        "/api/projects/{id}/parts",
        Need::Role(Role::Member),
        Some("added a part"),
    ),
    (
        "POST",
        "/api/parts/{id}",
        Need::Role(Role::Member),
        Some("changed a part"),
    ),
    (
        "POST",
        "/api/parts/{id}/remove",
        Need::Role(Role::Member),
        Some("removed a part"),
    ),
    (
        "POST",
        "/api/parts/{id}/files",
        Need::Role(Role::Member),
        Some("gave a part a file"),
    ),
    (
        "POST",
        "/api/parts/{id}/files/{file}/remove",
        Need::Role(Role::Member),
        Some("took a file from a part"),
    ),
    (
        "POST",
        "/api/parts/{id}/count",
        Need::Role(Role::Admin),
        Some("corrected a part's count"),
    ),
    (
        "POST",
        "/machines/{id}/farm",
        Need::Machine(Level::Manage),
        Some("changed whether the queue sends work to"),
    ),
    // Asking the staff for permission on a machine one may only look at, and answering.
    (
        "POST",
        "/machines/{id}/ask",
        Need::Machine(Level::View),
        Some("asked for permission on"),
    ),
    (
        "POST",
        "/permissions/requests/{id}/{answer}",
        Need::Role(Role::Admin),
        None,
    ),
    (
        "POST",
        "/space/safety",
        Need::Role(Role::Admin),
        Some("switched the PPE checks of the space"),
    ),
    // A machine's queue (crate::queue): reordering, taking out. Everyone may look; the
    // handler lets people change their own prints, and staff anything.
    (
        "POST",
        "/machines/{id}/queue/order",
        Need::Machine(Level::View),
        None,
    ),
    (
        "POST",
        "/machines/{id}/queue/{item}/remove",
        Need::Machine(Level::View),
        Some("took a file out of the queue of"),
    ),
    // Safety: "I have and wear …" (crate::safety), and what a machine needs.
    (
        "POST",
        "/machines/{id}/ppe",
        Need::Machine(Level::Use),
        Some("confirmed having and wearing the PPE for"),
    ),
    (
        "POST",
        "/machines/{id}/safety",
        Need::Machine(Level::Manage),
        Some("changed the PPE of"),
    ),
    (
        "POST",
        "/machines/{id}/sheet",
        Need::Machine(Level::Use),
        Some("swapped the build plate of"),
    ),
    (
        "POST",
        "/machines/{id}/upkeep",
        Need::Machine(Level::Manage),
        Some("set the service interval of"),
    ),
    (
        "POST",
        "/machines/{id}/serviced",
        Need::Machine(Level::Manage),
        Some("did the maintenance of"),
    ),
    (
        "POST",
        "/machines/{id}/ready",
        Need::Machine(Level::Use),
        Some("set ready"),
    ),
    (
        "POST",
        "/machines/{id}/bad",
        Need::Machine(Level::Use),
        Some("marked a bad print on"),
    ),
    // Files: the machine is in the form; the handler checks its right.
    (
        "POST",
        "/api/files",
        Need::Role(Role::Member),
        Some("uploaded a file"),
    ),
    (
        "POST",
        "/api/files/examples",
        Need::Role(Role::Member),
        Some("added the example files"),
    ),
    (
        "POST",
        "/files/{id}/remove",
        Need::Role(Role::Member),
        Some("removed a file"),
    ),
    (
        "POST",
        "/api/files/remove",
        Need::Role(Role::Member),
        Some("removed files"),
    ),
    (
        "POST",
        "/files/{id}/print",
        Need::Role(Role::Member),
        Some("started a job with"),
    ),
    (
        "POST",
        "/files/{id}/frame",
        Need::Role(Role::Member),
        Some("framed"),
    ),
    ("GET", "/api/codes", Need::Role(Role::Guest), None),
    // Materials.
    (
        "POST",
        "/api/stock/materials",
        Need::Role(Role::Member),
        Some("added a material"),
    ),
    (
        "POST",
        "/api/stock/materials/{id}",
        Need::Role(Role::Member),
        Some("changed a material"),
    ),
    (
        "POST",
        "/api/stock/locations",
        Need::Role(Role::Member),
        Some("added a location"),
    ),
    (
        "POST",
        "/api/stock/locations/{id}",
        Need::Role(Role::Member),
        Some("changed a location"),
    ),
    (
        "DELETE",
        "/api/stock/locations/{id}",
        Need::Role(Role::Member),
        Some("removed a location"),
    ),
    (
        "POST",
        "/api/stock/receive",
        Need::Role(Role::Member),
        Some("received stock"),
    ),
    (
        "POST",
        "/api/stock/items/{id}/move",
        Need::Role(Role::Member),
        Some("moved"),
    ),
    (
        "POST",
        "/api/stock/items/{id}/load",
        Need::Role(Role::Member),
        Some("loaded"),
    ),
    (
        "POST",
        "/api/stock/items/{id}/unload",
        Need::Role(Role::Member),
        Some("unloaded"),
    ),
    (
        "POST",
        "/api/stock/items/{id}/amount",
        Need::Role(Role::Member),
        Some("set the amount of"),
    ),
    (
        "POST",
        "/api/stock/items/{id}/used-up",
        Need::Role(Role::Member),
        Some("used up"),
    ),
    (
        "POST",
        "/api/stock/catalogue/{external_id}/take",
        Need::Role(Role::Member),
        Some("took from SpoolmanDB"),
    ),
    (
        "POST",
        "/api/stock/import/spoolman",
        Need::Role(Role::Admin),
        Some("imported from Spoolman"),
    ),
    (
        "POST",
        "/api/stock/export/spoolman",
        Need::Role(Role::Admin),
        Some("exported to Spoolman"),
    ),
    // Spoolman's API (Moonraker, OctoPrint, the library page).
    (
        "POST",
        "/api/v1/vendor",
        Need::Role(Role::Member),
        Some("added a vendor"),
    ),
    (
        "PATCH",
        "/api/v1/vendor/{id}",
        Need::Role(Role::Member),
        Some("changed vendor"),
    ),
    (
        "DELETE",
        "/api/v1/vendor/{id}",
        Need::Role(Role::Member),
        Some("removed vendor"),
    ),
    (
        "POST",
        "/api/v1/filament",
        Need::Role(Role::Member),
        Some("added a filament"),
    ),
    (
        "PATCH",
        "/api/v1/filament/{id}",
        Need::Role(Role::Member),
        Some("changed filament"),
    ),
    (
        "DELETE",
        "/api/v1/filament/{id}",
        Need::Role(Role::Member),
        Some("removed filament"),
    ),
    (
        "POST",
        "/api/v1/spool",
        Need::Role(Role::Member),
        Some("added a spool"),
    ),
    (
        "PATCH",
        "/api/v1/spool/{id}",
        Need::Role(Role::Member),
        Some("changed spool"),
    ),
    (
        "DELETE",
        "/api/v1/spool/{id}",
        Need::Role(Role::Member),
        Some("removed spool"),
    ),
    (
        "PUT",
        "/api/v1/spool/{id}/use",
        Need::Role(Role::Member),
        None,
    ),
    (
        "PUT",
        "/api/v1/spool/{id}/measure",
        Need::Role(Role::Member),
        Some("weighed spool"),
    ),
    (
        "PATCH",
        "/api/v1/location/{*name}",
        Need::Role(Role::Member),
        Some("renamed a location"),
    ),
    (
        "POST",
        "/api/v1/setting/{key}",
        Need::Role(Role::Member),
        Some("changed a setting"),
    ),
    (
        "POST",
        "/api/v1/field/{entity}/{key}",
        Need::Role(Role::Admin),
        Some("added a field"),
    ),
    (
        "DELETE",
        "/api/v1/field/{entity}/{key}",
        Need::Role(Role::Admin),
        Some("removed a field"),
    ),
    (
        "POST",
        "/api/v1/spool/{id}/tag",
        Need::Role(Role::Member),
        Some("tagged spool"),
    ),
    (
        "DELETE",
        "/api/v1/spool/{id}/tag/{uid}",
        Need::Role(Role::Member),
        Some("untagged spool"),
    ),
    (
        "POST",
        "/api/v1/filament/{id}/tag",
        Need::Role(Role::Member),
        Some("tagged filament"),
    ),
    (
        "DELETE",
        "/api/v1/filament/{id}/tag/{uid}",
        Need::Role(Role::Member),
        Some("untagged filament"),
    ),
    ("POST", "/api/v1/tag/scan", Need::Role(Role::Member), None),
    (
        "POST",
        "/api/v1/backup",
        Need::Role(Role::Admin),
        Some("backed up the materials"),
    ),
];

/// What a request with this method on this route needs; `None`: no rule, refused.
pub fn need(method: &Method, route: &str) -> Option<(Need, Option<&'static str>)> {
    let found = RULES
        .iter()
        .find(|(m, path, ..)| *path == route && (*m == "*" || *m == method.as_str()));
    match found {
        Some((_, _, need, label)) => Some((*need, *label)),
        None if matches!(*method, Method::GET | Method::HEAD) => {
            Some((Need::Role(Role::Guest), None))
        }
        None => None,
    }
}

/// After the route is matched: what it needs, against who asks; then, for what changed
/// something, a line in the audit log.
pub async fn permit(State(store): State<Shared>, request: Request, next: Next) -> Response {
    let (mut parts, body) = request.into_parts();
    let params = RawPathParams::from_request_parts(&mut parts, &())
        .await
        .ok();
    let route = parts
        .extensions
        .get::<MatchedPath>()
        .map_or("", |m| m.as_str())
        .to_owned();
    let request = Request::from_parts(parts, body);
    let method = request.method().clone();
    let Some((need, label)) = need(&method, &route) else {
        return (StatusCode::FORBIDDEN, "nobody may do that here").into_response();
    };
    let param = |name: &str| {
        params.as_ref().and_then(|p| {
            p.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_owned())
        })
    };
    let access = request.extensions().get::<Access>().cloned();
    let signed = request
        .extensions()
        .get::<crate::accounts::Signed>()
        .is_some();
    let allowed = match need {
        Need::Public => Ok(()),
        Need::Signed if signed || access.is_some() => Ok(()),
        Need::Signed => Err((StatusCode::UNAUTHORIZED, "sign in first")),
        Need::InstanceAdmin => match &access {
            Some(a) if a.instance_admin && a.token_id.is_none() => Ok(()),
            _ if request
                .extensions()
                .get::<crate::accounts::Signed>()
                .is_some_and(|s| s.instance_admin) =>
            {
                Ok(())
            }
            _ => Err((StatusCode::FORBIDDEN, "only who runs the server may")),
        },
        Need::Role(role) => match &access {
            Some(a) if a.role >= role => Ok(()),
            Some(_) => Err((StatusCode::FORBIDDEN, role_refusal(role))),
            None => Err((StatusCode::FORBIDDEN, "not in a space")),
        },
        Need::Machine(level) => match (&access, param("id")) {
            (Some(a), Some(machine)) => match store.lock().unwrap().may(a, &machine, level) {
                Ok(Some(true)) => Ok(()),
                Ok(Some(false)) => Err((StatusCode::FORBIDDEN, level_refusal(level))),
                Ok(None) => Err((StatusCode::NOT_FOUND, "no such machine")),
                Err(error) => return crate::failed(error),
            },
            (None, _) => Err((StatusCode::FORBIDDEN, "not in a space")),
            (_, None) => Err((StatusCode::FORBIDDEN, "nobody may do that here")),
        },
    };
    if let Err((status, why)) = allowed {
        return (status, why).into_response();
    }
    let response = next.run(request).await;
    if let (Some(label), Some(access)) = (label, &access)
        && (response.status().is_success() || response.status().is_redirection())
    {
        let target = ["id", "user", "external_id", "name", "key"]
            .iter()
            .find_map(|k| param(k))
            .unwrap_or_default();
        let _ = store.lock().unwrap().audit(access, label, &target, "");
    }
    response
}

fn role_refusal(role: Role) -> &'static str {
    match role {
        Role::Guest => "not in this space",
        Role::Member => "guests look only: ask an admin to make you a member",
        Role::Admin => "only an admin or owner of the space may",
        Role::Owner => "only an owner of the space may",
    }
}

fn level_refusal(level: Level) -> &'static str {
    match level {
        Level::View => "not in this space",
        Level::Use => "you may not use this machine: it needs permission (Space → Members)",
        Level::Operate => {
            "moving, heating and G-code take 'operate' permission on this machine ('use' is jobs only): see Space → Members"
        }
        Level::Manage => "only an admin or owner of the space may change this machine",
    }
}

/// For a handler whose machine is in the form or the body (printing a file, framing it,
/// loading a spool): whether this access has `level` on it; the refusal otherwise.
pub fn check_machine(
    store: &Store,
    access: &Access,
    machine: &str,
    level: Level,
) -> Result<(), Box<Response>> {
    match store.may(access, machine, level) {
        Ok(Some(true)) => Ok(()),
        Ok(Some(false)) => Err(Box::new(
            (StatusCode::FORBIDDEN, level_refusal(level)).into_response(),
        )),
        Ok(None) => Err(Box::new(
            (StatusCode::NOT_FOUND, "no such machine").into_response(),
        )),
        Err(error) => Err(Box::new(crate::failed(error))),
    }
}

/// A space someone is in.
#[derive(Debug, Clone, Serialize)]
pub struct SpaceOf {
    pub id: i64,
    /// Its id under the hood (a UUID), the same wherever it goes.
    pub uid: String,
    pub name: String,
    /// The organisation it is part of, by name.
    pub organisation: String,
    pub role: Role,
}

/// A member of a space, for its page.
#[derive(Debug, Clone, Serialize)]
pub struct Member {
    pub user_id: i64,
    pub name: String,
    pub display_name: String,
    /// The role that counts in this space: the higher of the two below.
    pub role: Role,
    /// Their role in the organisation (in all its spaces), if they are in it.
    pub org_role: Option<Role>,
    /// Their role in this space only, if they have one.
    pub space_role: Option<Role>,
    pub since: i64,
}

/// An invite link (its token is shown once, when made).
#[derive(Debug, Clone, Serialize)]
pub struct Invite {
    pub id: i64,
    pub space: i64,
    pub space_name: String,
    pub role: Role,
    pub by: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub used_by: Option<String>,
    pub revoked: bool,
    /// Into the whole organisation (all its spaces), or the one space.
    pub whole_organisation: bool,
}

impl Invite {
    pub fn open(&self) -> bool {
        !self.revoked && self.used_by.is_none() && self.expires_at > now()
    }
}

/// An induction: a member's right on a machine that needs one.
#[derive(Debug, Clone, Serialize)]
pub struct Grant {
    pub user_id: i64,
    pub display_name: String,
    pub level: Level,
    pub by: Option<String>,
    pub granted_at: i64,
    pub expires_at: Option<i64>,
}

/// Someone asking the staff for permission (or a training) on a machine.
#[derive(Debug, Clone, Serialize)]
pub struct AskedFor {
    pub id: i64,
    pub user_id: i64,
    pub who: String,
    pub machine_id: String,
    pub machine: String,
    pub training: bool,
    pub asked_at: i64,
}

/// A group of machines, and who has permission on all of them.
#[derive(Debug, Clone, Serialize)]
pub struct Group {
    pub id: i64,
    pub name: String,
    /// (id, name) of its machines.
    pub machines: Vec<(String, String)>,
    pub grants: Vec<Grant>,
}

/// An API token of a space (the secret itself is shown once, when made).
#[derive(Debug, Clone, Serialize)]
pub struct Token {
    pub id: i64,
    pub name: String,
    pub role: Role,
    pub by: String,
    pub created_at: i64,
    pub last_used: Option<i64>,
}

/// A line of the audit log.
#[derive(Debug, Clone, Serialize)]
pub struct AuditLine {
    pub at: i64,
    pub actor: String,
    pub action: String,
    pub target: String,
    pub detail: String,
}

/// Invite links last a week, unless used or revoked first.
pub const INVITE_SECONDS: i64 = 7 * 24 * 3600;
/// Prefix of API tokens, so one is recognised when pasted somewhere it should not be.
pub const TOKEN_PREFIX: &str = "mct_";

impl Store {
    /// The space a request acts in, with its name: from who asks.
    pub fn space_of(&self, access: &Access) -> rusqlite::Result<(i64, String)> {
        Ok((access.space, self.space_name(access.space)?))
    }

    /// Someone's role in a space: the higher of their role in its organisation (which
    /// counts in all its spaces) and in the space itself.
    pub fn role_of(&self, user: i64, space: i64) -> rusqlite::Result<Option<Role>> {
        let roles: Vec<String> = {
            let mut q = self.db.prepare(
                "SELECT role FROM memberships WHERE user_id = ?1 AND space_id = ?2
                 UNION ALL
                 SELECT o.role FROM org_memberships o JOIN spaces s ON s.organisation_id = o.organisation_id
                 WHERE o.user_id = ?1 AND s.id = ?2",
            )?;
            q.query_map(params![user, space], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        Ok(roles.iter().filter_map(|r| Role::parse(r)).max())
    }

    /// Someone's role in an organisation itself (not through one of its spaces).
    pub fn org_role_of(&self, user: i64, organisation: i64) -> rusqlite::Result<Option<Role>> {
        Ok(self
            .db
            .query_row(
                "SELECT role FROM org_memberships WHERE user_id = ?1 AND organisation_id = ?2",
                params![user, organisation],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .and_then(|r| Role::parse(&r)))
    }

    pub fn organisation_of(&self, space: i64) -> rusqlite::Result<i64> {
        self.db.query_row(
            "SELECT organisation_id FROM spaces WHERE id = ?1",
            [space],
            |r| r.get(0),
        )
    }

    pub fn organisation_name(&self, organisation: i64) -> rusqlite::Result<String> {
        self.db.query_row(
            "SELECT name FROM organisations WHERE id = ?1",
            [organisation],
            |r| r.get(0),
        )
    }

    pub fn rename_organisation(&self, organisation: i64, name: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE organisations SET name = ?2 WHERE id = ?1",
            params![organisation, name],
        )?;
        Ok(())
    }

    /// Puts a user in an organisation (all its spaces) with a role, or changes it.
    pub fn set_org_membership(
        &self,
        user: i64,
        organisation: i64,
        role: &str,
    ) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO org_memberships (user_id, organisation_id, role, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (user_id, organisation_id) DO UPDATE SET role = excluded.role",
            params![user, organisation, role, now()],
        )?;
        Ok(())
    }

    /// The spaces a user can be in (through an organisation or of their own), by
    /// organisation and name, with their role in each.
    pub fn spaces_of(&self, user: i64) -> rusqlite::Result<Vec<SpaceOf>> {
        let rows: Vec<(i64, String, String, String, String)> = {
            let mut q = self.db.prepare(
                "SELECT s.id, s.uid, s.name, o.name, r.role FROM (
                     SELECT space_id AS id, role FROM memberships WHERE user_id = ?1
                     UNION ALL
                     SELECT s.id, m.role FROM org_memberships m JOIN spaces s ON s.organisation_id = m.organisation_id
                     WHERE m.user_id = ?1
                 ) r JOIN spaces s ON s.id = r.id JOIN organisations o ON o.id = s.organisation_id
                 ORDER BY o.name COLLATE NOCASE, s.name COLLATE NOCASE, s.id",
            )?;
            q.query_map([user], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<rusqlite::Result<_>>()?
        };
        let mut out: Vec<SpaceOf> = Vec::new();
        for (id, uid, name, organisation, role) in rows {
            let Some(role) = Role::parse(&role) else {
                continue;
            };
            match out.iter_mut().find(|s| s.id == id) {
                Some(seen) => seen.role = seen.role.max(role),
                None => out.push(SpaceOf {
                    id,
                    uid,
                    name,
                    organisation,
                    role,
                }),
            }
        }
        Ok(out)
    }

    pub fn space_name(&self, space: i64) -> rusqlite::Result<String> {
        self.db
            .query_row("SELECT name FROM spaces WHERE id = ?1", [space], |r| {
                r.get(0)
            })
    }

    /// A space by its id under the hood.
    pub fn space_by_uid(&self, uid: &str) -> rusqlite::Result<Option<i64>> {
        self.db
            .query_row("SELECT id FROM spaces WHERE uid = ?1", [uid], |r| r.get(0))
            .optional()
    }

    pub fn space_uid(&self, space: i64) -> rusqlite::Result<String> {
        self.db
            .query_row("SELECT uid FROM spaces WHERE id = ?1", [space], |r| {
                r.get(0)
            })
    }

    /// A new space in an organisation.
    pub fn create_space(&self, organisation: i64, name: &str) -> rusqlite::Result<i64> {
        self.db.execute(
            "INSERT INTO spaces (name, uid, organisation_id) VALUES (?1, ?2, ?3)",
            params![name, crate::accounts::new_uuid(), organisation],
        )?;
        Ok(self.db.last_insert_rowid())
    }

    /// A new organisation with its first space, its maker the owner of both.
    pub fn create_organisation(
        &self,
        name: &str,
        space_name: &str,
        owner: i64,
    ) -> rusqlite::Result<(i64, i64)> {
        self.db.execute(
            "INSERT INTO organisations (uid, name, created_at) VALUES (?1, ?2, ?3)",
            params![crate::accounts::new_uuid(), name, now()],
        )?;
        let organisation = self.db.last_insert_rowid();
        self.set_org_membership(owner, organisation, Role::Owner.as_str())?;
        let space = self.create_space(organisation, space_name)?;
        Ok((organisation, space))
    }

    /// The session goes on in another space.
    pub fn set_session_space(&self, session: i64, space: i64) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE sessions SET space_id = ?2 WHERE id = ?1",
            params![session, space],
        )?;
        Ok(())
    }

    /// Everyone who can be in a space: the organisation's members (in all its spaces), and
    /// those of this space only; each with the role that counts here.
    pub fn members(&self, space: i64) -> rusqlite::Result<Vec<Member>> {
        let rows: Vec<(i64, String, String, String, i64, bool)> = {
            let mut q = self.db.prepare(
                "SELECT u.id, u.name, u.display_name, m.role, m.created_at, 1 FROM org_memberships m
                 JOIN users u ON u.id = m.user_id JOIN spaces s ON s.organisation_id = m.organisation_id
                 WHERE s.id = ?1
                 UNION ALL
                 SELECT u.id, u.name, u.display_name, m.role, m.created_at, 0 FROM memberships m
                 JOIN users u ON u.id = m.user_id WHERE m.space_id = ?1",
            )?;
            q.query_map([space], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?
        };
        let mut out: Vec<Member> = Vec::new();
        for (user_id, name, display_name, role, since, whole) in rows {
            let Some(role) = Role::parse(&role) else {
                continue;
            };
            match out.iter_mut().find(|m| m.user_id == user_id) {
                Some(seen) => {
                    if whole {
                        seen.org_role = Some(role);
                    } else {
                        seen.space_role = Some(role);
                    }
                    seen.role = seen.role.max(role);
                }
                None => out.push(Member {
                    user_id,
                    name,
                    display_name,
                    role,
                    org_role: whole.then_some(role),
                    space_role: (!whole).then_some(role),
                    since,
                }),
            }
        }
        out.sort_by_key(|m| m.display_name.to_lowercase());
        Ok(out)
    }

    /// Owners of the organisation a space is in.
    pub fn owners(&self, space: i64) -> rusqlite::Result<i64> {
        self.db.query_row(
            "SELECT COUNT(*) FROM org_memberships m JOIN spaces s ON s.organisation_id = m.organisation_id
             WHERE s.id = ?1 AND m.role = 'owner'",
            [space],
            |r| r.get(0),
        )
    }

    /// Takes someone out of a space (`whole`: out of its organisation, so of all its
    /// spaces), with their inductions there; their sessions find another space.
    pub fn remove_member(&self, space: i64, user: i64, whole: bool) -> rusqlite::Result<bool> {
        let n = if whole {
            let organisation = self.organisation_of(space)?;
            let n = self.db.execute(
                "DELETE FROM org_memberships WHERE organisation_id = ?1 AND user_id = ?2",
                params![organisation, user],
            )?;
            self.db.execute(
                "DELETE FROM machine_grants WHERE user_id = ?2
                     AND space_id IN (SELECT id FROM spaces WHERE organisation_id = ?1)",
                params![organisation, user],
            )?;
            self.db.execute(
                "DELETE FROM group_grants WHERE user_id = ?2 AND group_id IN (SELECT g.id FROM machine_groups g
                     JOIN spaces s ON s.id = g.space_id WHERE s.organisation_id = ?1)",
                params![organisation, user],
            )?;
            n
        } else {
            let n = self.db.execute(
                "DELETE FROM memberships WHERE space_id = ?1 AND user_id = ?2",
                params![space, user],
            )?;
            self.db.execute(
                "DELETE FROM machine_grants WHERE space_id = ?1 AND user_id = ?2",
                params![space, user],
            )?;
            self.db.execute(
                "DELETE FROM group_grants WHERE user_id = ?2
                     AND group_id IN (SELECT id FROM machine_groups WHERE space_id = ?1)",
                params![space, user],
            )?;
            n
        };
        self.db.execute(
            "UPDATE sessions SET space_id = NULL WHERE user_id = ?1",
            [user],
        )?;
        Ok(n > 0)
    }

    /// Whether `access` may do `need` with a machine of its space; `None` when the machine is
    /// not in it. Managing it (settings, permissions, removing) is for owners and admins;
    /// using it (jobs, moving, heating) is by permission for everyone, whatever their role:
    /// being owner does not mean having had the laser course.
    pub fn may(
        &self,
        access: &Access,
        machine: &str,
        need: Level,
    ) -> rusqlite::Result<Option<bool>> {
        let Some(level) = self.machine_level(access, machine)? else {
            return Ok(None);
        };
        Ok(Some(match need {
            Level::Manage => access.role >= Role::Admin,
            _ => level >= need,
        }))
    }

    /// What `access` may do with a machine of its space as a user of it: view, use, operate
    /// (managing is [`Store::may`]'s, by role); `None` when the machine is not in it.
    pub fn machine_level(&self, access: &Access, machine: &str) -> rusqlite::Result<Option<Level>> {
        let found: Option<bool> = self
            .db
            .query_row(
                "SELECT induction_required FROM machines WHERE id = ?1 AND space_id = ?2",
                params![machine, access.space],
                |r| r.get(0),
            )
            .optional()?;
        let Some(induction) = found else {
            return Ok(None);
        };
        Ok(Some(match access.role {
            Role::Guest => Level::View,
            _ if !induction => Level::Operate,
            _ => self
                .grant_of(machine, access.user_id)?
                .unwrap_or(Level::View),
        }))
    }

    /// A member's permission on a machine, while it lasts: given on the machine, or on a
    /// group it is in; the highest counts.
    fn grant_of(&self, machine: &str, user: i64) -> rusqlite::Result<Option<Level>> {
        let mut q = self.db.prepare(
            "SELECT level FROM machine_grants WHERE machine_id = ?1 AND user_id = ?2
                 AND (expires_at IS NULL OR expires_at > ?3)
             UNION ALL
             SELECT g.level FROM group_grants g JOIN machine_group_members m ON m.group_id = g.group_id
             WHERE m.machine_id = ?1 AND g.user_id = ?2 AND (g.expires_at IS NULL OR g.expires_at > ?3)",
        )?;
        let levels = q.query_map(params![machine, user, now()], |r| r.get::<_, String>(0))?;
        Ok(levels.flatten().filter_map(|l| Level::parse(&l)).max())
    }

    /// The machine groups of a space, with their machines and who has permission on them.
    pub fn groups(&self, space: i64) -> rusqlite::Result<Vec<Group>> {
        let heads: Vec<(i64, String)> = {
            let mut q = self.db.prepare(
                "SELECT id, name FROM machine_groups WHERE space_id = ?1 ORDER BY name COLLATE NOCASE",
            )?;
            q.query_map([space], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut groups = Vec::new();
        for (id, name) in heads {
            let machines = {
                let mut q = self.db.prepare(
                    "SELECT m.id, m.name FROM machine_group_members gm JOIN machines m ON m.id = gm.machine_id
                     WHERE gm.group_id = ?1 AND m.space_id = ?2 ORDER BY m.name COLLATE NOCASE",
                )?;
                q.query_map(params![id, space], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            let grants = {
                let mut q = self.db.prepare(
                    "SELECT g.user_id, u.display_name, g.level, b.display_name, g.granted_at, g.expires_at
                     FROM group_grants g JOIN users u ON u.id = g.user_id
                     LEFT JOIN users b ON b.id = g.granted_by
                     WHERE g.group_id = ?1 ORDER BY u.display_name COLLATE NOCASE",
                )?;
                q.query_map([id], |r| {
                    Ok(Grant {
                        user_id: r.get(0)?,
                        display_name: r.get(1)?,
                        level: Level::parse(&r.get::<_, String>(2)?).unwrap_or(Level::View),
                        by: r.get(3)?,
                        granted_at: r.get(4)?,
                        expires_at: r.get(5)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?
            };
            groups.push(Group {
                id,
                name,
                machines,
                grants,
            });
        }
        Ok(groups)
    }

    /// A new group of machines; `kind` fills it with the space's machines of that kind (fdm,
    /// laser, label), or with all of them (`all`).
    pub fn create_group(
        &self,
        space: i64,
        name: &str,
        kind: Option<&str>,
    ) -> rusqlite::Result<i64> {
        self.db.execute(
            "INSERT INTO machine_groups (space_id, name, created_at) VALUES (?1, ?2, ?3)",
            params![space, name, now()],
        )?;
        let id = self.db.last_insert_rowid();
        if kind == Some("all") {
            self.db.execute(
                "INSERT OR IGNORE INTO machine_group_members (group_id, machine_id)
                 SELECT ?1, id FROM machines WHERE space_id = ?2",
                params![id, space],
            )?;
        } else if let Some(kind) = kind {
            // The kind is kept as JSON: "fdm", with its quotes.
            self.db.execute(
                "INSERT OR IGNORE INTO machine_group_members (group_id, machine_id)
                 SELECT ?1, id FROM machines WHERE space_id = ?2 AND kind = ?3",
                params![id, space, format!("\"{kind}\"")],
            )?;
        }
        Ok(id)
    }

    /// What the audit log says of a person and a machine: their names.
    pub fn names_for_log(&self, user: Option<i64>, machine: Option<&str>) -> (String, String) {
        let user = user
            .and_then(|u| {
                self.db
                    .query_row("SELECT display_name FROM users WHERE id = ?1", [u], |r| {
                        r.get(0)
                    })
                    .ok()
            })
            .unwrap_or_default();
        let machine = machine
            .and_then(|m| {
                self.db
                    .query_row("SELECT name FROM machines WHERE id = ?1", [m], |r| r.get(0))
                    .ok()
            })
            .unwrap_or_default();
        (user, machine)
    }

    /// A group's name, when it is of the space.
    pub fn group_name(&self, space: i64, group: i64) -> rusqlite::Result<Option<String>> {
        self.db
            .query_row(
                "SELECT name FROM machine_groups WHERE id = ?1 AND space_id = ?2",
                params![group, space],
                |r| r.get(0),
            )
            .optional()
    }

    pub fn remove_group(&self, space: i64, group: i64) -> rusqlite::Result<bool> {
        if self.group_name(space, group)?.is_none() {
            return Ok(false);
        }
        self.db
            .execute("DELETE FROM group_grants WHERE group_id = ?1", [group])?;
        self.db.execute(
            "DELETE FROM machine_group_members WHERE group_id = ?1",
            [group],
        )?;
        self.db
            .execute("DELETE FROM machine_groups WHERE id = ?1", [group])?;
        Ok(true)
    }

    /// Puts a machine of the space in a group of it, or takes it out.
    pub fn set_group_machine(
        &self,
        space: i64,
        group: i64,
        machine: &str,
        member: bool,
    ) -> rusqlite::Result<bool> {
        let machine_here: bool = self.db.query_row(
            "SELECT COUNT(*) FROM machines WHERE id = ?1 AND space_id = ?2",
            params![machine, space],
            |r| r.get(0),
        )?;
        if self.group_name(space, group)?.is_none() || !machine_here {
            return Ok(false);
        }
        if member {
            self.db.execute(
                "INSERT OR IGNORE INTO machine_group_members (group_id, machine_id) VALUES (?1, ?2)",
                params![group, machine],
            )?;
        } else {
            self.db.execute(
                "DELETE FROM machine_group_members WHERE group_id = ?1 AND machine_id = ?2",
                params![group, machine],
            )?;
        }
        Ok(true)
    }

    /// Gives a member permission on every machine of a group (or changes it).
    pub fn set_group_grant(
        &self,
        group: i64,
        user: i64,
        level: Level,
        by: i64,
        expires_at: Option<i64>,
    ) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO group_grants (group_id, user_id, level, granted_by, granted_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (group_id, user_id) DO UPDATE SET level = excluded.level,
                 granted_by = excluded.granted_by, granted_at = excluded.granted_at,
                 expires_at = excluded.expires_at",
            params![group, user, level.as_str(), by, now(), expires_at],
        )?;
        Ok(())
    }

    pub fn remove_group_grant(&self, group: i64, user: i64) -> rusqlite::Result<bool> {
        Ok(self.db.execute(
            "DELETE FROM group_grants WHERE group_id = ?1 AND user_id = ?2",
            params![group, user],
        )? > 0)
    }

    /// Whether a machine's permission is a training (it needs training, not only permission).
    pub fn needs_training(&self, space: i64, machine: &str) -> rusqlite::Result<bool> {
        Ok(self
            .db
            .query_row(
                "SELECT induction_required = 2 FROM machines WHERE id = ?1 AND space_id = ?2",
                params![machine, space],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    /// Open to every member (0), needs permission (1), or needs training (2).
    pub fn set_access_mode(
        &mut self,
        space: i64,
        machine: &str,
        mode: u8,
    ) -> rusqlite::Result<bool> {
        let n = self.db.execute(
            "UPDATE machines SET induction_required = ?3 WHERE id = ?1 AND space_id = ?2",
            params![machine, space, mode.min(2)],
        )?;
        self.changed();
        Ok(n > 0)
    }

    /// Someone asks the staff for permission on a machine: false when they asked already
    /// and nobody answered yet.
    pub fn ask_permission(&self, space: i64, user: i64, machine: &str) -> rusqlite::Result<bool> {
        if self.asked(user, machine)?.is_some() {
            return Ok(false);
        }
        self.db.execute(
            "INSERT INTO access_requests (space_id, user_id, machine_id, asked_at) VALUES (?1, ?2, ?3, ?4)",
            params![space, user, machine, now()],
        )?;
        Ok(true)
    }

    /// When someone asked for a machine, while nobody answered.
    pub fn asked(&self, user: i64, machine: &str) -> rusqlite::Result<Option<i64>> {
        self.db
            .query_row(
                "SELECT asked_at FROM access_requests WHERE user_id = ?1 AND machine_id = ?2 AND state = 'open'",
                params![user, machine],
                |r| r.get(0),
            )
            .optional()
    }

    /// A space's open requests, oldest first.
    pub fn requests(&self, space: i64) -> rusqlite::Result<Vec<AskedFor>> {
        let mut q = self.db.prepare(
            "SELECT r.id, r.user_id, u.display_name, r.machine_id, m.name, m.induction_required = 2, r.asked_at
             FROM access_requests r JOIN users u ON u.id = r.user_id JOIN machines m ON m.id = r.machine_id
             WHERE r.space_id = ?1 AND m.space_id = ?1 AND r.state = 'open' ORDER BY r.asked_at",
        )?;
        q.query_map([space], |r| {
            Ok(AskedFor {
                id: r.get(0)?,
                user_id: r.get(1)?,
                who: r.get(2)?,
                machine_id: r.get(3)?,
                machine: r.get(4)?,
                training: r.get(5)?,
                asked_at: r.get(6)?,
            })
        })?
        .collect()
    }

    /// Answers a request: given (`use` on the machine) or declined. The request, if it was
    /// open and of the space.
    pub fn answer_request(
        &self,
        space: i64,
        id: i64,
        give: bool,
        by: i64,
    ) -> rusqlite::Result<Option<AskedFor>> {
        let Some(request) = self.requests(space)?.into_iter().find(|r| r.id == id) else {
            return Ok(None);
        };
        if give {
            self.set_grant(
                space,
                &request.machine_id,
                request.user_id,
                Level::Use,
                by,
                None,
            )?;
        }
        self.db.execute(
            "UPDATE access_requests SET state = ?2, handled_by = ?3, handled_at = ?4 WHERE id = ?1",
            params![id, if give { "given" } else { "declined" }, by, now()],
        )?;
        Ok(Some(request))
    }

    pub fn induction_required(&self, space: i64, machine: &str) -> rusqlite::Result<bool> {
        Ok(self
            .db
            .query_row(
                "SELECT induction_required FROM machines WHERE id = ?1 AND space_id = ?2",
                params![machine, space],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    pub fn set_induction_required(
        &mut self,
        space: i64,
        machine: &str,
        on: bool,
    ) -> rusqlite::Result<bool> {
        let n = self.db.execute(
            "UPDATE machines SET induction_required = ?3 WHERE id = ?1 AND space_id = ?2",
            params![machine, space, on],
        )?;
        self.changed();
        Ok(n > 0)
    }

    pub fn grants(&self, space: i64, machine: &str) -> rusqlite::Result<Vec<Grant>> {
        let mut q = self.db.prepare(
            "SELECT g.user_id, u.display_name, g.level, b.display_name, g.granted_at, g.expires_at
             FROM machine_grants g JOIN users u ON u.id = g.user_id
             LEFT JOIN users b ON b.id = g.granted_by
             WHERE g.space_id = ?1 AND g.machine_id = ?2 ORDER BY u.display_name COLLATE NOCASE",
        )?;
        let rows = q.query_map(params![space, machine], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<i64>>(5)?,
            ))
        })?;
        Ok(rows
            .flatten()
            .filter_map(
                |(user_id, display_name, level, by, granted_at, expires_at)| {
                    Some(Grant {
                        user_id,
                        display_name,
                        level: Level::parse(&level)?,
                        by,
                        granted_at,
                        expires_at,
                    })
                },
            )
            .collect())
    }

    /// Gives a member of the space an induction on a machine of it (or changes it).
    pub fn set_grant(
        &self,
        space: i64,
        machine: &str,
        user: i64,
        level: Level,
        by: i64,
        expires_at: Option<i64>,
    ) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO machine_grants (space_id, machine_id, user_id, level, granted_by, granted_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (machine_id, user_id) DO UPDATE SET level = excluded.level,
                 granted_by = excluded.granted_by, granted_at = excluded.granted_at,
                 expires_at = excluded.expires_at",
            params![space, machine, user, level.as_str(), by, now(), expires_at],
        )?;
        Ok(())
    }

    pub fn remove_grant(&self, space: i64, machine: &str, user: i64) -> rusqlite::Result<bool> {
        Ok(self.db.execute(
            "DELETE FROM machine_grants WHERE space_id = ?1 AND machine_id = ?2 AND user_id = ?3",
            params![space, machine, user],
        )? > 0)
    }

    /// A new invite link into a space; the token for its address (only its hash is kept).
    pub fn create_invite(
        &self,
        space: i64,
        role: Role,
        by: i64,
        whole_organisation: bool,
    ) -> rusqlite::Result<String> {
        let token = random_token();
        self.db.execute(
            "INSERT INTO invites (token_hash, space_id, role, created_by, created_at, expires_at, whole_organisation)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![token_hash(&token), space, role.as_str(), by, now(), now() + INVITE_SECONDS, whole_organisation],
        )?;
        Ok(token)
    }

    fn invites_where(
        &self,
        filter: &str,
        value: &dyn rusqlite::ToSql,
    ) -> rusqlite::Result<Vec<Invite>> {
        let mut q = self.db.prepare(&format!(
            "SELECT i.id, i.space_id, s.name, i.role, b.display_name, i.created_at, i.expires_at,
                 u.display_name, i.revoked, i.whole_organisation
             FROM invites i JOIN spaces s ON s.id = i.space_id JOIN users b ON b.id = i.created_by
             LEFT JOIN users u ON u.id = i.used_by WHERE {filter} ORDER BY i.created_at DESC"
        ))?;
        let rows = q.query_map([value], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, bool>(8)?,
                r.get::<_, bool>(9)?,
            ))
        })?;
        Ok(rows
            .flatten()
            .filter_map(
                |(
                    id,
                    space,
                    space_name,
                    role,
                    by,
                    created_at,
                    expires_at,
                    used_by,
                    revoked,
                    whole_organisation,
                )| {
                    Some(Invite {
                        id,
                        space,
                        space_name,
                        role: Role::parse(&role)?,
                        by,
                        created_at,
                        expires_at,
                        used_by,
                        revoked,
                        whole_organisation,
                    })
                },
            )
            .collect())
    }

    /// A space's invite links, newest first.
    pub fn invites(&self, space: i64) -> rusqlite::Result<Vec<Invite>> {
        self.invites_where("i.space_id = ?1", &space)
    }

    /// The invite an address's token is for, open or not.
    pub fn invite(&self, token: &str) -> rusqlite::Result<Option<Invite>> {
        Ok(self
            .invites_where("i.token_hash = ?1", &token_hash(token))?
            .into_iter()
            .next())
    }

    pub fn revoke_invite(&self, space: i64, id: i64) -> rusqlite::Result<bool> {
        Ok(self.db.execute(
            "UPDATE invites SET revoked = 1 WHERE id = ?1 AND space_id = ?2",
            params![id, space],
        )? > 0)
    }

    /// Uses an open invite: the user joins its space with its role (or keeps a higher one).
    /// False when it was taken meanwhile.
    pub fn use_invite(&self, invite: &Invite, user: i64) -> rusqlite::Result<bool> {
        let taken = self.db.execute(
            "UPDATE invites SET used_by = ?2, used_at = ?3
             WHERE id = ?1 AND used_by IS NULL AND revoked = 0 AND expires_at > ?3",
            params![invite.id, user, now()],
        )?;
        if taken == 0 {
            return Ok(false);
        }
        if invite.whole_organisation {
            let organisation = self.organisation_of(invite.space)?;
            if self
                .org_role_of(user, organisation)?
                .is_none_or(|r| r < invite.role)
            {
                self.set_org_membership(user, organisation, invite.role.as_str())?;
            }
        } else if self
            .role_of(user, invite.space)?
            .is_none_or(|r| r < invite.role)
        {
            self.set_membership(user, invite.space, invite.role.as_str())?;
        }
        Ok(true)
    }

    /// A new API token of a space; the secret, to show once (only its hash is kept).
    pub fn create_token(
        &self,
        space: i64,
        by: i64,
        name: &str,
        role: Role,
    ) -> rusqlite::Result<String> {
        let token = format!("{TOKEN_PREFIX}{}", random_token());
        self.db.execute(
            "INSERT INTO api_tokens (token_hash, space_id, created_by, name, role, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![token_hash(&token), space, by, name, role.as_str(), now()],
        )?;
        Ok(token)
    }

    pub fn tokens(&self, space: i64) -> rusqlite::Result<Vec<Token>> {
        let mut q = self.db.prepare(
            "SELECT t.id, t.name, t.role, u.display_name, t.created_at, t.last_used
             FROM api_tokens t JOIN users u ON u.id = t.created_by
             WHERE t.space_id = ?1 AND t.revoked = 0 ORDER BY t.created_at DESC",
        )?;
        let rows = q.query_map([space], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<i64>>(5)?,
            ))
        })?;
        Ok(rows
            .flatten()
            .filter_map(|(id, name, role, by, created_at, last_used)| {
                Some(Token {
                    id,
                    name,
                    role: Role::parse(&role)?,
                    by,
                    created_at,
                    last_used,
                })
            })
            .collect())
    }

    pub fn revoke_token(&self, space: i64, id: i64) -> rusqlite::Result<bool> {
        Ok(self.db.execute(
            "UPDATE api_tokens SET revoked = 1 WHERE id = ?1 AND space_id = ?2",
            params![id, space],
        )? > 0)
    }

    /// Who an API token acts for: its space, its role (never more than its maker has there
    /// now; a maker who left the space takes the token's rights with them).
    pub fn token_access(&self, token: &str) -> rusqlite::Result<Option<Access>> {
        let found = self
            .db
            .query_row(
                "SELECT t.id, t.space_id, t.role, t.name, t.created_by, t.last_used
                 FROM api_tokens t
                 JOIN users u ON u.id = t.created_by AND u.disabled = 0
                 WHERE t.token_hash = ?1 AND t.revoked = 0",
                [token_hash(token)],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, Option<i64>>(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, space, role, name, user, last_used)) = found else {
            return Ok(None);
        };
        // The maker's role there now, through the organisation or the space.
        let (Some(role), Some(maker_role)) = (Role::parse(&role), self.role_of(user, space)?)
        else {
            return Ok(None);
        };
        if last_used.is_none_or(|t| now() - t >= 60) {
            self.db.execute(
                "UPDATE api_tokens SET last_used = ?2 WHERE id = ?1",
                params![id, now()],
            )?;
        }
        Ok(Some(Access {
            user_id: user,
            actor: format!("token \"{name}\""),
            space,
            role: role.min(maker_role),
            token_id: Some(id),
            session_id: None,
            instance_admin: false,
        }))
    }

    /// A line in a space's audit log.
    pub fn audit(
        &self,
        access: &Access,
        action: &str,
        target: &str,
        detail: &str,
    ) -> rusqlite::Result<()> {
        let detail: String = detail.chars().take(500).collect();
        self.db.execute(
            "INSERT INTO audit_log (space_id, at, user_id, token_id, actor, action, target, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                access.space,
                now(),
                access.user_id,
                access.token_id,
                access.actor,
                action,
                target,
                detail
            ],
        )?;
        Ok(())
    }

    /// A space's audit log, newest first; a machine's id as its name where it still has one.
    pub fn audit_log(&self, space: i64, limit: usize) -> rusqlite::Result<Vec<AuditLine>> {
        let mut q = self.db.prepare(
            "SELECT a.at, a.actor, a.action,
                 COALESCE(m.name, a.target), a.detail
             FROM audit_log a LEFT JOIN machines m ON m.id = a.target AND m.space_id = a.space_id
             WHERE a.space_id = ?1 ORDER BY a.at DESC, a.id DESC LIMIT ?2",
        )?;
        let rows = q.query_map(params![space, limit as i64], |r| {
            Ok(AuditLine {
                at: r.get(0)?,
                actor: r.get(1)?,
                action: r.get(2)?,
                target: r.get(3)?,
                detail: r.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// Whether anyone signed in may make a space (a hosted service), or only who runs the
    /// server (the default).
    pub fn open_spaces(&self) -> bool {
        self.db
            .query_row(
                "SELECT value FROM meta WHERE key = 'open_spaces'",
                [],
                |r| r.get::<_, String>(0),
            )
            .is_ok_and(|v| v == "1")
    }

    pub fn set_open_spaces(&self, on: bool) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO meta (key, value) VALUES ('open_spaces', ?1)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [if on { "1" } else { "0" }],
        )?;
        Ok(())
    }
}

/// For the tests that read every route: all of the cloud's source files.
#[cfg(test)]
pub(crate) fn route_sources() -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "rs"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect();
    out.sort();
    assert!(out.len() > 20, "the cloud's sources");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use http_body_util::BodyExt;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    /// Every route that changes something has a rule; none falls to "refused" by mistake
    /// (or worse, through). Read from the source, so a new route is checked too.
    #[test]
    fn every_change_has_a_rule() {
        // Every source file, read when the test runs: a new module cannot be forgotten.
        let sources = crate::access::route_sources();
        let mut missing = Vec::new();
        let mut checked = 0;
        for source in &sources {
            for piece in source.split(".route(").skip(1) {
                let piece = piece.trim_start();
                let path = match piece.strip_prefix('"').and_then(|r| r.split('"').next()) {
                    Some(p) if p.starts_with('/') => p,
                    _ if piece.starts_with("protocol::") => continue,
                    _ => continue,
                };
                let call = piece.split(".route(").next().unwrap_or("");
                let call = &call[..call.len().min(400)];
                for (word, method) in [
                    ("post(", Method::POST),
                    ("put(", Method::PUT),
                    ("patch(", Method::PATCH),
                    ("delete(", Method::DELETE),
                ] {
                    if call.contains(word) {
                        checked += 1;
                        if need(&method, path).is_none() {
                            missing.push(format!("{method} {path}"));
                        }
                    }
                }
            }
        }
        assert!(checked > 60, "{checked}");
        assert!(missing.is_empty(), "routes without a rule: {missing:?}");
    }

    struct World {
        store: Shared,
        app: Router,
    }

    impl World {
        fn new() -> Self {
            let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
            let app = crate::app(store.clone());
            World { store, app }
        }

        /// A user in an organisation (or in one space only) with a role; their cookie.
        fn person(
            &self,
            name: &str,
            org: Option<(i64, Role)>,
            space: Option<(i64, Role)>,
        ) -> (i64, String) {
            let store = self.store.lock().unwrap();
            let user = store.create_user(name, name, "x", false).unwrap();
            if let Some((org, role)) = org {
                store.set_org_membership(user, org, role.as_str()).unwrap();
            }
            if let Some((space, role)) = space {
                store.set_membership(user, space, role.as_str()).unwrap();
            }
            let token = store.create_session(user, None, "", "").unwrap();
            (user, format!("{}={token}", crate::auth::COOKIE))
        }

        async fn send(&self, request: HttpRequest<Body>) -> (StatusCode, String) {
            let response = self.app.clone().oneshot(request).await.unwrap();
            let status = response.status();
            let body = response.into_body().collect().await.unwrap().to_bytes();
            (status, String::from_utf8_lossy(&body).into_owned())
        }

        async fn get(&self, cookie: &str, path: &str) -> (StatusCode, String) {
            self.send(
                HttpRequest::get(path)
                    .header("cookie", cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
        }

        async fn post(&self, cookie: &str, path: &str, form: &str) -> (StatusCode, String) {
            self.send(
                HttpRequest::post(path)
                    .header("cookie", cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("x-requested-with", "fetch")
                    .body(Body::from(form.to_owned()))
                    .unwrap(),
            )
            .await
        }
    }

    /// Permission per group: a new machine is closed; a group of 3D printers opens them all
    /// at once for a member, also one put in later; per machine still counts, the most wins.
    #[tokio::test]
    async fn groups_give_permission_on_many_machines() {
        let w = World::new();
        let (org, space) = {
            let s = w.store.lock().unwrap();
            let owner = s.create_user("olga", "Olga", "x", false).unwrap();
            s.create_organisation("Fablab", "Werkplaats", owner)
                .unwrap()
        };
        let (_, adam) = w.person("adam", Some((org, Role::Admin)), None);
        let (mia_id, mia) = w.person("mia", Some((org, Role::Member)), None);
        let (p1, p2, laser) = {
            let mut s = w.store.lock().unwrap();
            let mk3s = crate::profiles::find("prusa/mk3s").unwrap();
            (
                s.add_virtual(space, mk3s).unwrap(),
                s.add_virtual(space, mk3s).unwrap(),
                s.add_virtual(space, crate::profiles::find("ortur/lm2-pro-s2").unwrap())
                    .unwrap(),
            )
        };
        let as_mia = Access {
            user_id: mia_id,
            actor: "mia".into(),
            space,
            role: Role::Member,
            token_id: None,
            instance_admin: false,
            session_id: None,
        };
        let level = |m: &str| {
            w.store
                .lock()
                .unwrap()
                .machine_level(&as_mia, m)
                .unwrap()
                .unwrap()
        };
        assert_eq!(level(&p1), Level::View, "a new machine needs permission");
        // Members make no groups.
        assert_eq!(
            w.post(&mia, "/permissions/groups", "name=Mine&kind=fdm")
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        let (status, body) = w
            .post(&adam, "/permissions/groups", "name=3D+printers&kind=fdm")
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        let group = w.store.lock().unwrap().groups(space).unwrap()[0].clone();
        assert_eq!(group.machines.len(), 2, "both printers, not the laser");
        // With all machines: the printers and the laser.
        let all = {
            let s = w.store.lock().unwrap();
            let id = s.create_group(space, "Everything", Some("all")).unwrap();
            let n = s
                .groups(space)
                .unwrap()
                .into_iter()
                .find(|g| g.id == id)
                .unwrap()
                .machines
                .len();
            s.remove_group(space, id).unwrap();
            n
        };
        assert_eq!(all, 3);
        let grants = format!("/permissions/groups/{}/grants", group.id);
        assert_eq!(
            w.post(&adam, &grants, &format!("user={mia_id}&level=use"))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            (level(&p1), level(&p2), level(&laser)),
            (Level::Use, Level::Use, Level::View)
        );
        // Per machine as well: the most counts.
        w.post(
            &adam,
            &format!("/machines/{p1}/grants"),
            &format!("user={mia_id}&level=operate"),
        )
        .await;
        assert_eq!((level(&p1), level(&p2)), (Level::Operate, Level::Use));
        // A printer put in later is opened too; one taken out is closed again.
        let p3 = {
            let mut s = w.store.lock().unwrap();
            s.add_virtual(space, crate::profiles::find("prusa/mk3s").unwrap())
                .unwrap()
        };
        assert_eq!(level(&p3), Level::View);
        w.post(
            &adam,
            &format!("/permissions/groups/{}/machines", group.id),
            &format!("machine={p3}"),
        )
        .await;
        assert_eq!(level(&p3), Level::Use);
        w.post(
            &adam,
            &format!("/permissions/groups/{}/machines/{p2}/remove", group.id),
            "",
        )
        .await;
        assert_eq!(level(&p2), Level::View);
        // Ended: only what the machine itself gives is left.
        w.post(
            &adam,
            &grants,
            &format!("user={mia_id}&level=use&until=2020-01-01"),
        )
        .await;
        assert_eq!((level(&p1), level(&p3)), (Level::Operate, Level::View));
        // Removing the group takes its permissions; those per machine stay.
        w.post(&adam, &grants, &format!("user={mia_id}&level=use"))
            .await;
        assert_eq!(
            w.post(
                &adam,
                &format!("/permissions/groups/{}/remove", group.id),
                ""
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!((level(&p1), level(&p3)), (Level::Operate, Level::View));
        // From the Members list: on a group, or on a machine; only to members.
        let g2 = w
            .store
            .lock()
            .unwrap()
            .create_group(space, "Printers", Some("fdm"))
            .unwrap();
        let give = format!("/permissions/people/{mia_id}/permission");
        assert_eq!(
            w.post(&adam, &give, &format!("on=g:{g2}&level=use"))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(level(&p3), Level::Use);
        assert_eq!(
            w.post(&adam, &give, &format!("on=m:{laser}&level=operate"))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(level(&laser), Level::Operate);
        assert_eq!(
            w.post(&mia, &give, &format!("on=m:{laser}&level=operate"))
                .await
                .0,
            StatusCode::FORBIDDEN,
            "members give no permission"
        );
        let adam_id: i64 = w
            .store
            .lock()
            .unwrap()
            .db
            .query_row("SELECT id FROM users WHERE name = 'adam'", [], |r| r.get(0))
            .unwrap();
        // Being an admin does not mean having had the course: Adam manages the laser (its
        // name, its permissions) but may not use it until he gives himself permission.
        let as_adam = Access {
            user_id: adam_id,
            actor: "adam".into(),
            space,
            role: Role::Admin,
            token_id: None,
            instance_admin: false,
            session_id: None,
        };
        let adam_may = |need: Level| {
            w.store
                .lock()
                .unwrap()
                .may(&as_adam, &laser, need)
                .unwrap()
                .unwrap()
        };
        assert!(adam_may(Level::Manage) && !adam_may(Level::Use));
        assert_eq!(
            w.post(&adam, &format!("/machines/{laser}/name"), "name=Laser")
                .await
                .0,
            StatusCode::NO_CONTENT,
            "managing: yes"
        );
        assert_eq!(
            w.post(&adam, &format!("/machines/{laser}/command"), "gcode=G0")
                .await
                .0,
            StatusCode::FORBIDDEN,
            "using: not without permission"
        );
        // He sets his own permissions in Space, like anyone's.
        assert_eq!(
            w.post(
                &adam,
                &format!("/permissions/people/{adam_id}/permission"),
                &format!("on=m:{laser}&level=operate")
            )
            .await
            .0,
            StatusCode::NO_CONTENT,
            "in Space, his own too"
        );
        assert!(adam_may(Level::Operate));
        // Open to every member, and closed again: the two choices.
        w.post(&adam, &format!("/machines/{p2}/access"), "induction=0")
            .await;
        assert_eq!(level(&p2), Level::Operate, "open: every member operates it");
        w.post(&adam, &format!("/machines/{p2}/access"), "induction=1")
            .await;
        assert_eq!(
            level(&p2),
            Level::Use,
            "closed again: through the group Printers"
        );
        // In the Space page's Members tab: an admin sees everyone's permissions, with a
        // Permissions… window per person; a member sees only their own, without it.
        let (status, page) = w.get(&adam, "/space").await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            page.contains(&format!(
                r#"data-mx-user="{mia_id}" data-mx-group="{g2}" checked>"#
            )),
            "Mia's tick in the matrix"
        );
        assert!(
            page.contains(&format!(r#"data-open-perm="{adam_id}""#)),
            "his own row too: set in Space"
        );
        assert!(
            page.contains(&format!(r#"data-open-perm="{mia_id}""#)),
            "her window"
        );
        assert!(page.contains(r#"data-panel="permissions""#));
        let (status, page) = w.get(&mia, "/space").await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            page.contains(&format!(
                r#"data-mx-user="{mia_id}" data-mx-group="{g2}" checked disabled>"#
            )),
            "her own row, not to change"
        );
        // A tick in the matrix: off takes it away, on gives "operate" (everything); the
        // level under it switches to "use" (jobs only). Not for a member.
        let tick = format!("/permissions/groups/{g2}/people/{mia_id}");
        assert_eq!(w.post(&mia, &tick, "on=0").await.0, StatusCode::FORBIDDEN);
        assert_eq!(w.post(&adam, &tick, "on=0").await.0, StatusCode::OK);
        assert_eq!(level(&p3), Level::View);
        let (status, now) = w.post(&adam, &tick, "on=1").await;
        assert_eq!((status, now.as_str()), (StatusCode::OK, "operate"));
        assert_eq!(level(&p3), Level::Operate);
        let (status, now) = w.post(&adam, &tick, "on=1&level=use").await;
        assert_eq!((status, now.as_str()), (StatusCode::OK, "use"));
        assert_eq!(level(&p3), Level::Use);
        assert!(
            !page.contains(r#"data-open-perm=""#),
            "a member changes nothing"
        );
        // The window saves all at once: ticked is given (its level, its end), unticked taken.
        let save = format!("/permissions/people/{mia_id}");
        let form = format!(
            "on=g:{g2}&level:g:{g2}=operate&until:g:{g2}=&on=m:{p1}&level:m:{p1}=use&until:m:{p1}=2099-01-01"
        );
        let (status, body) = w.post(&adam, &save, &form).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        assert_eq!(
            (level(&p3), level(&laser)),
            (Level::Operate, Level::View),
            "the laser's own permission: unticked, gone"
        );
        assert_eq!(level(&p1), Level::Operate, "the most of group and machine");
        assert_eq!(w.post(&mia, &save, "").await.0, StatusCode::FORBIDDEN);
        assert_eq!(w.post(&adam, &save, "").await.0, StatusCode::NO_CONTENT);
        assert_eq!(
            (level(&p1), level(&p3)),
            (Level::View, Level::View),
            "nothing ticked: nothing left"
        );
        // Needs training: Mia asks the staff (once, however often she clicks); the staff
        // give it, and she may use it.
        let p4 = w
            .store
            .lock()
            .unwrap()
            .add_virtual(space, crate::profiles::find("prusa/mk3s").unwrap())
            .unwrap();
        assert_eq!(
            w.post(&adam, &format!("/machines/{p4}/access"), "induction=2")
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        assert!(w.store.lock().unwrap().needs_training(space, &p4).unwrap());
        assert_eq!(level(&p4), Level::View);
        let ask = format!("/machines/{p4}/ask");
        assert_eq!(w.post(&mia, &ask, "").await.0, StatusCode::NO_CONTENT);
        assert_eq!(w.post(&mia, &ask, "").await.0, StatusCode::NO_CONTENT);
        let asked = w.store.lock().unwrap().requests(space).unwrap();
        assert_eq!(asked.len(), 1, "asked once");
        assert!(asked[0].training);
        let (_, page) = w.get(&adam, "/space").await;
        assert!(page.contains("asks for a training"), "the staff see it");
        let give = format!("/permissions/requests/{}/give", asked[0].id);
        assert_eq!(
            w.post(&mia, &give, "").await.0,
            StatusCode::FORBIDDEN,
            "not hers to give"
        );
        assert_eq!(w.post(&adam, &give, "").await.0, StatusCode::NO_CONTENT);
        assert_eq!(level(&p4), Level::Use);
        assert_eq!(
            w.post(&adam, &give, "").await.0,
            StatusCode::NOT_FOUND,
            "answered"
        );
        assert_eq!(
            w.post(&mia, &ask, "").await.0,
            StatusCode::CONFLICT,
            "she may use it now"
        );
        // Someone else's space's group: not found.
        let other = {
            let s = w.store.lock().unwrap();
            let owner = s.create_user("zed", "Zed", "x", false).unwrap();
            let (_, b) = s.create_organisation("Zuid", "Hal", owner).unwrap();
            s.create_group(b, "Theirs", None).unwrap()
        };
        assert_eq!(
            w.post(
                &adam,
                &format!("/permissions/groups/{other}/grants"),
                &format!("user={mia_id}&level=use")
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }

    /// A queue: everyone looks; members move and take out their own prints only, and only
    /// among themselves (the others keep their place); staff change anything.
    #[tokio::test]
    async fn a_queue_is_changed_by_whom_it_is_of() {
        let w = World::new();
        let (org, space) = {
            let s = w.store.lock().unwrap();
            let owner = s.create_user("olga", "Olga", "x", false).unwrap();
            s.create_organisation("Fablab", "Werkplaats", owner)
                .unwrap()
        };
        let (_, adam) = w.person("adam", Some((org, Role::Admin)), None);
        let (mia_id, mia) = w.person("mia", Some((org, Role::Member)), None);
        let (zoe_id, _) = w.person("zoe", Some((org, Role::Member)), None);
        let (m, a, b, c) = {
            let mut s = w.store.lock().unwrap();
            let m = s
                .add_virtual(space, crate::profiles::find("prusa/mk3s").unwrap())
                .unwrap();
            s.db.execute(
                "INSERT INTO files (id, space_id, name, size, uploaded_at, meta) VALUES ('f1', ?1, 'a.gcode', 10, 0, '{}')",
                [space],
            )
            .unwrap();
            let file = s.file(space, "f1").unwrap().unwrap();
            s.queue_add(space, &m, &file, Some((mia_id, "mia")))
                .unwrap();
            s.queue_add(space, &m, &file, Some((zoe_id, "zoe")))
                .unwrap();
            s.queue_add(space, &m, &file, Some((mia_id, "mia")))
                .unwrap();
            let q: Vec<i64> = s.queue_of(&m).unwrap().iter().map(|q| q.id).collect();
            (m, q[0], q[1], q[2])
        };
        let order = format!("/machines/{m}/queue/order");
        let ids = |v: [i64; 3]| format!("ids={},{},{}", v[0], v[1], v[2]);
        let now = || -> Vec<i64> {
            w.store
                .lock()
                .unwrap()
                .queue_of(&m)
                .unwrap()
                .iter()
                .map(|q| q.id)
                .collect()
        };
        // Mia swaps her own two: Zoe's stays second.
        assert_eq!(
            w.post(&mia, &order, &ids([c, b, a])).await.0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(now(), [c, b, a]);
        // Moving Zoe's, or getting ahead of it: no.
        assert_eq!(
            w.post(&mia, &order, &ids([b, c, a])).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            w.post(&mia, &order, &ids([c, a, b])).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(now(), [c, b, a], "unchanged");
        // Taking out: her own, not Zoe's.
        assert_eq!(
            w.post(&mia, &format!("/machines/{m}/queue/{b}/remove"), "")
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        // Staff: anything.
        assert_eq!(
            w.post(&adam, &order, &ids([b, a, c])).await.0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(now(), [b, a, c]);
        assert_eq!(
            w.post(&adam, &format!("/machines/{m}/queue/{b}/remove"), "")
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            w.post(&mia, &format!("/machines/{m}/queue/{a}/remove"), "")
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(now(), [c]);
    }

    /// A machine moves to another space of the organisation, by someone who is admin in both;
    /// not while it has work, not into a space they do not manage. Permissions on it stay
    /// behind, and so does its place in the queue.
    #[tokio::test]
    async fn a_machine_moves_to_another_space() {
        let w = World::new();
        let (org, a1, a2, b1) = {
            let s = w.store.lock().unwrap();
            let owner = s.create_user("olga", "Olga", "x", false).unwrap();
            let (org, a1) = s
                .create_organisation("Fablab", "Werkplaats", owner)
                .unwrap();
            let a2 = s.create_space(org, "Lab").unwrap();
            let (_, b1) = s.create_organisation("Makers Zuid", "Hal", owner).unwrap();
            (org, a1, a2, b1)
        };
        // Both signed in to the first space.
        let in_first = |name: &str, role: Role| {
            let s = w.store.lock().unwrap();
            let user = s.create_user(name, name, "x", false).unwrap();
            s.set_org_membership(user, org, role.as_str()).unwrap();
            let token = s.create_session(user, Some(a1), "", "").unwrap();
            (user, format!("{}={token}", crate::auth::COOKIE))
        };
        let (_, adam) = in_first("adam", Role::Admin);
        let (mia_id, mia) = in_first("mia", Role::Member);
        let m = {
            let mut s = w.store.lock().unwrap();
            let m = s
                .add_virtual(a1, crate::profiles::find("prusa/mk3s").unwrap())
                .unwrap();
            s.set_takes_work(a1, &m, true).unwrap();
            m
        };
        w.post(&adam, &format!("/machines/{m}/access"), "induction=1")
            .await;
        w.post(
            &adam,
            &format!("/machines/{m}/grants"),
            &format!("user={mia_id}&level=use"),
        )
        .await;
        let (uid_a2, uid_b1) = {
            let s = w.store.lock().unwrap();
            (s.space_uid(a2).unwrap(), s.space_uid(b1).unwrap())
        };
        let to = |uid: &str| format!("space={uid}");
        let path = format!("/machines/{m}/move");
        assert_eq!(
            w.post(&mia, &path, &to(&uid_a2)).await.0,
            StatusCode::FORBIDDEN,
            "a member does not manage it"
        );
        assert_eq!(
            w.post(&adam, &path, &to(&uid_b1)).await.0,
            StatusCode::FORBIDDEN,
            "not admin there"
        );
        // Work under way: not now.
        w.store
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO jobs (space_id, machine_id, file_id, file_name, state, created_at) VALUES (?1, ?2, 'f', 'a.gcode', 'printing', 0)",
                params![a1, m],
            )
            .unwrap();
        let (status, why) = w.post(&adam, &path, &to(&uid_a2)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{why}");
        w.store
            .lock()
            .unwrap()
            .db
            .execute("UPDATE jobs SET state = 'stopped', taken_at = 1", [])
            .unwrap();
        let (status, body) = w.post(&adam, &path, &to(&uid_a2)).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        let s = w.store.lock().unwrap();
        assert!(
            s.machine(a1, &m).unwrap().is_none(),
            "gone from the first space"
        );
        assert!(s.machine(a2, &m).unwrap().is_some(), "in the second");
        assert!(
            !s.takes_work(&m).unwrap(),
            "no work from the new space's queue until someone says"
        );
        let grants: i64 =
            s.db.query_row(
                "SELECT COUNT(*) FROM machine_grants WHERE machine_id = ?1",
                [&m],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(grants, 0, "permissions stay behind");
        for (space, action) in [(a1, "moved to another space"), (a2, "moved here")] {
            let n: i64 =
                s.db.query_row(
                    "SELECT COUNT(*) FROM audit_log WHERE space_id = ?1 AND action = ?2",
                    params![space, action],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{action} in the audit log of space {space}");
        }
    }

    /// Two makerspaces on one server: neither sees, nor touches, the other's machines,
    /// files or spools; a member of an organisation is in all its spaces.
    #[tokio::test]
    async fn organisations_never_see_each_other() {
        let w = World::new();
        let (a_org, a1, a2, b_org, b1) = {
            let s = w.store.lock().unwrap();
            let owner = s.create_user("root", "Root", "x", true).unwrap();
            let (a_org, a1) = s
                .create_organisation("Fablab Noord", "Werkplaats", owner)
                .unwrap();
            let a2 = s.create_space(a_org, "Lab").unwrap();
            let (b_org, b1) = s.create_organisation("Makers Zuid", "Hal", owner).unwrap();
            (a_org, a1, a2, b_org, b1)
        };
        let profile = crate::profiles::find("prusa/mk3s").unwrap();
        let (a_machine, b_machine) = {
            let mut s = w.store.lock().unwrap();
            (
                s.add_virtual(a1, profile).unwrap(),
                s.add_virtual(b1, profile).unwrap(),
            )
        };
        let (_, ann) = w.person("ann", Some((a_org, Role::Member)), None);
        let (_, bob) = w.person("bob", Some((b_org, Role::Admin)), None);
        // Ann starts in the first of her spaces by name (the Lab): to the workshop.
        assert_eq!(
            w.post(&ann, "/spaces/switch", &format!("space={a1}"))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        // Each sees their own machine only.
        let (_, list) = w.get(&ann, "/api/machines").await;
        assert!(
            list.contains(&a_machine) && !list.contains(&b_machine),
            "{list}"
        );
        let (_, list) = w.get(&bob, "/api/machines").await;
        assert!(
            list.contains(&b_machine) && !list.contains(&a_machine),
            "{list}"
        );
        // Another organisation's machine is not there at all, even for an admin.
        let (status, _) = w
            .post(&bob, &format!("/machines/{a_machine}/command"), "gcode=G28")
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = w
            .post(&bob, &format!("/machines/{a_machine}/name"), "name=mine")
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        // A's materials are not B's: a vendor Ann adds is in her list, not in Bob's, and Bob
        // cannot read it by its number either.
        let (status, body) = w
            .send(
                HttpRequest::post("/api/v1/vendor")
                    .header("cookie", &ann)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name": "Prusament"}"#))
                    .unwrap(),
            )
            .await;
        assert!(status.is_success(), "{status} {body}");
        let vendor = serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"]
            .as_i64()
            .unwrap();
        assert!(w.get(&ann, "/api/v1/vendor").await.1.contains("Prusament"));
        assert_eq!(w.get(&bob, "/api/v1/vendor").await.1, "[]");
        assert_eq!(
            w.get(&bob, &format!("/api/v1/vendor/{vendor}")).await.0,
            StatusCode::NOT_FOUND
        );
        // Ann is in both of her organisation's spaces, and can go to either; not to B's.
        let spaces: Vec<i64> = w
            .store
            .lock()
            .unwrap()
            .spaces_of(2)
            .unwrap()
            .iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(spaces.len(), 2);
        assert!(spaces.contains(&a1) && spaces.contains(&a2));
        let (status, _) = w.post(&ann, "/spaces/switch", &format!("space={a2}")).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, list) = w.get(&ann, "/api/machines").await;
        assert_eq!(list, "[]", "the lab has no machines");
        let (status, _) = w.post(&ann, "/spaces/switch", &format!("space={b1}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// Roles: a guest looks only; a member uploads and runs open machines; only admins add
    /// machines and members; nobody gives a role above their own; an organisation keeps an
    /// owner.
    #[tokio::test]
    async fn roles_say_who_may() {
        let w = World::new();
        let (org, space) = {
            let s = w.store.lock().unwrap();
            let owner = s.create_user("olga", "Olga", "x", false).unwrap();
            s.create_organisation("Fablab", "Werkplaats", owner)
                .unwrap()
        };
        let olga = {
            let s = w.store.lock().unwrap();
            format!(
                "{}={}",
                crate::auth::COOKIE,
                s.create_session(1, None, "", "").unwrap()
            )
        };
        let (_, gus) = w.person("gus", Some((org, Role::Guest)), None);
        let (_, mia) = w.person("mia", Some((org, Role::Member)), None);
        let (adam_id, adam) = w.person("adam", Some((org, Role::Admin)), None);
        let machine = {
            let mut s = w.store.lock().unwrap();
            let m = s
                .add_virtual(space, crate::profiles::find("prusa/mk3s").unwrap())
                .unwrap();
            // Open to every member: this test is about roles, not permissions.
            s.set_induction_required(space, &m, false).unwrap();
            m
        };
        let upload = |cookie: &str| {
            HttpRequest::post("/api/files?name=cube.gcode")
                .header("cookie", cookie)
                .body(Body::from("G28\n"))
                .unwrap()
        };
        assert_eq!(w.send(upload(&gus)).await.0, StatusCode::FORBIDDEN);
        assert_eq!(w.send(upload(&mia)).await.0, StatusCode::CREATED);
        assert_eq!(
            w.get(&gus, "/api/machines").await.0,
            StatusCode::OK,
            "a guest looks"
        );
        assert_eq!(
            w.post(&gus, &format!("/machines/{machine}/command"), "gcode=G28")
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            w.post(&mia, &format!("/machines/{machine}/command"), "gcode=G28")
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            w.post(&mia, &format!("/machines/{machine}/name"), "name=x")
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            w.post(&mia, "/space/invites", "role=member").await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            w.post(&adam, &format!("/machines/{machine}/name"), "name=Gonzo")
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        // An admin does not make owners, nor touch one.
        assert_eq!(
            w.post(
                &adam,
                "/space/members/3/role",
                "scope=organisation&role=owner"
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            w.post(
                &adam,
                "/space/members/1/role",
                "scope=organisation&role=member"
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            w.post(
                &adam,
                "/space/members/3/role",
                "scope=organisation&role=admin"
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
        // The last owner stays one.
        assert_eq!(
            w.post(
                &olga,
                "/space/members/1/role",
                "scope=organisation&role=admin"
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            w.post(
                &olga,
                &format!("/space/members/{adam_id}/role"),
                "scope=organisation&role=owner"
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            w.post(
                &olga,
                "/space/members/1/role",
                "scope=organisation&role=admin"
            )
            .await
            .0,
            StatusCode::NO_CONTENT,
            "now there is another"
        );
    }

    /// A machine that needs an induction: a member without one may not use it; with "use"
    /// runs jobs but sends no G-code; with "operate" does both; an ended one counts no more.
    #[tokio::test]
    async fn inductions_open_a_machine() {
        let w = World::new();
        let (org, space) = {
            let s = w.store.lock().unwrap();
            let owner = s.create_user("olga", "Olga", "x", false).unwrap();
            s.create_organisation("Fablab", "Werkplaats", owner)
                .unwrap()
        };
        let (_, adam) = w.person("adam", Some((org, Role::Admin)), None);
        let (mia_id, mia) = w.person("mia", Some((org, Role::Member)), None);
        let laser = w
            .store
            .lock()
            .unwrap()
            .add_virtual(space, crate::profiles::find("ortur/lm2-pro-s2").unwrap())
            .unwrap();
        assert_eq!(
            w.post(&adam, &format!("/machines/{laser}/access"), "induction=1")
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        let command = format!("/machines/{laser}/command");
        let media = format!("/machines/{laser}/job");
        assert_eq!(
            w.post(&mia, &command, "gcode=G0").await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            w.post(&mia, &media, "action=pause").await.0,
            StatusCode::FORBIDDEN
        );
        let grants = format!("/machines/{laser}/grants");
        assert_eq!(
            w.post(&adam, &grants, &format!("user={mia_id}&level=use"))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            w.post(&mia, &command, "gcode=G0").await.0,
            StatusCode::FORBIDDEN,
            "use is not operate"
        );
        assert_ne!(
            w.post(&mia, &media, "action=pause").await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            w.post(&adam, &grants, &format!("user={mia_id}&level=operate"))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        // A laser in a space that asks for PPE: first the glasses (crate::safety). Laser off
        // never waits for them. (A space that does not ask: no glasses needed.)
        assert_eq!(
            w.post(&mia, &command, "gcode=G0").await.0,
            StatusCode::NO_CONTENT,
            "not asked: on"
        );
        assert_eq!(
            w.post(&mia, "/space/safety", "on=1").await.0,
            StatusCode::FORBIDDEN,
            "an admin switches it"
        );
        assert_eq!(
            w.post(&adam, "/space/safety", "on=1").await.0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            w.post(&mia, &command, "gcode=G0").await.0,
            StatusCode::PRECONDITION_REQUIRED
        );
        assert_eq!(
            w.post(&mia, &command, "action=laser-off").await.0,
            StatusCode::NO_CONTENT
        );
        let ppe = format!("/machines/{laser}/ppe");
        assert_eq!(
            w.post(&mia, &ppe, "").await.0,
            StatusCode::BAD_REQUEST,
            "nothing ticked"
        );
        assert_eq!(
            w.post(&mia, &ppe, "have=Laser+safety+glasses").await.0,
            StatusCode::OK
        );
        assert_eq!(
            w.post(&mia, &command, "gcode=G0").await.0,
            StatusCode::NO_CONTENT
        );
        // Its manager says it needs safety glasses and hearing protection: asked again, and
        // both are needed.
        assert_eq!(
            w.post(
                &adam,
                &format!("/machines/{laser}/safety"),
                "item=Safety+glasses&other=Hearing+protection"
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            w.post(&mia, &command, "gcode=G0").await.0,
            StatusCode::PRECONDITION_REQUIRED
        );
        assert_eq!(
            w.post(&mia, &ppe, "have=Safety+glasses").await.0,
            StatusCode::BAD_REQUEST,
            "not without hearing protection"
        );
        assert_eq!(
            w.post(&mia, &ppe, "have=Safety+glasses&have=Hearing+protection")
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            w.post(&mia, &command, "gcode=G0").await.0,
            StatusCode::NO_CONTENT
        );
        // Until a day long gone: ended.
        assert_eq!(
            w.post(
                &adam,
                &grants,
                &format!("user={mia_id}&level=operate&until=2020-01-01")
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            w.post(&mia, &command, "gcode=G0").await.0,
            StatusCode::FORBIDDEN
        );
        // Members may not give inductions.
        assert_eq!(
            w.post(&mia, &grants, &format!("user={mia_id}&level=operate"))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }

    /// An invite link: a newcomer makes an account and is in; the link works once.
    #[tokio::test]
    async fn an_invite_link_lets_one_person_in() {
        let w = World::new();
        let (org, space) = {
            let s = w.store.lock().unwrap();
            let owner = s.create_user("olga", "Olga", "x", false).unwrap();
            s.create_organisation("Fablab", "Werkplaats", owner)
                .unwrap()
        };
        let (_, adam) = w.person("adam", Some((org, Role::Admin)), None);
        let (status, body) = w
            .post(&adam, "/space/invites", "role=member&scope=organisation")
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let path = serde_json::from_str::<serde_json::Value>(&body).unwrap()["path"]
            .as_str()
            .unwrap()
            .to_owned();
        let (status, page) = w
            .send(HttpRequest::get(&path).body(Body::empty()).unwrap())
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            page.contains("adam") && page.contains("Fablab") && page.contains("member"),
            "{page}"
        );
        let join = |form: &str| {
            HttpRequest::post(&path)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form.to_owned()))
                .unwrap()
        };
        let (status, _) = w
            .send(join(
                "display_name=Nina&name=nina&password=a+long+password&again=a+long+password",
            ))
            .await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        let nina = w.store.lock().unwrap().user_id("nina").unwrap().unwrap();
        assert_eq!(
            w.store.lock().unwrap().role_of(nina, space).unwrap(),
            Some(Role::Member)
        );
        let (status, _) = w
            .send(join(
                "display_name=Other&name=other&password=a+long+password&again=a+long+password",
            ))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "used");
        // An admin of one space only cannot invite into the whole organisation.
        let (_, sam) = w.person("sam", None, Some((space, Role::Admin)));
        assert_eq!(
            w.post(&sam, "/space/invites", "role=member&scope=organisation")
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            w.post(&sam, "/space/invites", "role=member&scope=space")
                .await
                .0,
            StatusCode::OK
        );
    }

    /// An API token: in the address (as Moonraker sends it) or a header; its role holds;
    /// revoked, it stops; what it did is in the audit log under its name.
    #[tokio::test]
    async fn tokens_and_the_audit_log() {
        let w = World::new();
        let (org, space) = {
            let s = w.store.lock().unwrap();
            let owner = s.create_user("olga", "Olga", "x", false).unwrap();
            s.create_organisation("Fablab", "Werkplaats", owner)
                .unwrap()
        };
        let (_, adam) = w.person("adam", Some((org, Role::Admin)), None);
        let (status, body) = w
            .post(&adam, "/space/tokens", "name=Voron&role=member")
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let token = serde_json::from_str::<serde_json::Value>(&body).unwrap()["token"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(token.starts_with(TOKEN_PREFIX));
        // Moonraker: the token in the address, Spoolman's path after it.
        let (status, spools) = w
            .send(
                HttpRequest::get(format!("/t/{token}/api/v1/spool"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!((status, spools.as_str()), (StatusCode::OK, "[]"));
        let (status, _) = w
            .send(
                HttpRequest::post("/api/v1/vendor")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name": "Prusa"}"#))
                    .unwrap(),
            )
            .await;
        assert!(status.is_success(), "{status}");
        // A member's token does not add machines.
        let (status, _) = w
            .send(
                HttpRequest::post("/virtual")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("profile=prusa/mk3s"))
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        // A made-up token, a header someone set themselves: nothing.
        let (status, _) = w
            .send(
                HttpRequest::get("/t/mct_nonsense/api/v1/spool")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = w
            .send(
                HttpRequest::get("/api/v1/spool")
                    .header(crate::auth::TOKEN_HEADER, &token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "the header is the server's own, not the client's"
        );
        let log = w.store.lock().unwrap().audit_log(space, 10).unwrap();
        assert!(
            log.iter()
                .any(|l| l.actor == "token \"Voron\"" && l.action == "added a vendor"),
            "{log:?}"
        );
        assert!(
            log.iter()
                .any(|l| l.actor == "adam" && l.action == "made an API token"),
            "{log:?}"
        );
        // Revoked: it stops.
        let id = w.store.lock().unwrap().tokens(space).unwrap()[0].id;
        assert_eq!(
            w.post(&adam, &format!("/space/tokens/{id}/revoke"), "")
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        let (status, _) = w
            .send(
                HttpRequest::get(format!("/t/{token}/api/v1/spool"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
