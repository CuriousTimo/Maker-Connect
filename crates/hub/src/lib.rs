//! # Maker Connect hub
//!
//! The website and the API the agents report to. One binary with one SQLite file and no
//! other services, so it runs on a Pi, a NAS, an old laptop or in Docker; everything
//! belongs to a space ([`store`]), so the same software could serve many makerspaces.
//!
//! - `POST` [`protocol::REGISTER`], [`protocol::TELEMETRY`]: the agents ([`maker_connect_protocol`]).
//! - `GET /`: the machines of the space, live; `POST /pair`: add a machine with its code.
//! - `GET /api/machines`: the same list as JSON; `GET /api/machines/stream`: that list
//!   again on every change, as server-sent events, for the page.
//! - `/p/...`: the printer side of Prusa Connect, so a Buddy-firmware Prusa (a MINI)
//!   reports here itself, with no agent ([`prusa_connect`]).
//!
//! - `/login`, `/setup`, `/logout`: people sign in ([`auth`]); every other page and API
//!   needs a session, except the routes machines use with their own tokens.

pub mod about;
pub mod access;
pub mod accounts;
pub mod apps;
pub mod auth;
pub mod codes;
pub mod commands;
pub mod devices;
pub mod downloads;
pub mod farm;
pub mod files;
pub mod gcode;
pub mod link;
pub mod pages;
pub mod profiles;
pub mod queue;
pub mod safety;
pub mod samples;
pub mod spoolman;
pub mod spoolmandb;
pub mod stock;
pub mod store;
pub mod updates;
pub mod web;

use std::sync::{Arc, Mutex};
use store::Store;

pub type Shared = Arc<Mutex<Store>>;

pub(crate) use devices::legacy_gcode::{
    JOG_FEEDS, JOG_STEPS, action, grbl_action, jog_command, probe_gcode, prusa_mk3_machine,
};
pub use devices::{
    printer_files, prusa_connect, sd_card, simplyprint, virtual_grbl, virtual_machines,
};
pub(crate) use link::{agent_job, peer_address};
pub use pages::{
    access_page, files_page, label_page, machine_page, now_page, permissions_page, projects_page,
    space_page,
};
pub use web::router::app;
pub(crate) use web::router::{done, failed};
pub(crate) use web::shell::{PAGE, STYLE, escape, header};
