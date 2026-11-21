//! The registries (hub-core-v2.md §2.10): one Rust table each, the single source. The docs in
//! `docs/registries/` are generated from them ([`render_docs`]) and a test fails when they drift
//! (`tests/registry_docs.rs`; `MC_WRITE_REGISTRIES=1` writes them).
//!
//! The verbs and state sets are the rows of `docs/archive/plans/hub-core-v2-device-map.md` (GitHub #14),
//! with the owner's answers of 2026-11-18 applied: each row names its reference's own call or state
//! (file:line; `OP/` = `reference/OctoPrint-1.11.8/src/octoprint/`, `LF/` = LaserFoundry v0.5.0
//! `61febdb`, read with `git show v0.5.0:<path>`). A row the map marks "not checked" is not here.
//! Argument bounds are per device, in its capability set (the adapter's baseline), never here.
//!
//! Actions are the names of hub-core-v2.md §2.6; which verb needs which and the presets are step 3's
//! (authz): until then no action is in a preset, and only the two the owner named damaging are.
//!
//! Test fixture rows (every name starts with `fixture`) exist only under `cfg(test)`.

use crate::capability::{Kind, Unit};
use crate::fault::{self, FaultCode};
use crate::state::{StateDecl, StateSet};

/// A capability verb: what it is called, its arguments' kinds and units (bounds are per device,
/// in its capability set), the action a caller needs, and the device kinds that may declare it.
#[derive(Debug)]
pub struct VerbEntry {
    pub name: &'static str,
    pub args: &'static [ArgEntry],
    /// An [`ActionEntry`] name.
    pub action: &'static str,
    /// Accepted only from a person's own session, never from the queue or a token (the laser
    /// rule, hub-core-v2.md §2.1). The host enforces it; a driver cannot turn it off.
    pub person_only: bool,
    /// [`StateSet`] kinds.
    pub kinds: &'static [&'static str],
    pub doc: &'static str,
}

#[derive(Debug)]
pub struct ArgEntry {
    pub name: &'static str,
    pub kind: Kind,
    pub unit: Option<Unit>,
    pub doc: &'static str,
}

/// An event type: its name, what its `data` holds, and the action needed to see it.
#[derive(Debug)]
pub struct EventTypeEntry {
    pub name: &'static str,
    pub data: &'static str,
    /// An [`ActionEntry`] name.
    pub visible_with: &'static str,
    pub doc: &'static str,
}

/// An action within a Space: the presets that include it, and whether it can damage something
/// (damaging actions are granted by name, never by a preset).
#[derive(Debug)]
pub struct ActionEntry {
    pub name: &'static str,
    pub presets: &'static [&'static str],
    pub damaging: bool,
    pub doc: &'static str,
}

/// A metric: its name and unit.
#[derive(Debug)]
pub struct MetricEntry {
    pub name: &'static str,
    pub unit: Unit,
    pub doc: &'static str,
}

// ---- Device kinds (one state set each) ----

/// A plain Marlin printer through OctoPrint 1.11.8 (`--mode marlin`).
pub const KIND_MARLIN: &str = "marlin";
/// A DYMO LabelManager PnP through labelle (`--mode dymo-pnp`).
pub const KIND_LABELMANAGER: &str = "labelmanager";
/// A DYMO LabelWriter 450 or 320 through DYMO's CUPS driver 1.4.0.
pub const KIND_LABELWRITER: &str = "labelwriter";
/// A GRBL laser through LaserFoundry v0.5.0 (`--mode grbl`).
pub const KIND_GRBL: &str = "grbl";
/// A DYMO M-series scale through usbscale (`--mode dymo-scale`).
pub const KIND_SCALE: &str = "scale";

// ---- Verb names ----

pub const GCODE: &str = "mc.printer.gcode";
pub const HOME: &str = "core.command.home";
pub const MOVE: &str = "core.command.move";
pub const MOVE_Z: &str = "core.command.move_z";
pub const MOVE_E: &str = "core.command.move_e";
pub const SET_NOZZLE: &str = "core.command.set_nozzle_temperature";
pub const SET_BED: &str = "core.command.set_bed_temperature";
pub const SET_SPEED: &str = "core.command.set_speed";
pub const SET_FLOW: &str = "core.command.set_flow";
pub const DISABLE_STEPPERS: &str = "core.command.disable_steppers";
pub const FAN: &str = "mc.printer.fan";
pub const START_PRINT: &str = "core.command.start_print";
pub const PAUSE_PRINT: &str = "core.command.pause_print";
pub const RESUME_PRINT: &str = "core.command.resume_print";
pub const STOP_PRINT: &str = "core.command.stop_print";
pub const SD_LIST: &str = "mc.printer.sd_list";
pub const DELETE_FILE: &str = "core.command.delete_file";

pub const LABEL_PRINT: &str = "mc.label.print";

pub const LASER_GCODE: &str = "mc.laser.gcode";
pub const LASER_JOG: &str = "mc.laser.jog";
pub const LASER_HOME: &str = "mc.laser.home";
pub const LASER_UNLOCK: &str = "mc.laser.unlock";
pub const LASER_SET_ORIGIN: &str = "mc.laser.set_origin";
pub const LASER_GO_ORIGIN: &str = "mc.laser.go_origin";
pub const LASER_READ_SETTINGS: &str = "mc.laser.read_settings";
pub const LASER_FRAME: &str = "mc.laser.frame";
pub const LASER_START: &str = "mc.laser.start";
pub const LASER_PAUSE: &str = "mc.laser.pause";
pub const LASER_RESUME: &str = "mc.laser.resume";
pub const LASER_STOP: &str = "mc.laser.stop";

// ---- Action names (hub-core-v2.md §2.6) ----

