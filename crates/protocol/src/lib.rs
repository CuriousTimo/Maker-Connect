//! What a machine is and reports, and what the hub asks of it: the types the device engine
//! (`maker-connect-engine`) and the hub share. Plain JSON with lowercase names.

use serde::{Deserialize, Serialize};

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
    /// A scale: it reports a weight, nothing more (DYMO's M-series).
    Scale,
    Other,
    /// Not a machine: the device that carries an agent's USB cameras (`maker-connect.cameras`,
    /// `docs/design/prusa-sdk-extension.md` part 5). The hub keeps it for its token, which the
    /// cameras register with, and lists it among no machines: its cameras are in Space → Cameras.
    Cameras,
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
            MachineKind::Scale => "Scale",
            MachineKind::Other => "Machine",
            MachineKind::Cameras => "Cameras",
        }
    }
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

/// A question on a printer's screen, with its buttons in order (Marlin's host prompt:
/// `//action:prompt_begin`, `prompt_button`, `prompt_show`); answered with the button's
/// index (`M876 S<index>`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Prompt {
    pub text: String,
    pub buttons: Vec<String>,
}

/// One tool of a machine with several: its number (from 1, as on its screen), its nozzle's
/// temperature and target (0: off), and whether it is the one printing.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ToolHeat {
    pub number: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nozzle_c: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nozzle_target_c: Option<f32>,
    #[serde(default)]
    pub active: bool,
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
    /// A question on its screen with buttons, waiting for an answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<Prompt>,
    /// The chamber or enclosure, where the machine has one (Prusa's CORE One, the XL's
    /// enclosure): its temperature and what it heats or cools to (0: none set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chamber_c: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chamber_target_c: Option<f32>,
    /// Each tool, where it has more than one (the XL's heads, an INDX's tools); `nozzle_c`
    /// above is the active one's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolHeat>,
    /// Fan speeds, where the machine tells them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub print_fan_rpm: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotend_fan_rpm: Option<u32>,
    /// Where the tool is: X, Y, Z (Marlin's M114); an axis the machine has not told is `None`
    /// (a Prusa on Connect that has told only Z).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_mm: Option<[Option<f32>; 3]>,
    /// The speed factor (M220), 100 as sliced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_percent: Option<f32>,
    /// The flow factor (M221), 100 as sliced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_percent: Option<f32>,
    /// The material loaded ("PETG"), where the machine knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<String>,
    /// A Buddy printer's heads as its last Full telemetry frame wrote them: by tool id (`"0"`,
    /// `"1"`), each member's value by Buddy's name (`nozzle_temp`, `active`, ...) (Buddy 8.0.2
    /// `render.cpp:219-288`). The values above (`nozzle_c`, `tools`, the fans) are read off it.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub tool_tree: std::collections::BTreeMap<String, serde_json::Map<String, serde_json::Value>>,
    /// The job the machine runs now (its progress is [`Telemetry::job`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<i64>,
    /// A GRBL machine's settings, once read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grbl: Option<GrblSettings>,
    /// A job that ended since the last report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_end: Option<JobEnd>,
    /// Commands from the hub carried out since the last report.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub done: Vec<CommandDone>,
    /// A bed mesh, once, when the machine has just measured or reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<Mesh>,
    /// What the machine says it is, so the hub can pick its profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// A label printer: whether a cassette (or roll) is in. It cannot tell which: the owner
    /// says that on the website.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cassette: Option<bool>,
    /// Why the machine cannot work now, in its own words ("no cassette").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    /// Someone said on the printer itself that its bed is clear (Prusa's "Set Ready" in its
    /// menu, `//action:ready`): news once, like a finished command.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bed_clear: bool,
    /// Someone took that back on the printer itself (Prusa's menu, `//action:not_ready`):
    /// news once.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub not_ready: bool,
    /// Someone asked on the printer to print the last job again (Prusa's MK3: "Reprint" in
    /// its menu, `//action:start`): news once.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reprint: bool,
    /// The steel sheet chosen on the printer (Prusa's MK3: its name, "Smooth1").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
    /// How it prints: "normal" or "silent" (Prusa's MK3 mode, from its EEPROM).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub print_mode: Option<String>,
    /// Whether an SD card is in it, where it tells.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sd_card: Option<bool>,
    /// Its MMU: the slot in use (0-based), and what it is doing ("Feeding to nozzle").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmu_slot: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmu_doing: Option<String>,
    /// Its lifetime totals as it keeps them (Prusa's MK3 EEPROM): filament in metres, and
    /// minutes of printing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_filament_m: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_print_min: Option<u32>,
    /// The time left is the agent's own estimate (from how long the lines so far took), not
    /// the slicer's (PrusaLink's `inaccurate_estimates`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub time_estimated: bool,
    /// How long until the next filament change in the print, in seconds (the slicer's
    /// `M73` C/D, "Change in mins").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filament_change_s: Option<u64>,
    /// Something it does by itself that takes a while and wants no commands meanwhile
    /// ("Calibrating its thermal model").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doing: Option<String>,
    /// A battery-powered machine's charge, in percent, as the machine itself says it (a
    /// DYMO LabelManager PnP: byte 3 of its status).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_percent: Option<u8>,
    /// The machine's own status as it answered, in hex, where it is not yet understood
    /// in full (a DYMO's eight bytes), to find out what the rest says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_status: Option<String>,
    /// A label printer on DYMO's CUPS driver: the driver's own `STATE:` keyword during a job,
    /// unchanged (`com.dymo.out-of-paper-error`); none after `STATE: none`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_reason: Option<String>,
    /// A printer on OctoPrint's serial layer: its `MachineCom` state as `comm.py:445-460` names it
    /// (`STATE_OPERATIONAL`), unchanged; [`Telemetry::state`] merges some of them. The agent maps it
    /// to the protocol's state (`docs/design/prusa-sdk-extension.md`, part 9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comm_state: Option<String>,
    /// A scale's reading, as the scale says it (`ports/usbscale`, docs/archive/plans/agent-engine.md Step 3e).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<ScaleReading>,
    /// The agent itself: where it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentInfo>,
}

