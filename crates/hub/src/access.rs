//! Who may do what, in which space: roles, rights per machine, invites, API tokens and the
//! audit log. The model of the wiki's *Design: Multi-Tenant Hub*:
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

use crate::Store;
use crate::store::{Legacy, now};

mod grants;
use axum::{
    extract::{FromRequestParts, MatchedPath, RawPathParams, Request, State},
    http::{Method, StatusCode, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
pub use grants::Grants;
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
    ("*", "/logo-large.svg", Need::Public, None),
    ("*", "/logo-large-dark.svg", Need::Public, None),
    ("*", "/p/register", Need::Public, None),
    ("*", "/p/telemetry", Need::Public, None),
    ("*", "/p/events", Need::Public, None),
    ("*", "/p/ws", Need::Public, None),
    // The agent fetches a file to print with its Token (the route checks it is placed there).
    (
        "GET",
        "/p/teams/{team}/files/{hash}/raw",
        Need::Public,
        None,
    ),
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
    // HS13: the LaserFoundry fork the hub serves; the handlers also ask that the person may use a laser.
    ("GET", "/prepare", Need::Role(Role::Guest), None),
    ("GET", "/laser-design", Need::Role(Role::Member), None),
    ("GET", "/laser-design/", Need::Role(Role::Member), None),
    (
        "GET",
        "/laser-design/{*file}",
        Need::Role(Role::Member),
        None,
    ),
    (
        "GET",
        "/api/laser-design/lasers",
        Need::Role(Role::Member),
        None,
    ),
    // One's own.
    ("POST", "/logout", Need::Signed, None),
    ("GET", "/api/me", Need::Signed, None),
    ("GET", "/welcome", Need::Signed, None),
    ("POST", "/spaces/switch", Need::Signed, None),
    ("POST", "/spaces", Need::Signed, None),
    ("POST", "/account/password", Need::Signed, None),
    ("POST", "/instance/open-spaces", Need::InstanceAdmin, None),
    // Updating the hub (crate::updates).
    ("GET", "/api/update", Need::InstanceAdmin, None),
    ("POST", "/about/update", Need::InstanceAdmin, None),
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
    ("GET", "/machines/add", Need::Role(Role::Admin), None),
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
    // An instance admin's, and only theirs (design decisions/20261119-device-move.md); the handler
    // writes the audit log of both spaces.
    ("POST", "/machines/{id}/move", Need::InstanceAdmin, None),
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
    // HS7b: Start burn fires the laser: operate it (memory maker-connect-laser-never-unattended), PPE and a
    // session are checked in the handler.
    (
        "POST",
        "/machines/{id}/start-burn",
        Need::Machine(Level::Operate),
        Some("started the burn on"),
    ),
    (
        "POST",
        "/machines/{id}/taken",
        Need::Machine(Level::Use),
        Some("took the print off"),
    ),
    (
        "POST",
        "/machines/{id}/not-ready",
        Need::Machine(Level::Use),
        Some("said it is not ready after all:"),
    ),
    // A button of the question on its screen.
    (
        "POST",
        "/machines/{id}/answer",
        Need::Machine(Level::Use),
        Some("answered the question on the screen of"),
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
    // Cameras (crate::cameras, owner 2026-11-16): watching is checked per camera by its
    // handler (`Legacy::may_view_camera`); adding, changing and removing take an admin.
    ("GET", "/cameras", Need::Role(Role::Guest), None),
    ("GET", "/cameras/{id}", Need::Role(Role::Guest), None),
    ("GET", "/cameras/{id}/live", Need::Role(Role::Guest), None),
    ("GET", "/cameras/{id}/ws", Need::Role(Role::Guest), None),
    (
        "POST",
        "/cameras",
        Need::Role(Role::Admin),
        Some("added a camera"),
    ),
    (
        "POST",
        "/cameras/{id}",
        Need::Role(Role::Admin),
        Some("changed a camera"),
    ),
    (
        "POST",
        "/cameras/{id}/remove",
        Need::Role(Role::Admin),
        Some("removed a camera"),
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
pub async fn permit(State(store): State<Store>, request: Request, next: Next) -> Response {
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
            (Some(a), Some(machine)) => match store
                .grants_for(a, level)
                .await
                .map_err(crate::store::BoxError::from)
                .map(|grants| store.lock().unwrap().may(a, &machine, level, &grants))
            {
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
        let _ = store.audit(access, label, &target, "").await;
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
    store: &Legacy,
    access: &Access,
    machine: &str,
    level: Level,
    grants: &Grants,
) -> Result<(), Box<Response>> {
    match store.may(access, machine, level, grants) {
        Some(true) => Ok(()),
        Some(false) => Err(Box::new(
            (StatusCode::FORBIDDEN, level_refusal(level)).into_response(),
        )),
        None => Err(Box::new(
            (StatusCode::NOT_FOUND, "no such machine").into_response(),
        )),
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
    /// What they ask for: use or operate.
    pub level: Level,
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

impl Legacy {
    /// Whether `access` may do `need` with a machine of its space; `None` when the machine is
    /// not in it. Managing it (settings, permissions, removing) is for owners and admins;
    /// using it (jobs, moving, heating) is by permission for everyone, whatever their role:
    /// being owner does not mean having had the laser course. `grants`: the person's
    /// ([`Store::grants_of`], read before the lock); [`Store::grants_for`] for view and manage.
    pub fn may(
        &self,
        access: &Access,
        machine: &str,
        need: Level,
        grants: &Grants,
    ) -> Option<bool> {
        debug_assert!(
            grants.read() || matches!(need, Level::View | Level::Manage),
            "use and operate depend on grants: read them first"
        );
        let level = self.level_with(access, machine, grants)?;
        Some(match need {
            Level::Manage => access.role >= Role::Admin,
            _ => level >= need,
        })
    }

    /// What `access` may do with a machine of its space as a user of it: view, use, operate
    /// (managing is [`Legacy::may`]'s, by role); `None` when the machine is not in it.
    pub fn machine_level(&self, access: &Access, machine: &str, grants: &Grants) -> Option<Level> {
        debug_assert!(grants.read(), "a level depends on grants: read them first");
        self.level_with(access, machine, grants)
    }

    fn level_with(&self, access: &Access, machine: &str, grants: &Grants) -> Option<Level> {
        let induction = grants.machine(machine).map(|m| m.induction_required != 0)?;
        Some(match access.role {
            Role::Guest => Level::View,
            _ if !induction => Level::Operate,
            // A member's permission on a machine, while it lasts: given on the machine, or on a
            // group it is in; the highest counts.
            _ => grants.of(machine).unwrap_or(Level::View),
        })
    }
}

/// For the tests that read every route: all of the hub's source files, in every folder.
#[cfg(test)]
pub(crate) fn route_sources() -> Vec<String> {
    fn read(dir: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                read(&path, out);
            } else if path.extension().is_some_and(|x| x == "rs") {
                out.push(std::fs::read_to_string(path).unwrap());
            }
        }
    }
    let mut out = Vec::new();
    read(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut out,
    );
    out.sort();
    assert!(out.len() > 20, "the hub's sources");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::Router;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use http_body_util::BodyExt;

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
                // Up to the next route, or the end of the router (not into what follows it).
                let call = piece.split(".route(").next().unwrap_or("");
                let call = call.split(';').next().unwrap_or("");
                let call = call.split("\n}").next().unwrap_or("");
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
        store: Store,
        app: Router,
    }

    impl World {
        async fn new(pool: &sqlx::PgPool) -> Self {
            let store = Store::for_test(pool).await;
            let app = crate::app(store.clone());
            World { store, app }
        }

        /// A user in an organisation (or in one space only) with a role; their cookie.
        async fn person(
            &self,
            name: &str,
            org: Option<(i64, Role)>,
            space: Option<(i64, Role)>,
        ) -> (i64, String) {
            let store = &self.store;
            let user = store.create_user(name, name, "x", false).await.unwrap();
            if let Some((org, role)) = org {
                store
                    .set_org_membership(user, store.a_space_of(org).await, role.as_str())
                    .await
                    .unwrap();
            }
            if let Some((space, role)) = space {
                store
                    .set_membership(user, space, role.as_str())
                    .await
                    .unwrap();
            }
            let token = store.create_session(user, None, "", "").await.unwrap();
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
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn groups_give_permission_on_many_machines(pool: sqlx::PgPool) {
        let w = World::new(&pool).await;
        let (org, space) = {
            let owner = w
                .store
                .create_user("olga", "Olga", "x", false)
                .await
                .unwrap();
            w.store
                .create_organisation("Fablab", "Werkplaats", owner)
                .await
                .unwrap()
        };
        let (_, adam) = w.person("adam", Some((org, Role::Admin)), None).await;
        let (mia_id, mia) = w.person("mia", Some((org, Role::Member)), None).await;
        let (p1, p2, laser) = {
            let mk3s = crate::profiles::find("prusa/mk3s").unwrap();
            (
                w.store.add_virtual(space, mk3s).await.unwrap(),
                w.store.add_virtual(space, mk3s).await.unwrap(),
                w.store
                    .add_virtual(space, crate::profiles::find("ortur/lm2-pro-s2").unwrap())
                    .await
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
        let level = async |m: &str| {
            let grants = w.store.grants_of(&as_mia).await.unwrap();
            w.store
                .lock()
                .unwrap()
                .machine_level(&as_mia, m, &grants)
                .unwrap()
        };
        assert_eq!(
            level(&p1).await,
            Level::View,
            "a new machine needs permission"
        );
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
        let group = w.store.groups(space).await.unwrap()[0].clone();
        assert_eq!(group.machines.len(), 2, "both printers, not the laser");
        // With all machines: the printers and the laser.
        let all = {
            let s = &w.store;
            let id = s
                .create_group(space, "Everything", Some("all"))
                .await
                .unwrap();
            let n = s
                .groups(space)
                .await
                .unwrap()
                .into_iter()
                .find(|g| g.id == id)
                .unwrap()
                .machines
                .len();
            s.remove_group(space, id).await.unwrap();
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
            (level(&p1).await, level(&p2).await, level(&laser).await),
            (Level::Use, Level::Use, Level::View)
        );
        // Per machine as well: the most counts.
        w.post(
            &adam,
            &format!("/machines/{p1}/grants"),
            &format!("user={mia_id}&level=operate"),
        )
        .await;
        assert_eq!(
            (level(&p1).await, level(&p2).await),
            (Level::Operate, Level::Use)
        );
        // A printer put in later is opened too; one taken out is closed again.
        let p3 = w
            .store
            .add_virtual(space, crate::profiles::find("prusa/mk3s").unwrap())
            .await
            .unwrap();
        assert_eq!(level(&p3).await, Level::View);
        w.post(
            &adam,
            &format!("/permissions/groups/{}/machines", group.id),
            &format!("machine={p3}"),
        )
        .await;
        assert_eq!(level(&p3).await, Level::Use);
        w.post(
            &adam,
            &format!("/permissions/groups/{}/machines/{p2}/remove", group.id),
            "",
        )
        .await;
        assert_eq!(level(&p2).await, Level::View);
        // Ended: only what the machine itself gives is left.
        w.post(
            &adam,
            &grants,
            &format!("user={mia_id}&level=use&until=2020-01-01"),
        )
        .await;
        assert_eq!(
            (level(&p1).await, level(&p3).await),
            (Level::Operate, Level::View)
        );
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
        assert_eq!(
            (level(&p1).await, level(&p3).await),
            (Level::Operate, Level::View)
        );
        // From the Members list: on a group, or on a machine; only to members.
        let g2 = w
            .store
            .create_group(space, "Printers", Some("fdm"))
            .await
            .unwrap();
        let give = format!("/permissions/people/{mia_id}/permission");
        assert_eq!(
            w.post(&adam, &give, &format!("on=g:{g2}&level=use"))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(level(&p3).await, Level::Use);
        assert_eq!(
            w.post(&adam, &give, &format!("on=m:{laser}&level=operate"))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(level(&laser).await, Level::Operate);
        assert_eq!(
            w.post(&mia, &give, &format!("on=m:{laser}&level=operate"))
                .await
                .0,
            StatusCode::FORBIDDEN,
            "members give no permission"
        );
        let adam_id = w.store.user_id("adam").await.unwrap().unwrap();
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
        let adam_may = async |need: Level| {
            let grants = w.store.grants_for(&as_adam, need).await.unwrap();
            w.store
                .lock()
                .unwrap()
                .may(&as_adam, &laser, need, &grants)
                .unwrap()
        };
        assert!(adam_may(Level::Manage).await && !adam_may(Level::Use).await);
        assert_eq!(
            w.post(&adam, &format!("/machines/{laser}/name"), "name=Laser")
                .await
                .0,
            StatusCode::NO_CONTENT,
            "managing: yes"
        );
        assert_eq!(
            w.post(&adam, &format!("/machines/{laser}/command"), "action=home")
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
        assert!(adam_may(Level::Operate).await);
        // Open to every member, and closed again: the two choices.
        w.post(&adam, &format!("/machines/{p2}/access"), "induction=0")
            .await;
        assert_eq!(
            level(&p2).await,
            Level::Operate,
            "open: every member operates it"
        );
        w.post(&adam, &format!("/machines/{p2}/access"), "induction=1")
            .await;
        assert_eq!(
            level(&p2).await,
            Level::Use,
            "closed again: through the group Printers"
        );
        // On Space → Permissions: an admin sees everyone's permissions, with a Permissions…
        // window per person; a member sees only their own (My permissions), without it.
        let (status, page) = w.get(&adam, "/space/permissions").await;
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
        assert!(
            page.contains(r#"<a href="/space/permissions" aria-current="page">Permissions</a>"#)
        );
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
        assert_eq!(level(&p3).await, Level::View);
        let (status, now) = w.post(&adam, &tick, "on=1").await;
        assert_eq!((status, now.as_str()), (StatusCode::OK, "operate"));
        assert_eq!(level(&p3).await, Level::Operate);
        let (status, now) = w.post(&adam, &tick, "on=1&level=use").await;
        assert_eq!((status, now.as_str()), (StatusCode::OK, "use"));
        assert_eq!(level(&p3).await, Level::Use);
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
            (level(&p3).await, level(&laser).await),
            (Level::Operate, Level::View),
            "the laser's own permission: unticked, gone"
        );
        assert_eq!(
            level(&p1).await,
            Level::Operate,
            "the most of group and machine"
        );
        assert_eq!(w.post(&mia, &save, "").await.0, StatusCode::FORBIDDEN);
        assert_eq!(w.post(&adam, &save, "").await.0, StatusCode::NO_CONTENT);
        assert_eq!(
            (level(&p1).await, level(&p3).await),
            (Level::View, Level::View),
            "nothing ticked: nothing left"
        );
        // Needs training: Mia asks the staff (once, however often she clicks); the staff
        // give it, and she may use it.
        let p4 = w
            .store
            .add_virtual(space, crate::profiles::find("prusa/mk3s").unwrap())
            .await
            .unwrap();
        assert_eq!(
            w.post(&adam, &format!("/machines/{p4}/access"), "induction=2")
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        assert!(w.store.machine(space, &p4).await.unwrap().unwrap().training);
        assert_eq!(level(&p4).await, Level::View);
        let ask = format!("/machines/{p4}/ask");
        assert_eq!(w.post(&mia, &ask, "").await.0, StatusCode::NO_CONTENT);
        assert_eq!(w.post(&mia, &ask, "").await.0, StatusCode::NO_CONTENT);
        let asked = w.store.requests(space).await.unwrap();
        assert_eq!(asked.len(), 1, "asked once");
        assert!(asked[0].training);
        let (_, page) = w.get(&adam, "/space/permissions").await;
        assert!(page.contains("asks for a training"), "the staff see it");
        let give = format!("/permissions/requests/{}/give", asked[0].id);
        assert_eq!(
            w.post(&mia, &give, "").await.0,
            StatusCode::FORBIDDEN,
            "not hers to give"
        );
        assert_eq!(w.post(&adam, &give, "").await.0, StatusCode::NO_CONTENT);
        assert_eq!(level(&p4).await, Level::Use);
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
        // Using is not operating: Controls asks for 'operate' (once); Give gives that.
        let page = w.get(&mia, &format!("/machines/{p4}")).await.1;
        assert!(
            page.contains(r#"<input type="hidden" name="level" value="operate"><button>Ask for permission</button>"#),
            "Controls offers to ask"
        );
        assert_eq!(
            w.post(&mia, &ask, "level=run").await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            w.post(&mia, &ask, "level=operate").await.0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            w.post(&mia, &ask, "level=operate").await.0,
            StatusCode::NO_CONTENT
        );
        let asked = w.store.requests(space).await.unwrap();
        assert_eq!(asked.len(), 1, "asked once");
        assert_eq!(asked[0].level, Level::Operate);
        let page = w.get(&mia, &format!("/machines/{p4}")).await.1;
        assert!(
            !page.contains(r#"value="operate"><button>"#),
            "asked: no button"
        );
        let (_, page) = w.get(&adam, "/space/permissions").await;
        assert!(page.contains("asks to operate"), "the staff see it");
        let give = format!("/permissions/requests/{}/give", asked[0].id);
        assert_eq!(w.post(&adam, &give, "").await.0, StatusCode::NO_CONTENT);
        assert_eq!(level(&p4).await, Level::Operate);
        assert_eq!(
            w.post(&mia, &ask, "level=operate").await.0,
            StatusCode::CONFLICT,
            "she may operate it now"
        );
        // Someone else's space's group: not found.
        let other = {
            let owner = w.store.create_user("zed", "Zed", "x", false).await.unwrap();
            let (_, b) = w
                .store
                .create_organisation("Zuid", "Hal", owner)
                .await
                .unwrap();
            w.store.create_group(b, "Theirs", None).await.unwrap()
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
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_queue_is_changed_by_whom_it_is_of(pool: sqlx::PgPool) {
        let w = World::new(&pool).await;
        let (org, space) = {
            let owner = w
                .store
                .create_user("olga", "Olga", "x", false)
                .await
                .unwrap();
            w.store
                .create_organisation("Fablab", "Werkplaats", owner)
                .await
                .unwrap()
        };
        let (_, adam) = w.person("adam", Some((org, Role::Admin)), None).await;
        let (mia_id, mia) = w.person("mia", Some((org, Role::Member)), None).await;
        let (zoe_id, _) = w.person("zoe", Some((org, Role::Member)), None).await;
        let (m, a, b, c) = {
            let m = w
                .store
                .add_virtual(space, crate::profiles::find("prusa/mk3s").unwrap())
                .await
                .unwrap();
            w.store
                .add_file(space, "f1", "a.gcode", 10, &Default::default())
                .await
                .unwrap();
            let file = w.store.file(space, "f1").await.unwrap().unwrap();
            for by in [(mia_id, "mia"), (zoe_id, "zoe"), (mia_id, "mia")] {
                w.store.queue_add(space, &m, &file, Some(by)).await.unwrap();
            }
            let q: Vec<i64> = w
                .store
                .queue_of(space, &m)
                .await
                .unwrap()
                .iter()
                .map(|q| q.id)
                .collect();
            (m, q[0], q[1], q[2])
        };
        let order = format!("/machines/{m}/queue/order");
        let ids = |v: [i64; 3]| format!("ids={},{},{}", v[0], v[1], v[2]);
        let now = async || -> Vec<i64> {
            w.store
                .queue_of(space, &m)
                .await
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
        assert_eq!(now().await, [c, b, a]);
        // Moving Zoe's, or getting ahead of it: no.
        assert_eq!(
            w.post(&mia, &order, &ids([b, c, a])).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            w.post(&mia, &order, &ids([c, a, b])).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(now().await, [c, b, a], "unchanged");
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
        assert_eq!(now().await, [b, a, c]);
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
        assert_eq!(now().await, [c]);
    }

    /// A machine moves to another space, by an instance admin only (design
    /// decisions/20261119-device-move.md): not by an admin of the organisation, not while it has
    /// work (`409 device/JOB_OPEN`). Permissions on it stay behind.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_machine_moves_to_another_space(pool: sqlx::PgPool) {
        let w = World::new(&pool).await;
        let (_, a1, a2) = {
            let owner = w
                .store
                .create_user("olga", "Olga", "x", false)
                .await
                .unwrap();
            let (org, a1) = w
                .store
                .create_organisation("Fablab", "Werkplaats", owner)
                .await
                .unwrap();
            let a2 = w.store.create_space(a1, "Lab").await.unwrap();
            (org, a1, a2)
        };
        // All signed in to the first space.
        let in_first = async |name: &str, role: Role, instance_admin: bool| {
            let user = w
                .store
                .create_user(name, name, "x", instance_admin)
                .await
                .unwrap();
            w.store
                .set_org_membership(user, a1, role.as_str())
                .await
                .unwrap();
            let token = w
                .store
                .create_session(user, Some(a1), "", "")
                .await
                .unwrap();
            (user, format!("{}={token}", crate::auth::COOKIE))
        };
        let (_, adam) = in_first("adam", Role::Admin, false).await;
        let (mia_id, mia) = in_first("mia", Role::Member, false).await;
        // Who runs the server, an admin here too.
        let (_, ian) = in_first("ian", Role::Admin, true).await;
        let m = {
            let m = w
                .store
                .add_virtual(a1, crate::profiles::find("prusa/mk3s").unwrap())
                .await
                .unwrap();
            w.store.set_takes_work(a1, &m, true).await.unwrap();
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
        assert_eq!(
            w.store.grants(a1, &m).await.unwrap().len(),
            1,
            "mia's permission before the move"
        );
        let uid_a2 = w.store.space_uid(a2).await.unwrap();
        let to = |uid: &str| format!("space={uid}");
        let path = format!("/machines/{m}/move");
        assert_eq!(
            w.post(&mia, &path, &to(&uid_a2)).await.0,
            StatusCode::FORBIDDEN,
            "a member does not manage it"
        );
        assert_eq!(
            w.post(&adam, &path, &to(&uid_a2)).await.0,
            StatusCode::FORBIDDEN,
            "an admin of the organisation does not move machines between spaces"
        );
        assert_eq!(
            w.post(&ian, &path, "space=nowhere").await.0,
            StatusCode::NOT_FOUND,
            "no such space"
        );
        // Work under way: not now.
        sqlx::query(
            "INSERT INTO jobs (space_id, machine_id, file_id, file_name, state, created_at) VALUES ($1, $2, 'f', 'a.gcode', 'printing', at_unix(0))",
        )
        .bind(a1)
        .bind(&m)
        .execute(w.store.pg().admin_in_tests())
        .await
        .unwrap();
        let (status, why) = w.post(&ian, &path, &to(&uid_a2)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{why}");
        assert_eq!(why, r#"{"code":"device/JOB_OPEN"}"#);
        sqlx::query("UPDATE jobs SET state = 'stopped', taken_at = at_unix(1)")
            .execute(w.store.pg().admin_in_tests())
            .await
            .unwrap();
        let (status, body) = w.post(&ian, &path, &to(&uid_a2)).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        {
            assert!(
                w.store.machine(a1, &m).await.unwrap().is_none(),
                "gone from the first space"
            );
            let moved = w.store.machine(a2, &m).await.unwrap();
            assert!(moved.is_some(), "in the second");
            assert!(
                !moved.unwrap().takes_work,
                "no work from the new space's queue until someone says"
            );
        }
        let grants: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM machine_grants WHERE machine_id = $1")
                .bind(&m)
                .fetch_one(w.store.pg().admin_in_tests())
                .await
                .unwrap();
        assert_eq!(grants, 0, "permissions stay behind");
        for (space, action) in [(a1, "moved to another space"), (a2, "moved here")] {
            let n = w
                .store
                .audit_log(space, 500)
                .await
                .unwrap()
                .iter()
                .filter(|l| l.action == action)
                .count();
            assert_eq!(n, 1, "{action} in the audit log of space {space}");
        }
    }

    /// Two makerspaces on one server: neither sees, nor touches, the other's machines,
    /// files or spools; a member of an organisation is in all its spaces.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn organisations_never_see_each_other(pool: sqlx::PgPool) {
        let w = World::new(&pool).await;
        let (a_org, a1, a2, b_org, b1) = {
            let owner = w
                .store
                .create_user("root", "Root", "x", true)
                .await
                .unwrap();
            let (a_org, a1) = w
                .store
                .create_organisation("Fablab Noord", "Werkplaats", owner)
                .await
                .unwrap();
            let a2 = w.store.create_space(a1, "Lab").await.unwrap();
            let (b_org, b1) = w
                .store
                .create_organisation("Makers Zuid", "Hal", owner)
                .await
                .unwrap();
            (a_org, a1, a2, b_org, b1)
        };
        let profile = crate::profiles::find("prusa/mk3s").unwrap();
        let (a_machine, b_machine) = (
            w.store.add_virtual(a1, profile).await.unwrap(),
            w.store.add_virtual(b1, profile).await.unwrap(),
        );
        let (_, ann) = w.person("ann", Some((a_org, Role::Member)), None).await;
        let (_, bob) = w.person("bob", Some((b_org, Role::Admin)), None).await;
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
            .spaces_of(2)
            .await
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
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn roles_say_who_may(pool: sqlx::PgPool) {
        let w = World::new(&pool).await;
        let (org, space) = {
            let owner = w
                .store
                .create_user("olga", "Olga", "x", false)
                .await
                .unwrap();
            w.store
                .create_organisation("Fablab", "Werkplaats", owner)
                .await
                .unwrap()
        };
        let olga = {
            format!(
                "{}={}",
                crate::auth::COOKIE,
                w.store.create_session(1, None, "", "").await.unwrap()
            )
        };
        let (_, gus) = w.person("gus", Some((org, Role::Guest)), None).await;
        let (_, mia) = w.person("mia", Some((org, Role::Member)), None).await;
        let (adam_id, adam) = w.person("adam", Some((org, Role::Admin)), None).await;
        let machine = {
            let m = w
                .store
                .add_virtual(space, crate::profiles::find("prusa/mk3s").unwrap())
                .await
                .unwrap();
            // Open to every member: this test is about roles, not permissions.
            w.store
                .set_induction_required(space, &m, false)
                .await
                .unwrap();
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
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn inductions_open_a_machine(pool: sqlx::PgPool) {
        let w = World::new(&pool).await;
        let (org, space) = {
            let owner = w
                .store
                .create_user("olga", "Olga", "x", false)
                .await
                .unwrap();
            w.store
                .create_organisation("Fablab", "Werkplaats", owner)
                .await
                .unwrap()
        };
        let (_, adam) = w.person("adam", Some((org, Role::Admin)), None).await;
        let (mia_id, mia) = w.person("mia", Some((org, Role::Member)), None).await;
        let laser = w
            .store
            .add_virtual(space, crate::profiles::find("ortur/lm2-pro-s2").unwrap())
            .await
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
            w.post(&mia, &command, "action=home").await.0,
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
            w.post(&mia, &command, "action=home").await.0,
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
        // A laser in a space that asks for PPE: first the glasses (crate::safety). Switching
        // off never waits for them. (A space that does not ask: no glasses needed.)
        assert_eq!(
            w.post(&mia, &command, "action=home").await.0,
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
            w.post(&mia, &command, "action=home").await.0,
            StatusCode::PRECONDITION_REQUIRED
        );
        assert_ne!(
            w.post(&mia, &command, "action=laser-off").await.0,
            StatusCode::NO_CONTENT,
            "Laser off is gone"
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
            w.post(&mia, &command, "action=home").await.0,
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
            w.post(&mia, &command, "action=home").await.0,
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
            w.post(&mia, &command, "action=home").await.0,
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
            w.post(&mia, &command, "action=home").await.0,
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

    /// A laser job starts only when a person, in their session, presses Start: `print` and
    /// "Start again from there" do; an API token's print is refused; the copies after the
    /// first wait in the queue and nothing starts them.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_laser_starts_only_by_a_person(pool: sqlx::PgPool) {
        use crate::store::{JobState, LASER_NEVER_ALONE};
        let w = World::new(&pool).await;
        let (org, space) = {
            let owner = w
                .store
                .create_user("olga", "Olga", "x", false)
                .await
                .unwrap();
            w.store
                .create_organisation("Fablab", "Werkplaats", owner)
                .await
                .unwrap()
        };
        let (adam_id, adam) = w.person("adam", Some((org, Role::Admin)), None).await;
        let (laser, token) = {
            let s = &w.store;
            let laser = s
                .add_virtual(space, crate::profiles::find("ortur/lm2-pro-s2").unwrap())
                .await
                .unwrap();
            let token = s
                .virtual_machines()
                .await
                .unwrap()
                .into_iter()
                .find(|(m, ..)| *m == laser)
                .unwrap()
                .1;
            s.add_file(
                space,
                "f1",
                "sign.gcode",
                10,
                &crate::gcode::FileMeta {
                    kind: Some(maker_connect_protocol::MachineKind::Laser),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            s.report(&token, &Default::default()).await.unwrap();
            (laser, token)
        };
        async fn jobs(w: &World, laser: &str) -> (i64, usize) {
            let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE machine_id = $1")
                .bind(laser)
                .fetch_one(w.store.pg().admin_in_tests())
                .await
                .unwrap();
            let space: i64 = sqlx::query_scalar("SELECT space_id FROM machines WHERE id = $1")
                .bind(laser)
                .fetch_one(w.store.pg().admin_in_tests())
                .await
                .unwrap();
            (n, w.store.queue_of(space, laser).await.unwrap().len())
        }
        // An API token (no session) may not start a laser, even with permission to use it.
        assert_eq!(
            w.post(
                &adam,
                &format!("/machines/{laser}/grants"),
                &format!("user={adam_id}&level=operate")
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
        let (status, body) = w.post(&adam, "/space/tokens", "name=Bot&role=admin").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let api = serde_json::from_str::<serde_json::Value>(&body).unwrap()["token"]
            .as_str()
            .unwrap()
            .to_owned();
        let (status, body) = w
            .send(
                HttpRequest::post("/files/f1/print")
                    .header("authorization", format!("Bearer {api}"))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("machine={laser}")))
                    .unwrap(),
            )
            .await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::CONFLICT, LASER_NEVER_ALONE),
            "a token never starts a laser"
        );
        assert_eq!(jobs(&w, &laser).await, (0, 0));
        // HS7b: a laser takes one loaded job: no copies, no queue.
        let (status, body) = w
            .post(
                &adam,
                "/files/f1/print",
                &format!("machine={laser}&copies=3"),
            )
            .await;
        assert_eq!(
            (status, body.as_str()),
            (
                StatusCode::CONFLICT,
                "a laser takes one job at a time: no copies"
            )
        );
        // A person: the job is loaded, not started; nothing starts it by itself.
        let (status, body) = w
            .post(&adam, "/files/f1/print", &format!("machine={laser}"))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(jobs(&w, &laser).await, (1, 0));
        {
            let s = &w.store;
            let job = s.active_job(space, &laser).await.unwrap().unwrap();
            assert_eq!(job.state, JobState::Loaded);
            assert_eq!(job.by.as_deref(), Some("adam"));
            assert_eq!(s.start_queued().await.unwrap(), 0, "a loaded job waits");
            assert_eq!(s.dispatch().await.unwrap(), 0);
        }
        // A token's replace takes nothing off: the loaded job stays (it is refused first).
        let (status, body) = w
            .send(
                HttpRequest::post("/files/f1/print")
                    .header("authorization", format!("Bearer {api}"))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("machine={laser}&replace=true")))
                    .unwrap(),
            )
            .await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::CONFLICT, LASER_NEVER_ALONE)
        );
        assert_eq!(
            w.store
                .active_job(space, &laser)
                .await
                .unwrap()
                .unwrap()
                .state,
            JobState::Loaded
        );
        // Stop on a loaded job: not now (the job never ran); the job stays loaded.
        let (status, body) = w
            .post(&adam, &format!("/machines/{laser}/job"), "action=stop")
            .await;
        assert_eq!(
            (status, body.as_str()),
            (
                StatusCode::CONFLICT,
                "not now: the job is loaded, not started"
            )
        );
        assert_eq!(
            w.store
                .active_job(space, &laser)
                .await
                .unwrap()
                .unwrap()
                .state,
            JobState::Loaded
        );
        let (status, _) = w
            .post(&adam, &format!("/machines/{laser}/job"), "action=pause")
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "a loaded job is not paused");
        // "Take it off" (unload): it ends stopped, never started.
        let (status, body) = w
            .post(&adam, &format!("/machines/{laser}/job"), "action=unload")
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        assert!(w.store.active_job(space, &laser).await.unwrap().is_none());
        let (status, body) = w
            .post(&adam, "/files/f1/print", &format!("machine={laser}"))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(jobs(&w, &laser).await, (2, 0));
        {
            let s = &w.store;
            let job = s.active_job(space, &laser).await.unwrap().unwrap();
            s.set_job(space, job.id, JobState::Finished, None)
                .await
                .unwrap();
        }
        w.store.report(&token, &Default::default()).await.unwrap();
        assert_eq!(jobs(&w, &laser).await, (2, 0));
        // "Start again from there": a person; interrupted jobs start again.
        {
            let s = &w.store;
            let job = s.job(space, 2).await.unwrap().unwrap();
            s.set_job(space, job.id, JobState::Stopped, None)
                .await
                .unwrap();
            s.set_restart_line(space, job.id, 5).await.unwrap();
        }
        let (status, body) = w
            .post(&adam, &format!("/machines/{laser}/job"), "action=restart")
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        assert_eq!(jobs(&w, &laser).await.0, 3);
        // Stop or pause with no job: "no job on this machine".
        w.store
            .set_job(space, 3, JobState::Finished, None)
            .await
            .unwrap();
        w.store.report(&token, &Default::default()).await.unwrap();
        for action in ["stop", "pause"] {
            let (status, body) = w
                .post(
                    &adam,
                    &format!("/machines/{laser}/job"),
                    &format!("action={action}"),
                )
                .await;
            assert_eq!(
                (status, body.as_str()),
                (StatusCode::NOT_FOUND, "no job on this machine"),
                "{action}"
            );
        }
    }

    /// An invite link: a newcomer makes an account and is in; the link works once.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn an_invite_link_lets_one_person_in(pool: sqlx::PgPool) {
        let w = World::new(&pool).await;
        let (org, space) = {
            let owner = w
                .store
                .create_user("olga", "Olga", "x", false)
                .await
                .unwrap();
            w.store
                .create_organisation("Fablab", "Werkplaats", owner)
                .await
                .unwrap()
        };
        let (_, adam) = w.person("adam", Some((org, Role::Admin)), None).await;
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
        let nina = w.store.user_id("nina").await.unwrap().unwrap();
        assert_eq!(
            w.store.role_of(nina, space).await.unwrap(),
            Some(Role::Member)
        );
        let (status, _) = w
            .send(join(
                "display_name=Other&name=other&password=a+long+password&again=a+long+password",
            ))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "used");
        // An admin of one space only cannot invite into the whole organisation.
        let (_, sam) = w.person("sam", None, Some((space, Role::Admin))).await;
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
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn tokens_and_the_audit_log(pool: sqlx::PgPool) {
        let w = World::new(&pool).await;
        let (org, space) = {
            let owner = w
                .store
                .create_user("olga", "Olga", "x", false)
                .await
                .unwrap();
            w.store
                .create_organisation("Fablab", "Werkplaats", owner)
                .await
                .unwrap()
        };
        let (_, adam) = w.person("adam", Some((org, Role::Admin)), None).await;
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
        let log = w.store.audit_log(space, 10).await.unwrap();
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
        let id = w.store.tokens(space).await.unwrap()[0].id;
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

    /// Every route and what it answers, as checked in (`tests/routes.txt`): the route paths
    /// as `route_sources` parses them, `RULES`, the status of each method on each path for
    /// each role and for no session (the router called with made-up path parameters), and a
    /// hash of the page shell and of every embedded style and script. Moving code must not
    /// change it. `MAKER_CONNECT_REGENERATE_ROUTES=1` writes it anew.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(all(feature = "cameras", feature = "pg-tests")),
        ignore = "the snapshot is of the build with all modules (docs/plans/hub-modules.md), on Postgres"
    )]
    async fn the_routes_are_as_checked_in(pool: sqlx::PgPool) {
        use sha2::{Digest, Sha256};
        use std::fmt::Write as _;
        let hash = |bytes: &[u8]| -> String {
            Sha256::digest(bytes)
                .iter()
                .fold(String::new(), |mut out, b| {
                    let _ = write!(out, "{b:02x}");
                    out
                })
        };

        // The route paths.
        let mut paths = std::collections::BTreeSet::new();
        for source in route_sources() {
            for piece in source.split(".route(").skip(1) {
                let piece = piece.trim_start();
                let path = match piece.strip_prefix('"').and_then(|r| r.split('"').next()) {
                    Some(p) if p.starts_with('/') => p,
                    _ => continue,
                };
                paths.insert(path.to_owned());
            }
        }
        let mut text = String::new();
        text.push_str("# route paths\n");
        for path in &paths {
            let _ = writeln!(text, "{path}");
        }
        text.push_str("# RULES\n");
        for rule in RULES {
            let _ = writeln!(text, "{rule:?}");
        }

        // The status of each method on each path, for no session and for each role.
        text.push_str("# status: method path, then none guest member admin owner\n");
        // A new hub: the store emptied again, its owner and organisation, and a session for
        // no one and for each role.
        async fn new_hub(pool: &sqlx::PgPool) -> (World, Vec<Option<String>>) {
            let store = Store::fresh_for_test(pool).await;
            let w = World {
                app: crate::app(store.clone()),
                store,
            };
            let owner = w
                .store
                .create_user("olga", "Olga", "x", false)
                .await
                .unwrap();
            let (org, _) = w
                .store
                .create_organisation("Fablab", "Werkplaats", owner)
                .await
                .unwrap();
            let mut cookies: Vec<Option<String>> = vec![None];
            for role in [Role::Guest, Role::Member, Role::Admin, Role::Owner] {
                let name = role.as_str();
                cookies.push(Some(w.person(name, Some((org, role)), None).await.1));
            }
            (w, cookies)
        }
        let mut fresh: Option<(World, Vec<Option<String>>)> = None;
        for path in &paths {
            let mut address = String::new();
            let mut rest = path.as_str();
            while let Some(open) = rest.find('{') {
                address.push_str(&rest[..open]);
                let close = rest[open..].find('}').unwrap() + open;
                address.push_str(if rest[open..].starts_with("{*") {
                    "9/9"
                } else {
                    "9"
                });
                rest = &rest[close + 1..];
            }
            address.push_str(rest);
            for method in [
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
            ] {
                // Every line starts on a new hub (the database emptied again). A line whose
                // calls were all refused before a handler ran (401, 403, 405) leaves the hub as
                // it was, so the next line keeps it.
                if fresh.is_none() {
                    fresh = Some(new_hub(&pool).await);
                }
                let (w, cookies) = fresh.as_ref().unwrap();
                let mut keep = true;
                let mut line = format!("{method} {path} =>");
                for cookie in cookies {
                    let mut request = HttpRequest::builder()
                        .method(method.clone())
                        .uri(&address)
                        .header("x-requested-with", "fetch");
                    if let Some(cookie) = cookie {
                        request = request.header("cookie", cookie);
                    }
                    let response = w
                        .app
                        .clone()
                        .oneshot(request.body(Body::empty()).unwrap())
                        .await
                        .unwrap();
                    let status = response.status().as_u16();
                    keep &= matches!(status, 401 | 403 | 405);
                    let _ = write!(line, " {status}");
                }
                if !keep {
                    fresh = None;
                }
                text.push_str(&line);
                text.push('\n');
            }
        }

        // The page shell, and every embedded style and script, by file.
        text.push_str("# embedded\n");
        let _ = writeln!(
            text,
            "header {}",
            hash(crate::header("Space", "", "").as_bytes())
        );
        fn assets(dir: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    assets(&path, out);
                } else if path.extension().is_some_and(|x| x == "css" || x == "js") {
                    let name = path.file_name().unwrap().to_string_lossy().into_owned();
                    out.push((name, std::fs::read(&path).unwrap()));
                }
            }
        }
        let mut files = Vec::new();
        assets(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        files.sort();
        for (name, bytes) in files {
            let _ = writeln!(text, "{name} {}", hash(&bytes));
        }

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/routes.txt");
        if std::env::var("MAKER_CONNECT_REGENERATE_ROUTES").is_ok_and(|v| v == "1") {
            std::fs::write(path, &text).unwrap();
        } else {
            assert!(
                text == include_str!("../tests/routes.txt"),
                "the routes differ from tests/routes.txt"
            );
        }
    }
}