pub const ACT_VIEW: &str = "device.view";
pub const ACT_MOVE: &str = "device.move";
pub const ACT_HEAT: &str = "device.heat";
pub const ACT_GCODE: &str = "device.gcode";
pub const ACT_JOB_START: &str = "job.start";
pub const ACT_JOB_CONTROL: &str = "job.control";
pub const ACT_LASER_START: &str = "mc.laser.start";

const fn arg(name: &'static str, kind: Kind, unit: Option<Unit>, doc: &'static str) -> ArgEntry {
    ArgEntry {
        name,
        kind,
        unit,
        doc,
    }
}

const fn row(
    name: &'static str,
    args: &'static [ArgEntry],
    action: &'static str,
    kinds: &'static [&'static str],
    doc: &'static str,
) -> VerbEntry {
    VerbEntry {
        name,
        args,
        action,
        person_only: false,
        kinds,
        doc,
    }
}

const MM: Option<Unit> = Some(Unit::Millimeter);
const MM_MIN: Option<Unit> = Some(Unit::MillimeterPerMinute);
const CELSIUS: Option<Unit> = Some(Unit::Celsius);
const PERCENT: Option<Unit> = Some(Unit::Percent);
const MARLIN: &[&str] = &[KIND_MARLIN];
const LASER: &[&str] = &[KIND_GRBL];

