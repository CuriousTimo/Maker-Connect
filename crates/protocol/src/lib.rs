//! What an agent (the module next to a machine) and the cloud say to each other. Shared by
//! both, so they always fit.
//!
//! The agent always starts the conversation, as Prusa Link does with Prusa Connect: it
//! registers once, then keeps reporting its machine's state over HTTPS (JSON). So an agent
//! works behind any NAT or firewall, with no port opened at the makerspace. Commands for
//! the machine come back in the replies.
//!
//! 1. `POST` [`REGISTER`] with [`Register`]: the cloud answers [`Registered`] with the
//!    machine's id, a token for everything after, and a pairing code. The agent shows the
//!    code; someone enters it on the website, which adds the machine to their space.
//! 2. `POST` [`TELEMETRY`] with [`Telemetry`] every few seconds, the token as
//!    `Authorization: Bearer <token>`: the cloud answers [`TelemetryReply`], with the
//!    commands for the machine, if any. The next report says how they went
//!    ([`Telemetry::done`]), and brings a bed mesh when one was measured.
//!
//! Every message carries [`VERSION`] (in [`Register`]), so agents and the cloud can be
//! updated apart.

use serde::{Deserialize, Serialize};

/// The protocol version this crate speaks.
pub const VERSION: u32 = 1;

pub const REGISTER: &str = "/api/agent/v1/register";
pub const TELEMETRY: &str = "/api/agent/v1/telemetry";

/// What kind of machine an agent drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MachineKind {
    /// An FDM 3D printer.
    Fdm,
    Laser,
    Cnc,
    /// A cutting plotter: vinyl, paper, card (Cricut, Silhouette, Roland).
    Cutter,
    /// A label printer (Brother, DYMO, Zebra).
    Label,
    /// A paper printer.
    Paper,
    Other,
}

impl MachineKind {
    pub fn label(self) -> &'static str {
        match self {
            MachineKind::Fdm => "3D printer",
            MachineKind::Laser => "Laser",
            MachineKind::Cnc => "CNC",
            MachineKind::Cutter => "Cutting plotter",
            MachineKind::Label => "Label printer",
            MachineKind::Paper => "Paper printer",
            MachineKind::Other => "Machine",
        }
    }
}

/// An agent introduces itself and its machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Register {
    /// [`VERSION`], as the agent speaks it.
    pub protocol: u32,
    /// The machine's name, as its owner calls it ("MK3S Gonzo").
    pub name: String,
    pub kind: MachineKind,
    pub agent_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Registered {
    pub machine_id: String,
    /// For every request after this one: `Authorization: Bearer <token>`. Kept secret.
    pub token: String,
    /// Shown by the agent, entered on the website to add the machine to a space.
    pub pairing_code: String,
}

/// What a machine is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MachineState {
    /// The agent runs, but its machine is not connected (switched off, no cable).
    #[default]
    Disconnected,
    /// Ready for a job.
    Idle,
    /// Running a job.
    Busy,
    Paused,
    /// Waits for someone to do something before it can work: a label printer without a
    /// cassette, a printer out of filament. Not an error: the reason is known
    /// ([`Telemetry::problem`]). Prusa Connect's ATTENTION.
    Attention,
    Error,
}

impl MachineState {
    pub fn label(self) -> &'static str {
        match self {
            MachineState::Disconnected => "Disconnected",
            MachineState::Idle => "Idle",
            MachineState::Busy => "Busy",
            MachineState::Paused => "Paused",
            MachineState::Attention => "Attention",
            MachineState::Error => "Error",
        }
    }
}

/// A job's progress on the machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobProgress {
    pub file: String,
    /// How far, as `done` of `total` (bytes of the file, or percent).
    pub done: u64,
    pub total: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_s: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_s: Option<u64>,
}

/// An agent's report on its machine.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Telemetry {
    pub state: MachineState,
    /// The machine's firmware, once known ("Marlin 2.1.2", "Prusa-Firmware 3.14.0").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nozzle_c: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bed_c: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<JobProgress>,
    /// The temperatures it heats to; 0 when off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nozzle_target_c: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bed_target_c: Option<f32>,
    /// Whether the heater has power now (Marlin's `@:` and `B@:` above 0), where told.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nozzle_heating: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bed_heating: Option<bool>,
    /// Fan speeds, where the machine tells them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub print_fan_rpm: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotend_fan_rpm: Option<u32>,
    /// Where the tool is: X, Y, Z (Marlin's M114).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_mm: Option<[f32; 3]>,
    /// The speed factor (M220), 100 as sliced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_percent: Option<f32>,
    /// The flow factor (M221), 100 as sliced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_percent: Option<f32>,
    /// The material loaded ("PETG"), where the machine knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<String>,
    /// The job the machine runs now (its progress is [`Telemetry::job`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<i64>,
    /// A GRBL machine's settings, once read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grbl: Option<GrblSettings>,
    /// A job that ended since the last report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_end: Option<JobEnd>,
    /// Commands from the cloud carried out since the last report.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub done: Vec<CommandDone>,
    /// A bed mesh, once, when the machine has just measured or reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<Mesh>,
    /// What the machine says it is, so the cloud can pick its profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// A label printer: whether a cassette (or roll) is in. It cannot tell which: the owner
    /// says that on the website.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cassette: Option<bool>,
    /// Why the machine cannot work now, in its own words ("no cassette").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    /// A battery-powered machine's battery, in volts (a DYMO LabelManager PnP's cell).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_v: Option<f32>,
    /// The machine's own status as it answered, in hex, where it is not yet understood
    /// in full (a DYMO's eight bytes), to find out what the rest says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_status: Option<String>,
    /// The agent itself: where it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentInfo>,
}