/// What a scale's report says, decoded as usbscale's `print_scale_data` does (HID POS 1.02 §6.2.4):
/// the scale's own status, unit and scaling exponent, and the weight in the scale's own unit (no
/// conversion to grams).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScaleReading {
    /// The status byte as the scale sent it.
    pub status: u8,
    /// usbscale's text for it ("Weighing...", "Scale reports Over Weight"), "unknown n" for a
    /// status it does not know; empty for 4, for which usbscale prints the weight instead.
    pub status_text: String,
    /// The unit byte as the scale sent it.
    pub unit: u8,
    /// usbscale's `UNITS` name ("g", "oz"), "unknown unit n" above 12.
    pub unit_name: String,
    /// The scaling exponent (a signed byte): the weight is `raw` times ten to it.
    pub exponent: i8,
    /// The weight bytes (little endian) as sent.
    pub raw: u16,
    /// `raw` times ten to `exponent`, in the scale's own unit.
    pub weight: f64,
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
    /// Its address on the network it reaches the hub through.
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
    /// Its serial number (Prusa's MK3: `PRUSA SN`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    /// The nozzle's diameter as it says it, in mm ("0.40"; Prusa's MK3: `M862.1 Q`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nozzle_mm: Option<String>,
    /// Its MMU's firmware, where one is connected ("3.0.3+896"; Prusa's MK3: `M707`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmu_firmware: Option<String>,
    /// A Marlin printer's `M115` answer, its first line's keys and values, as OctoPrint keeps it
    /// (`Printer.firmware_info["data"]`; extension part 9, `firmware_info`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware_info: Option<serde_json::Map<String, serde_json::Value>>,
    /// Its `Cap:` lines, as OctoPrint keeps them (`_firmware_capabilities`; part 9,
    /// `firmware_capabilities`), once the report ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware_capabilities: Option<serde_json::Map<String, serde_json::Value>>,
}

/// A file for the machine to run (`download`: where the file is), streamed to the machine.
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
    /// A LabelWriter's job: the PPD's `PageSize` choice of the roll loaded (`w79h252.2`), as CUPS's
    /// `-o PageSize=<name>`; the port marks exactly that page (W2b). None: the length chooses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<String>,
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