pub static CAPABILITY_VERBS: &[VerbEntry] = &[
    // ---- Marlin through OctoPrint 1.11.8 (device map §1.1) ----
    // OP/printer/standard.py:463 `Printer.commands`; endpoint OP/server/api/printer.py:393-443
    row(
        GCODE,
        &[arg(
            "text",
            Kind::String,
            None,
            "The G-code as typed; each non-empty line is one command (K1).",
        )],
        ACT_GCODE,
        MARLIN,
        "OctoPrint's `printer/command` with `commands`.",
    ),
    // OP/printer/standard.py:553 `Printer.home`; endpoint OP/server/api/printer.py:327-336
    row(
        HOME,
        &[arg(
            "axes",
            Kind::String,
            None,
            "OctoPrint's list of axes, from `x`, `y`, `z` (printer.py:308,331), comma-separated.",
        )],
        ACT_MOVE,
        MARLIN,
        "OctoPrint's `printer/printhead` `home`.",
    ),
    // OP/printer/standard.py:510 `Printer.jog` (relative); endpoint OP/server/api/printer.py:310-324
    row(
        MOVE,
        &[
            arg(
                "x",
                Kind::Number,
                MM,
                "Relative; the sign is the direction.",
            ),
            arg(
                "y",
                Kind::Number,
                MM,
                "Relative; the sign is the direction.",
            ),
            arg(
                "feedrate",
                Kind::Number,
                MM_MIN,
                "Optional; without it OctoPrint takes the profile's slowest axis (standard.py:541).",
            ),
        ],
        ACT_MOVE,
        MARLIN,
        "OctoPrint's `printer/printhead` `jog`, X and Y.",
    ),
    // the same `jog` call, axis z (OP/server/api/printer.py:308,313-318)
    row(
        MOVE_Z,
        &[
            arg(
                "distance",
                Kind::Number,
                MM,
                "Relative; the sign is the direction.",
            ),
            arg(
                "feedrate",
                Kind::Number,
                MM_MIN,
                "Optional, as for `core.command.move`.",
            ),
        ],
        ACT_MOVE,
        MARLIN,
        "OctoPrint's `printer/printhead` `jog`, Z.",
    ),
    // OP/printer/standard.py:575 `Printer.extrude`; endpoint OP/server/api/printer.py:133-142
    row(
        MOVE_E,
        &[
            arg(
                "length",
                Kind::Number,
                MM,
                "Negative retracts (printer.py:138-141).",
            ),
            arg(
                "feedrate",
                Kind::Number,
                MM_MIN,
                "Optional; OctoPrint caps it at the profile's E speed (standard.py:582-589).",
            ),
        ],
        ACT_MOVE,
        MARLIN,
        "OctoPrint's `printer/tool` `extrude`.",
    ),
    // OP/printer/standard.py:606 `Printer.set_temperature`; endpoint OP/server/api/printer.py:100-114
    row(
        SET_NOZZLE,
        &[
            arg(
                "nozzle_temperature",
                Kind::Number,
                CELSIUS,
                "Minimum 0 (standard.py:614); OctoPrint has no maximum.",
            ),
            arg(
                "tool",
                Kind::String,
                None,
                "OctoPrint's `toolN` (printer.py:86,105-107).",
            ),
        ],
        ACT_HEAT,
        MARLIN,
        "OctoPrint's `printer/tool` `target`.",
    ),
    // the same call, heater `bed` (OP/printer/standard.py:633-634); endpoint OP/server/api/printer.py:187-195
    row(
        SET_BED,
        &[arg(
            "bed_temperature",
            Kind::Number,
            CELSIUS,
            "Minimum 0 (standard.py:614); OctoPrint has no maximum.",
        )],
        ACT_HEAT,
        MARLIN,
        "OctoPrint's `printer/bed` `target`; refused without a heated bed (printer.py:176-177).",
    ),
    // OP/printer/standard.py:682 `Printer.feed_rate` (M220); endpoint OP/server/api/printer.py:338-345
    row(
        SET_SPEED,
        &[arg(
            "speed",
            Kind::Integer,
            PERCENT,
            "OctoPrint's integer factor, minimum 1 (standard.py:668-681).",
        )],
        ACT_JOB_CONTROL,
        MARLIN,
        "OctoPrint's `printer/printhead` `feedrate`.",
    ),
    // OP/printer/standard.py:689 `Printer.flow_rate` (M221); endpoint OP/server/api/printer.py:144-151
    row(
        SET_FLOW,
        &[arg(
            "flow",
            Kind::Integer,
            PERCENT,
            "OctoPrint's integer factor, minimum 1 (standard.py:668-681).",
        )],
        ACT_JOB_CONTROL,
        MARLIN,
        "OctoPrint's `printer/tool` `flowrate`.",
    ),
    // OP/templates/tabs/control.jinja2:82 "Motors off": `M18` through `printer/command` (owner, 2026-11-18)
    row(
        DISABLE_STEPPERS,
        &[],
        ACT_MOVE,
        MARLIN,
        "OctoPrint's control panel \"Motors off\" button: `M18`.",
    ),
    // OP/templates/tabs/control.jinja2:83-84 "Fan on"/"Fan off": `M106 S255`/`M106 S0` (owner, 2026-11-18)
    row(
        FAN,
        &[arg(
            "on",
            Kind::Boolean,
            None,
            "`true` is \"Fan on\" (`M106 S255`), `false` \"Fan off\" (`M106 S0`).",
        )],
        ACT_HEAT,
        MARLIN,
        "OctoPrint's control panel fan buttons.",
    ),
    // OP/printer/standard.py:696 `select_file`, :765 `start_print`; endpoints OP/server/api/files.py:845-905, job.py:32-38
    row(
        START_PRINT,
        &[
            arg(
                "path",
                Kind::String,
                None,
                "The file: a job file (origin `local`) or a name on the card (origin `sd`).",
            ),
            arg(
                "origin",
                Kind::String,
                None,
                "`local` or `sd` (ours: OctoPrint separates them by URL, files.py:845).",
            ),
        ],
        ACT_JOB_START,
        MARLIN,
        "OctoPrint selects the file and prints it; from the card with `printAfterSelect` (files.py:156-172, K7).",
    ),
    // OP/printer/standard.py:792 `Printer.pause_print`; endpoint OP/server/api/job.py:46-60
    row(
        PAUSE_PRINT,
        &[],
        ACT_JOB_CONTROL,
        MARLIN,
        "OctoPrint's `job` `pause`.",
    ),
    // OP/printer/standard.py:808 `Printer.resume_print`; endpoint OP/server/api/job.py:57-58
    row(
        RESUME_PRINT,
        &[],
        ACT_JOB_CONTROL,
        MARLIN,
        "OctoPrint's `job` `pause` with `resume`.",
    ),
    // OP/printer/standard.py:824 `Printer.cancel_print`; endpoint OP/server/api/job.py:61-67
    row(
        STOP_PRINT,
        &[],
        ACT_JOB_CONTROL,
        MARLIN,
        "OctoPrint's `job` `cancel`.",
    ),
    // OP/printer/standard.py:1074 `Printer.refresh_sd_files`; endpoint OP/server/api/printer.py:372-373
    row(
        SD_LIST,
        &[],
        ACT_VIEW,
        MARLIN,
        "OctoPrint's `printer/sd` `refresh`; the card's file names come back as the command's output.",
    ),
    // OP/printer/standard.py:1052 `Printer.delete_sd_file`; endpoint OP/server/api/files.py:1313-1343
    row(
        DELETE_FILE,
        &[
            arg(
                "path",
                Kind::String,
                None,
                "A file the card listed (files.py:505-507).",
            ),
            arg(
                "origin",
                Kind::String,
                None,
                "`sd` (ours: OctoPrint separates origins by URL, files.py:845).",
            ),
        ],
        ACT_JOB_CONTROL,
        MARLIN,
        "OctoPrint's `files/sdcard/<path>` DELETE; refused for the file in use (files.py:1329-1330).",
    ),
    // ---- Label printers (device map §2.1, §3.1) ----
    // labelle: `labelle --picture FILE --tape-size-mm N` cli/cli.py:108,224,590,613 -> DymoLabeler.print
    //   lib/devices/dymo_labeler.py:307; DYMO CUPS 1.4.0: `raster2dymolw` common/CupsFilter.h:69-99,
    //   lw/raster2dymolw.cpp:56
    VerbEntry {
        name: LABEL_PRINT,
        args: &[
            arg(
                "picture",
                Kind::String,
                None,
                "The job file: a PNG, one pixel per dot.",
            ),
            arg(
                "tape_size_mm",
                Kind::Integer,
                MM,
                "labelle: 19, 12, 9 or 6, default 12 (dymo_labeler.py:243-257).",
            ),
            arg(
                "page_size",
                Kind::String,
                None,
                "DYMO CUPS: a `PageSize` choice of the PPD, default its `DefaultPageSize` (lw450.ppd:147-149).",
            ),
        ],
        action: ACT_JOB_START,
        person_only: false,
        kinds: &[KIND_LABELMANAGER, KIND_LABELWRITER],
        doc: "Prints one label as the reference's own print run does.",
    },
    // ---- GRBL lasers through LaserFoundry v0.5.0 (device map §4.1) ----
    // LF/crates/device/src/link.rs:161 `Link::send_line`; app panels.rs:865
    row(
        LASER_GCODE,
        &[arg(
            "line",
            Kind::String,
            None,
            "One line, at most 79 bytes (stream.rs:10, link.rs:166).",
        )],
        ACT_GCODE,
        LASER,
        "LaserFoundry's console line; refused while a job runs (link.rs:169,277).",
    ),
    // LF/crates/device/src/link.rs:197 `Link::jog`; app panels.rs:813
    row(
        LASER_JOG,
        &[
            arg(
                "x",
                Kind::Number,
                MM,
                "Relative, |x| <= 1000 (link.rs:26, 306-310).",
            ),
            arg(
                "y",
                Kind::Number,
                MM,
                "Relative, |y| <= 1000 (link.rs:26, 306-310).",
            ),
            arg("feed", Kind::Number, MM_MIN, "1 to 30000 (link.rs:27)."),
        ],
        ACT_MOVE,
        LASER,
        "LaserFoundry's jog: `$J=G91 G21 X.. Y.. F..`.",
    ),
    // LF/crates/device/src/link.rs:222 `Link::home` (`$H`); control_box.rs:158, panels.rs:838
    row(
        LASER_HOME,
        &[],
        ACT_MOVE,
        LASER,
        "LaserFoundry's Home: `$H`, one homing cycle.",
    ),
    // LF/crates/device/src/link.rs:230 `Link::unlock` (`$X`); control_box.rs:160-161, panels.rs:841
    row(
        LASER_UNLOCK,
        &[],
        ACT_MOVE,
        LASER,
        "LaserFoundry's Unlock: `$X`.",
    ),
    // LF/crates/device/src/link.rs:238 `Link::set_zero` (`G92 X0 Y0`); control_box.rs:164
    row(
        LASER_SET_ORIGIN,
        &[],
        ACT_MOVE,
        LASER,
        "LaserFoundry's Set origin: `G92 X0 Y0`.",
    ),
    // LF/crates/device/src/link.rs:243 `Link::go_to_zero` (`G90 G0 X0 Y0`); control_box.rs:167
    row(
        LASER_GO_ORIGIN,
        &[],
        ACT_MOVE,
        LASER,
        "LaserFoundry's Go to origin: `G90 G0 X0 Y0`.",
    ),
    // LF/crates/device/src/link.rs:184 `Link::request_settings` (`$$`); settings_ui.rs:60
    row(
        LASER_READ_SETTINGS,
        &[],
        ACT_VIEW,
        LASER,
        "LaserFoundry's settings read: `$$`.",
    ),
    // LF app `frame_action` panels.rs:734-740 -> `job.frame` -> LF/crates/gcode/src/lib.rs:92 `gcode::frame`
    //   (the design's bounds; owner, 2026-11-18: no fixed 3000 mm/min, no file-extent frame)
    row(
        LASER_FRAME,
        &[arg(
            "power",
            Kind::Number,
            PERCENT,
            "0 to 100 (panels.rs:778), default 0: the beam off (lib.rs:416).",
        )],
        ACT_MOVE,
        LASER,
        "LaserFoundry's Frame: `gcode::frame` over the design's bounds, run as a job.",
    ),
    // LF/crates/ui-egui/src/lib.rs:1221-1251 `start_job_with` -> LF/crates/device/src/link.rs:250 `Link::start_job`
    VerbEntry {
        name: LASER_START,
        args: &[
            arg("program", Kind::String, None, "The job file."),
            arg(
                "from_line",
                Kind::Integer,
                None,
                "1-based line to start from (`start_from_line`, lib.rs:1258-1266).",
            ),
        ],
        action: ACT_LASER_START,
        person_only: true,
        kinds: LASER,
        doc: "LaserFoundry's Start. A person's own session only (hub-core-v2.md §2.1).",
    },
    // LF/crates/device/src/link.rs:255 `Link::pause`; control_box.rs:144, panels.rs:772
    row(
        LASER_PAUSE,
        &[],
        ACT_JOB_CONTROL,
        LASER,
        "LaserFoundry's Pause.",
    ),
    // LF/crates/device/src/link.rs:259 `Link::resume`; control_box.rs:141, panels.rs:769
    row(
        LASER_RESUME,
        &[],
        ACT_JOB_CONTROL,
        LASER,
        "LaserFoundry's Resume.",
    ),
    // LF/crates/device/src/link.rs:265 `Link::stop` (soft reset, laser off); control_box.rs:147-149
    row(
        LASER_STOP,
        &[],
        ACT_JOB_CONTROL,
        LASER,
        "LaserFoundry's Stop: a GRBL soft reset, the laser off.",
    ),
];