/// The computer an agent runs on, as it tells.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentInfo {
    pub version: String,
    /// The git commit it was built from (`1d5c84c`, `-dirty` when files differed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// When it was built: `2026-11-04 14:35 UTC`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built: Option<String>,
    /// `windows`, `macos`, `linux`, and the processor: `x86_64`, `aarch64`.
    pub os: String,
    pub arch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    /// Its address on the network it reaches the cloud through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

/// What a machine says it is: M115's `MACHINE_TYPE` ("Prusa-MINI"), and on a Prusa its
/// model code (M862.2 Q: 120 for a MINI, 302 for an MK3S).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Identity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_code: Option<u32>,
    /// A Buddy-firmware Prusa's type as it tells Prusa Connect ("2.1.0" for a MINI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub printer_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryReply {
    /// Whether someone has added the machine to a space with its pairing code.
    pub paired: bool,
    /// The space it belongs to, once paired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    /// Commands for the machine, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<MachineCommand>,
    /// A file to run, when the machine has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<JobOrder>,
    /// What to do with the running job, said again each report until the machine has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_control: Option<JobControl>,
}

/// A file for the machine to run: fetched from the cloud at `download` (a path, with the
/// agent's token), then streamed to the machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobOrder {
    pub id: i64,
    pub name: String,
    pub download: String,
    pub size: u64,
    /// Start at this 1-based line of the file, not at its start: resuming a job that was
    /// interrupted (the machine rebuilds where it was with the laser off, then goes on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_line: Option<u64>,
    /// A label printer's job: the width of the tape loaded, in mm, as the owner said. The
    /// file is then a PNG, one pixel per dot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tape_mm: Option<u32>,
    /// A label printer's job on die-cut labels: how long a label is, in mm (a DYMO
    /// LabelWriter finds the gap after it). None on tape or a continuous roll.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_length_mm: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobControl {
    Pause,
    Resume,
    /// Stop now: motion stops, the laser or heaters go off.
    Stop,
}

/// How a job ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobEnd {
    pub id: i64,
    /// All of it ran.
    pub ok: bool,
    /// It was stopped on request (not ok, not a failure).
    #[serde(default)]
    pub stopped: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Where it could start again if it did not run to the end (a 1-based line of the file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_line: Option<u64>,
}

/// A GRBL controller's settings that matter before a job: read with `$$`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GrblSettings {
    /// `$32`: laser mode. Off, the beam stays on while the head stops between moves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub laser_mode: Option<bool>,
    /// `$30`: the `S` value for full power.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_power: Option<f64>,
    /// `$130`, `$131`, `$132`: how far each axis travels, mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub travel_mm: Option<[f64; 3]>,
}

/// A command for a machine: G-code lines, sent one by one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineCommand {
    pub id: u32,
    pub gcode: String,
}

/// How a command went.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandDone {
    pub id: u32,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What the machine said while carrying it out (its terminal).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output: Vec<String>,
}

/// A bed's height map, as the machine measured it: millimetres from where it expects the
/// bed, row by row from the front of the bed to the back, each row from left to right.
/// `None` where a point was not measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mesh {
    pub rows: Vec<Vec<Option<f32>>>,
}

impl Mesh {
    /// The lowest and the highest point.
    pub fn range(&self) -> Option<(f32, f32)> {
        let points = self.rows.iter().flatten().flatten().copied();
        points.fold(None, |range, z| match range {
            None => Some((z, z)),
            Some((low, high)) => Some((low.min(z), high.max(z))),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire format is plain JSON with lowercase names, so other agents (other
    /// languages, other machines) can speak it.
    #[test]
    fn messages_are_plain_json() {
        let telemetry = Telemetry {
            state: MachineState::Idle,
            firmware: None,
            nozzle_c: Some(21.5),
            bed_c: None,
            job: None,
            identity: None,
            ..Telemetry::default()
        };
        let text = serde_json::to_string(&telemetry).unwrap();
        assert_eq!(text, r#"{"state":"idle","nozzle_c":21.5}"#);
        assert_eq!(serde_json::from_str::<Telemetry>(&text).unwrap(), telemetry);
        let register: Register = serde_json::from_str(
            r#"{"protocol":1,"name":"Gonzo","kind":"fdm","agent_version":"0.1.0"}"#,
        )
        .unwrap();
        assert_eq!(register.kind, MachineKind::Fdm);
    }
}