/// What the hub asks of a machine, typed: the hub makes no G-code; the machine's port makes the
/// bytes, as its reference does (owner, 2026-11-12). Exactly the controls the hub's pages offer
/// today (owner's approval O8), nothing added. A label is printed as a job ([`JobOrder`] with
/// `tape_mm`, `label_length_mm`), and pause, resume and stop are [`JobControl`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// G-code a user typed in: passed through as typed.
    Gcode {
        text: String,
    },
    /// The jog pad: move by so many mm (relative, its step and sign), at the feed the page chose
    /// from its list, or (none chosen) the port's own as its reference has it.
    Jog {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        y: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        z: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feed: Option<f64>,
    },
    /// Home these axes, as OctoPrint's API takes them (`home: ["x", "y"]`, `server/api/printer.py:297`):
    /// OctoPrint's panel has one button for X and Y and one for Z; the page's one button of other
    /// machines is all three.
    Home {
        axes: Vec<Axis>,
    },
    /// Extrude (positive) or retract (negative) so many mm.
    Extrude {
        mm: f64,
    },
    /// A heater's target (0: off). Preheat and cool down are this.
    Temperature {
        heater: Heater,
        celsius: f64,
    },
    /// The part-cooling fan on or off.
    Fan {
        on: bool,
    },
    /// Speed and flow factors, in percent.
    Speed {
        percent: u16,
    },
    Flow {
        percent: u16,
    },
    MotorsOff,
    /// Load or unload filament; the loaded spool's temperatures where the hub knows them.
    LoadFilament {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nozzle_c: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bed_c: Option<f32>,
    },
    UnloadFilament {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nozzle_c: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bed_c: Option<f32>,
    },
    /// Measure the bed (its mesh).
    ProbeBed,
    /// The machine's own card: list it, print a file of it, delete one.
    SdList,
    SdPrint {
        file: String,
    },
    SdDelete {
        file: String,
    },
    /// A laser: unlock after an alarm, set and go to the work origin, read settings.
    Unlock,
    SetOrigin,
    GoOrigin,
    ReadSettings,
    /// A laser: run a file's outline with the beam off; the file as a job's (`download`).
    Frame {
        download: String,
    },
}

/// An axis to home, spelled as OctoPrint's `valid_axes` of the printhead API (`"x"`, `"y"`, `"z"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    X,
    Y,
    Z,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Heater {
    Nozzle,
    Bed,
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

    /// Plain JSON with lowercase names.
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
    }

    /// A scale's reading is plain JSON, and a scale is a kind of machine.
    #[test]
    fn a_scale_reading_is_plain_json() {
        let telemetry = Telemetry {
            state: MachineState::Idle,
            scale: Some(ScaleReading {
                status: 4,
                status_text: String::new(),
                unit: 2,
                unit_name: "g".into(),
                exponent: -1,
                raw: 1234,
                weight: 123.4,
            }),
            ..Telemetry::default()
        };
        let text = serde_json::to_string(&telemetry).unwrap();
        assert_eq!(
            text,
            r#"{"state":"idle","scale":{"status":4,"status_text":"","unit":2,"unit_name":"g","exponent":-1,"raw":1234,"weight":123.4}}"#
        );
        assert_eq!(serde_json::from_str::<Telemetry>(&text).unwrap(), telemetry);
        assert_eq!(MachineKind::Scale.label(), "Scale");
        assert_eq!(
            serde_json::from_str::<MachineKind>("\"scale\"").unwrap(),
            MachineKind::Scale
        );
    }

    /// B5: a home names its axes, spelled as OctoPrint's printhead API has them.
    #[test]
    fn a_home_names_its_axes() {
        let home = Action::Home {
            axes: vec![Axis::X, Axis::Y, Axis::Z],
        };
        let text = serde_json::to_string(&home).unwrap();
        assert_eq!(text, r#"{"action":"home","axes":["x","y","z"]}"#);
        assert_eq!(serde_json::from_str::<Action>(&text).unwrap(), home);
    }
}

/// W2b (proxy, 2026-11-17): `page_size` is optional on the wire both ways.
#[cfg(test)]
mod job_order_page_size {
    use super::JobOrder;

    #[test]
    fn with_and_without_page_size() {
        let without = r#"{"id":1,"name":"a.png","download":"/f","size":3,"label_length_mm":89}"#;
        let order: JobOrder = serde_json::from_str(without).unwrap();
        assert_eq!(order.page_size, None);
        assert!(!serde_json::to_string(&order).unwrap().contains("page_size"));
        let with = r#"{"id":1,"name":"a.png","download":"/f","size":3,"label_length_mm":89,"page_size":"w79h252.2"}"#;
        let order: JobOrder = serde_json::from_str(with).unwrap();
        assert_eq!(order.page_size.as_deref(), Some("w79h252.2"));
        let back: JobOrder = serde_json::from_str(&serde_json::to_string(&order).unwrap()).unwrap();
        assert_eq!(back.page_size.as_deref(), Some("w79h252.2"));
    }
}