const fn state(
    name: &'static str,
    accepts: &'static [&'static str],
    doc: &'static str,
) -> StateDecl {
    StateDecl { name, accepts, doc }
}

/// What every OPERATIONAL state of OctoPrint accepts (device map §1.3).
macro_rules! marlin_operational {
    ($($more:expr),*) => {
        &[GCODE, SET_NOZZLE, SET_BED, SET_SPEED, SET_FLOW, FAN, DELETE_FILE $(, $more)*]
    };
}

/// Every LaserFoundry verb but Start: `Link` refuses them only while a job runs (link.rs:277), and
/// the app's state rules are about the job, not the machine state (device map §4.2).
macro_rules! laser_any_state {
    ($($more:expr),*) => {
        &[LASER_GCODE, LASER_JOG, LASER_HOME, LASER_UNLOCK, LASER_SET_ORIGIN, LASER_GO_ORIGIN,
          LASER_READ_SETTINGS, LASER_FRAME, LASER_PAUSE, LASER_RESUME, LASER_STOP $(, $more)*]
    };
}

pub static STATE_SETS: &[StateSet] = &[
    // OP/util/comm.py:445-460 `MachineCom.STATE_*` (owner, 2026-11-18: the 16 as they are); texts
    // `getStateString` comm.py:965-1014; groups OPERATIONAL_STATES comm.py:463-473, PRINTING_STATES
    // comm.py:476-483; which verb each accepts: the endpoints' refusals (device map §1.3)
    StateSet {
        kind: KIND_MARLIN,
        states: &[
            state("NONE", &[], "STATE_NONE 0, \"Offline\"."),
            state(
                "OPEN_SERIAL",
                &[],
                "STATE_OPEN_SERIAL 1, \"Opening serial connection\".",
            ),
            state(
                "DETECT_SERIAL",
                &[],
                "STATE_DETECT_SERIAL 2, \"Detecting serial connection\".",
            ),
            state("CONNECTING", &[], "STATE_CONNECTING 3, \"Connecting\"."),
            state(
                "OPERATIONAL",
                marlin_operational!(
                    HOME,
                    MOVE,
                    MOVE_Z,
                    MOVE_E,
                    DISABLE_STEPPERS,
                    START_PRINT,
                    SD_LIST
                ),
                "STATE_OPERATIONAL 4, \"Operational\".",
            ),
            state(
                "STARTING",
                marlin_operational!(PAUSE_PRINT, STOP_PRINT),
                "STATE_STARTING 5, \"Starting\".",
            ),
            state(
                "PRINTING",
                marlin_operational!(PAUSE_PRINT, STOP_PRINT),
                "STATE_PRINTING 6, \"Printing\".",
            ),
            state(
                "PAUSED",
                marlin_operational!(
                    HOME,
                    MOVE,
                    MOVE_Z,
                    MOVE_E,
                    DISABLE_STEPPERS,
                    PAUSE_PRINT,
                    RESUME_PRINT,
                    STOP_PRINT
                ),
                "STATE_PAUSED 7, \"Paused\".",
            ),
            state(
                "PAUSING",
                marlin_operational!(PAUSE_PRINT, STOP_PRINT),
                "STATE_PAUSING 8, \"Pausing\".",
            ),
            state(
                "RESUMING",
                marlin_operational!(PAUSE_PRINT, STOP_PRINT),
                "STATE_RESUMING 9, \"Resuming\".",
            ),
            state(
                "FINISHING",
                marlin_operational!(PAUSE_PRINT, STOP_PRINT),
                "STATE_FINISHING 10, \"Finishing\".",
            ),
            state("CLOSED", &[], "STATE_CLOSED 11, \"Offline\"."),
            state("ERROR", &[], "STATE_ERROR 12, \"Error\"."),
            state(
                "CLOSED_WITH_ERROR",
                &[],
                "STATE_CLOSED_WITH_ERROR 13, \"Offline after error\".",
            ),
            state(
                "TRANSFERING_FILE",
                marlin_operational!(HOME, MOVE, MOVE_Z, MOVE_E, DISABLE_STEPPERS, SD_LIST),
                "STATE_TRANSFERING_FILE 14, \"Transferring file to SD\".",
            ),
            state(
                "CANCELLING",
                marlin_operational!(PAUSE_PRINT, STOP_PRINT),
                "STATE_CANCELLING 15, \"Cancelling\".",
            ),
        ],
        transitions: &[],
    },
    // labelle has no state set (device map §2.2): these three are ours (L1, L7, L10,
    // crates/engine/src/ports/labelle/port.rs:1-11, 133-138)
    StateSet {
        kind: KIND_LABELMANAGER,
        states: &[
            state("DISCONNECTED", &[], "Ours: no LabelManager found."),
            state(
                "IDLE",
                &[LABEL_PRINT],
                "Ours: a LabelManager is there (labelle's `is_ready`, dymo_labeler.py:304).",
            ),
            state("BUSY", &[], "Ours: a label prints."),
        ],
        transitions: &[],
    },
    // DYMO CUPS 1.4.0 `job_status_t` common/PrinterDriver.h:54-66; the 450 code sets jsOK, jsPaperOut,
    // jsError (lw/LabelWriterLanguageMonitor.cpp:261-271). IDLE and PRINTING are ours (port.rs:7, 245).
    StateSet {
        kind: KIND_LABELWRITER,
        states: &[
            state(
                "IDLE",
                &[LABEL_PRINT],
                "Ours: no job runs; nothing is sent while idle.",
            ),
            state("PRINTING", &[], "Ours: a job runs (`jsOK`)."),
            state(
                "OUT_OF_PAPER",
                &[],
                "`jsPaperOut`, `com.dymo.out-of-paper-error`.",
            ),
            state("ERROR", &[], "`jsError`, `com.dymo.general-error`."),
        ],
        transitions: &[],
    },
    // LF/crates/device/src/status.rs:3-14 `MachineState` at v0.5.0 (the map's :35-46 is not where it is),
    // from the `?` status report. Start only in
    // Idle or Unknown (`not_ready` LF/crates/ui-egui/src/lib.rs:1520-1532).
    StateSet {
        kind: KIND_GRBL,
        states: &[
            state("IDLE", laser_any_state!(LASER_START), "GRBL `Idle`."),
            state("RUN", laser_any_state!(), "GRBL `Run`."),
            state("HOLD", laser_any_state!(), "GRBL `Hold`."),
            state("JOG", laser_any_state!(), "GRBL `Jog`."),
            state("ALARM", laser_any_state!(), "GRBL `Alarm`."),
            state("DOOR", laser_any_state!(), "GRBL `Door`."),
            state("CHECK", laser_any_state!(), "GRBL `Check`."),
            state("HOME", laser_any_state!(), "GRBL `Home`."),
            state("SLEEP", laser_any_state!(), "GRBL `Sleep`."),
            state(
                "UNKNOWN",
                laser_any_state!(LASER_START),
                "A status LaserFoundry does not know (`Unknown`).",
            ),
        ],
        transitions: &[],
    },
    // usbscale 9ed1cb5 usbscale.c:406-452: the status byte of the weigh report; no verbs
    StateSet {
        kind: KIND_SCALE,
        states: &[
            state("FAULT", &[], "0x01 \"Fault\" (usbscale.c:407-409)."),
            state(
                "ZEROED",
                &[],
                "0x02 \"Scale is zero'd\" (usbscale.c:410-413).",
            ),
            state("WEIGHING", &[], "0x03 \"Weighing\" (usbscale.c:414-417)."),
            state(
                "STABLE",
                &[],
                "0x04: the weight, \"the only final, successful status\" (usbscale.c:419-431).",
            ),
            state(
                "UNDER_ZERO",
                &[],
                "0x05 \"Under Zero\" (usbscale.c:432-435).",
            ),
            state(
                "OVER_WEIGHT",
                &[],
                "0x06 \"Over Weight\" (usbscale.c:436-439).",
            ),
            state(
                "CALIBRATION_NEEDED",
                &[],
                "0x07 \"Calibration Needed\" (usbscale.c:440-443).",
            ),
            state(
                "REZEROING_NEEDED",
                &[],
                "0x08 \"Re-zeroing Needed\" (usbscale.c:444-447).",
            ),
            state(
                "UNKNOWN",
                &[],
                "Any other byte, \"Unknown status code\" (usbscale.c:448-451).",
            ),
        ],
        transitions: &[],
    },
];

pub static EVENT_TYPES: &[EventTypeEntry] = &[];

const fn action_row(name: &'static str, damaging: bool, doc: &'static str) -> ActionEntry {
    ActionEntry {
        name,
        presets: &[],
        damaging,
        doc,
    }
}

pub static ACTIONS: &[ActionEntry] = &[
    action_row(ACT_VIEW, false, "See a device and read from it."),
    action_row(ACT_MOVE, false, "Move a device's axes, home, motors off."),
    action_row(ACT_HEAT, false, "Set heaters and fans."),
    action_row(
        ACT_GCODE,
        true,
        "Send raw G-code (owner, 2026-11-18: granted by name).",
    ),
    action_row(ACT_JOB_START, false, "Start a job."),
    action_row(
        ACT_JOB_CONTROL,
        false,
        "Pause, resume or stop a job; change its speed or flow; delete a file.",
    ),
    action_row(
        ACT_LASER_START,
        true,
        "Start a laser job (owner, 2026-11-18: granted by name).",
    ),
];

pub static FAULT_CODES: &[&FaultCode] = &[
    &fault::CAPABILITY_NOT_SUPPORTED,
    &fault::ARGUMENT_UNKNOWN,
    &fault::ARGUMENT_MISSING,
    &fault::ARGUMENT_NOT_SUPPORTED,
    &fault::ARGUMENT_MALFORMED,
    &fault::ARGUMENT_OUT_OF_RANGE,
    &fault::STATE_NOT_ACCEPTED,
    &fault::PERSON_ONLY,
];

/// What the history keeps of a machine's telemetry (crate hub `store::history`). The names are the
/// Prusa Connect SDK's telemetry keys (`reference/Prusa-Connect-SDK-Printer`, `Printer.telemetry`).
/// The order is the `metric` table's ids, from 1 (hub migration `20261120000000_history.sql`):
/// append only.
pub static METRICS: &[MetricEntry] = &[
    MetricEntry {
        name: "temp_nozzle",
        unit: Unit::Celsius,
        doc: "The nozzle's temperature.",
    },
    MetricEntry {
        name: "target_nozzle",
        unit: Unit::Celsius,
        doc: "The nozzle's target temperature.",
    },
    MetricEntry {
        name: "temp_bed",
        unit: Unit::Celsius,
        doc: "The bed's temperature.",
    },
    MetricEntry {
        name: "target_bed",
        unit: Unit::Celsius,
        doc: "The bed's target temperature.",
    },
    MetricEntry {
        name: "fan_print",
        unit: Unit::Rpm,
        doc: "The print fan's speed.",
    },
    MetricEntry {
        name: "fan_hotend",
        unit: Unit::Rpm,
        doc: "The hotend fan's speed.",
    },
];

/// The test fixture rows, only under `cfg(test)`.
#[cfg(test)]
mod fx {
    use super::*;
    use crate::state::Transition;

    pub static VERBS: &[VerbEntry] = &[
        VerbEntry {
            name: "fixture.echo",
            args: &[
                arg("text", Kind::String, None, "Text to echo."),
                arg("tone", Kind::String, None, "A choice of values."),
            ],
            action: "fixture.operate",
            person_only: false,
            kinds: &["fixture"],
            doc: "Test fixture: answers ok, or refuses when text is `refuse`.",
        },
        VerbEntry {
            name: "fixture.move",
            args: &[
                arg("distance", Kind::Number, MM, "A bounded number."),
                arg("speed", Kind::Integer, MM_MIN, "An optional integer."),
            ],
            action: "fixture.operate",
            person_only: false,
            kinds: &["fixture"],
            doc: "Test fixture: moves nothing; sets `/position` in its state tree.",
        },
        row(
            "fixture.start",
            &[],
            "fixture.operate",
            &["fixture"],
            "Test fixture: IDLE to BUSY.",
        ),
        row(
            "fixture.stop",
            &[],
            "fixture.operate",
            &["fixture"],
            "Test fixture: BUSY to IDLE.",
        ),
        VerbEntry {
            name: "fixture.person_only",
            args: &[],
            action: "fixture.damaging",
            person_only: true,
            kinds: &["fixture"],
            doc: "Test fixture: a verb only a person's session may send. Does nothing.",
        },
    ];

    pub static STATE_SETS: &[StateSet] = &[StateSet {
        kind: "fixture",
        states: &[
            state(
                "IDLE",
                &[
                    "fixture.echo",
                    "fixture.move",
                    "fixture.start",
                    "fixture.person_only",
                ],
                "Test fixture: waiting.",
            ),
            state(
                "BUSY",
                &["fixture.echo", "fixture.stop"],
                "Test fixture: started.",
            ),
        ],
        transitions: &[
            Transition {
                from: "IDLE",
                to: "BUSY",
                verb: Some("fixture.start"),
            },
            Transition {
                from: "BUSY",
                to: "IDLE",
                verb: Some("fixture.stop"),
            },
            Transition {
                from: "BUSY",
                to: "IDLE",
                verb: None,
            },
        ],
    }];

    pub static EVENT_TYPES: &[EventTypeEntry] = &[EventTypeEntry {
        name: "fixture.happened",
        data: "`{}`",
        visible_with: "fixture.operate",
        doc: "Test fixture: an event type.",
    }];

    pub static ACTIONS: &[ActionEntry] = &[
        ActionEntry {
            name: "fixture.operate",
            presets: &["fixture"],
            damaging: false,
            doc: "Test fixture: an action a preset includes.",
        },
        ActionEntry {
            name: "fixture.damaging",
            presets: &[],
            damaging: true,
            doc: "Test fixture: granted by name only.",
        },
    ];

    pub static FAULT_CODES: &[&FaultCode] = &[&fault::FIXTURE_REFUSED];

    pub static METRICS: &[MetricEntry] = &[MetricEntry {
        name: "fixture.level",
        unit: Unit::Percent,
        doc: "Test fixture: a metric.",
    }];
}

/// No fixture rows outside tests.
#[cfg(not(test))]
mod fx {
    use super::*;
    pub static VERBS: &[VerbEntry] = &[];
    pub static STATE_SETS: &[StateSet] = &[];
    pub static EVENT_TYPES: &[EventTypeEntry] = &[];
    pub static ACTIONS: &[ActionEntry] = &[];
    pub static FAULT_CODES: &[&FaultCode] = &[];
    pub static METRICS: &[MetricEntry] = &[];
}

/// Every verb row (with the fixture rows under `cfg(test)`).
pub fn verbs() -> impl Iterator<Item = &'static VerbEntry> {
    CAPABILITY_VERBS.iter().chain(fx::VERBS)
}
pub fn state_sets() -> impl Iterator<Item = &'static StateSet> {
    STATE_SETS.iter().chain(fx::STATE_SETS)
}
pub fn event_types() -> impl Iterator<Item = &'static EventTypeEntry> {
    EVENT_TYPES.iter().chain(fx::EVENT_TYPES)
}
pub fn actions() -> impl Iterator<Item = &'static ActionEntry> {
    ACTIONS.iter().chain(fx::ACTIONS)
}
pub fn fault_codes() -> impl Iterator<Item = &'static FaultCode> {
    FAULT_CODES.iter().chain(fx::FAULT_CODES).copied()
}
pub fn metrics() -> impl Iterator<Item = &'static MetricEntry> {
    METRICS.iter().chain(fx::METRICS)
}

pub fn verb(name: &str) -> Option<&'static VerbEntry> {
    verbs().find(|v| v.name == name)
}

pub fn state_set(kind: &str) -> Option<&'static StateSet> {
    state_sets().find(|s| s.kind == kind)
}

pub fn action(name: &str) -> Option<&'static ActionEntry> {
    actions().find(|a| a.name == name)
}

/// The registered fault code `domain/REASON`.
pub fn fault_code(code: &str) -> Option<&'static FaultCode> {
    fault_codes().find(|f| f.code() == code)
}
/// What is wrong across the tables; empty when nothing is.
pub fn problems() -> Vec<String> {
    let mut out = Vec::new();
    let mut dup = |what: &str, names: Vec<String>| {
        for (i, n) in names.iter().enumerate() {
            if names[..i].contains(n) {
                out.push(format!("{what} {n} registered twice"));
            }
        }
    };
    dup("verb", verbs().map(|v| v.name.to_owned()).collect());
    dup(
        "state set",
        state_sets().map(|s| s.kind.to_owned()).collect(),
    );
    dup(
        "event type",
        event_types().map(|e| e.name.to_owned()).collect(),
    );
    dup("action", actions().map(|a| a.name.to_owned()).collect());
    dup("fault code", fault_codes().map(|f| f.code()).collect());
    dup("metric", metrics().map(|m| m.name.to_owned()).collect());
    for v in verbs() {
        if action(v.action).is_none() {
            out.push(format!(
                "verb {} needs unregistered action {}",
                v.name, v.action
            ));
        }
        for k in v.kinds {
            if state_set(k).is_none() {
                out.push(format!("verb {} names kind {k} with no state set", v.name));
            }
        }
        for (i, a) in v.args.iter().enumerate() {
            if v.args[..i].iter().any(|b| b.name == a.name) {
                out.push(format!(
                    "verb {} declares argument {} twice",
                    v.name, a.name
                ));
            }
        }
    }
    for s in state_sets() {
        out.extend(s.problems());
        for st in s.states {
            for v in st.accepts {
                if !verb(v).is_some_and(|e| e.kinds.contains(&s.kind)) {
                    out.push(format!(
                        "state set {} accepts {v}, not a registered verb of that kind",
                        s.kind
                    ));
                }
            }
        }
    }
    for e in event_types() {
        if action(e.visible_with).is_none() {
            out.push(format!(
                "event type {} names unregistered action {}",
                e.name, e.visible_with
            ));
        }
    }
    for a in actions() {
        if a.damaging && !a.presets.is_empty() {
            out.push(format!("damaging action {} is in a preset", a.name));
        }
    }
    for f in fault_codes() {
        let reason_ok = !f.reason.is_empty()
            && f.reason
                .bytes()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_');
        let domain_ok = !f.domain.is_empty()
            && f.domain
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c == b'.');
        if !reason_ok || !domain_ok || !(400..=599).contains(&f.status) {
            out.push(format!("fault code {} is malformed", f.code()));
        }
    }
    out
}

const HEADER: &str = "<!-- Generated from crates/device/src/registry.rs. Do not edit: \
run `MC_WRITE_REGISTRIES=1 cargo test -p maker-connect-device --test registry_docs`. -->";

/// The generated docs: file name under `docs/registries/` and its text.
pub fn render_docs() -> Vec<(&'static str, String)> {
    let yes_no = |b: bool| if b { "yes" } else { "no" };
    let list = |items: &[&str]| {
        if items.is_empty() {
            "-".to_owned()
        } else {
            items.join(", ")
        }
    };
    let mut docs = Vec::new();

    let mut s = page(
        "Capability verbs",
        "| Verb | Arguments (kind, unit) | Action | Person only | Kinds | |\n|---|---|---|---|---|---|\n",
    );
    for v in CAPABILITY_VERBS {
        let args: Vec<String> = v
            .args
            .iter()
            .map(|a| match a.unit {
                Some(u) => format!("`{}` ({}, {})", a.name, a.kind.as_str(), u.as_str()),
                None => format!("`{}` ({})", a.name, a.kind.as_str()),
            })
            .collect();
        let args = if args.is_empty() {
            "-".to_owned()
        } else {
            args.join(", ")
        };
        s += &format!(
            "| `{}` | {} | `{}` | {} | {} | {} |\n",
            v.name,
            args,
            v.action,
            yes_no(v.person_only),
            list(v.kinds),
            v.doc
        );
    }
    docs.push(("capability-verbs.md", s));

    let mut s = page("State sets", "");
    for set in STATE_SETS {
        s += &format!("## {}\n\n| State | Accepts | |\n|---|---|---|\n", set.kind);
        for st in set.states {
            s += &format!("| `{}` | {} | {} |\n", st.name, list(st.accepts), st.doc);
        }
        s += "\n| From | To | Verb |\n|---|---|---|\n";
        for t in set.transitions {
            s += &format!(
                "| `{}` | `{}` | {} |\n",
                t.from,
                t.to,
                t.verb
                    .map_or("(on its own)".to_owned(), |v| format!("`{v}`"))
            );
        }
        s += "\n";
    }
    docs.push(("state-sets.md", s.trim_end().to_owned() + "\n"));

    let mut s = page(
        "Event types",
        "| Type | Data | Visible with | |\n|---|---|---|---|\n",
    );
    for e in EVENT_TYPES {
        s += &format!(
            "| `{}` | {} | `{}` | {} |\n",
            e.name, e.data, e.visible_with, e.doc
        );
    }
    docs.push(("event-types.md", s));

    let mut s = page(
        "Actions",
        "| Action | Presets | Damaging | |\n|---|---|---|---|\n",
    );
    for a in ACTIONS {
        s += &format!(
            "| `{}` | {} | {} | {} |\n",
            a.name,
            list(a.presets),
            yes_no(a.damaging),
            a.doc
        );
    }
    docs.push(("actions.md", s));

    let mut s = page("Fault codes", "| Code | HTTP | Text |\n|---|---|---|\n");
    for f in FAULT_CODES {
        s += &format!("| `{}` | {} | {} |\n", f.code(), f.status, f.text);
    }
    docs.push(("fault-codes.md", s));

    let mut s = page("Metrics", "| Metric | Unit | |\n|---|---|---|\n");
    for m in METRICS {
        s += &format!("| `{}` | {} | {} |\n", m.name, m.unit.as_str(), m.doc);
    }
    docs.push(("metrics.md", s));
    docs
}

fn page(title: &str, table_head: &str) -> String {
    format!("# {title}\n\n{HEADER}\n\n{table_head}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tables_are_consistent() {
        assert!(problems().is_empty(), "{:#?}", problems());
    }

    #[test]
    fn fixture_rows_exist_only_in_tests() {
        assert!(
            CAPABILITY_VERBS
                .iter()
                .all(|v| !v.name.starts_with("fixture."))
        );
        assert!(STATE_SETS.iter().all(|s| s.kind != "fixture"));
        assert!(EVENT_TYPES.iter().all(|e| !e.name.starts_with("fixture.")));
        assert!(ACTIONS.iter().all(|a| !a.name.starts_with("fixture.")));
        assert!(FAULT_CODES.iter().all(|f| f.domain != "fixture"));
        assert!(METRICS.iter().all(|m| !m.name.starts_with("fixture.")));
        assert!(
            verb("fixture.echo").is_some(),
            "the fixture rows are there under cfg(test)"
        );
    }

    #[test]
    fn the_device_map_counts() {
        // docs/archive/plans/hub-core-v2-device-map.md, counts table, owner's answers applied; rows the map
        // marks "not checked" (Marlin dialog_action, DYMO cancel) are not declared
        let of = |kind: &str| {
            CAPABILITY_VERBS
                .iter()
                .filter(|v| v.kinds.contains(&kind))
                .count()
        };
        assert_eq!(of(KIND_MARLIN), 17);
        assert_eq!(of(KIND_LABELMANAGER), 1);
        assert_eq!(of(KIND_LABELWRITER), 1);
        assert_eq!(of(KIND_GRBL), 12);
        assert_eq!(of(KIND_SCALE), 0);
        let states = |kind: &str| state_set(kind).unwrap().states.len();
        assert_eq!(states(KIND_MARLIN), 16);
        assert_eq!(states(KIND_LABELMANAGER), 3);
        assert_eq!(states(KIND_LABELWRITER), 4);
        assert_eq!(states(KIND_GRBL), 10);
        assert_eq!(states(KIND_SCALE), 9);
        assert!(verb(LASER_START).unwrap().person_only);
        assert_eq!(CAPABILITY_VERBS.iter().filter(|v| v.person_only).count(), 1);
    }

    #[test]
    fn lookups_find_rows() {
        assert!(verb("fixture.person_only").unwrap().person_only);
        assert!(verb("nope").is_none());
        assert_eq!(fault_code("authz/PERSON_ONLY"), Some(&fault::PERSON_ONLY));
        assert!(state_set("fixture").is_some());
    }

    #[test]
    fn each_registry_renders_one_page() {
        let names: Vec<&str> = render_docs().iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            [
                "capability-verbs.md",
                "state-sets.md",
                "event-types.md",
                "actions.md",
                "fault-codes.md",
                "metrics.md"
            ]
        );
    }
}
