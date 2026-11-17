//! Port of the refusals of OctoPrint 1.11.8 `src/octoprint/server/api/printer.py` (the HTTP layer, the
//! permissions and the answers are not ported): what each endpoint checks before it acts, as functions
//! the port's top edge calls before it calls `Printer` (`docs/plans/agent-engine.md`, S5: the ports refuse as
//! their reference).
//!
//! Original: Copyright (C) 2014 The OctoPrint Project, AGPL-3.0 (`__author__` Gina Häußge).
//!
//! # Ported (source line per item)
//!
//! * 409 "Printer is not operational": `printerState` (:23), `printerToolCommand` (:73),
//!   `printerToolState` (:161), `printerBedCommand` (:174), `printerBedState` (:216),
//!   `printerChamberCommand` (:236), `printerChamberState` (:278), `printerCommand` (:398),
//!   `_get_temperature_data` (:456);
//! * 409 "Printer is currently printing": the extrude command (:134-136);
//! * 409 "Printer is not operational or currently printing": `printerPrintheadCommand` (:302-304, not for
//!   `feedrate`);
//! * 404 "SD support is disabled" (:357-358, `settings().getBoolean(["feature", "sdSupport"])`) and
//!   409 "Printer is not operational or currently busy": `printerSdCommand` (:360-361);
//! * 409 "Printer does not have a heated bed" (:177, :219) and "... a heated chamber" (:239, :281);
//! * the validation of `printerCommand` (:393-445): the three 400 for `command` / `commands` / `script`
//!   (:403-407), `commands` that is not a list (:418-420), the `command % parameters` (:424-432) and the
//!   script with its context (:434-441, the 404 "Unknown script", :441).
//! * from `server/api/job.py` (`controlJob`, :16): 409 "Printer is not operational" (:21) and the checks of
//!   `start` (:33-38), `restart` (:41-46), `pause` (:48-53) and `cancel` (:63-68), with `job.py`'s texts;
//! * from `server/api/files.py` (`gcodeFileCommand`, :846), command `select`: 415 "Cannot select file for
//!   printing, not a machinecode file" (:877-882) and 409 "Printer is already printing, cannot select a new
//!   file" (:884-888); the 404 of `_verifyFileExists` (:874) is `Printer::_validate_job`'s `InvalidFileLocation`.
//!
//! * `files.py`: the sdcard's `select` with `print` (:871-896) and `deleteGcodeFile` (:1315-1330) as far as they
//!   refuse (404, 409).
//!
//! # Ours
//!
//! 0. (K2, extends the S5 table) the `job.py` and `files.py` checks above, which the port's top edge calls
//!    before it starts a job or acts on its control.
//! 1. A Python exception that Flask turns into a 500 (not an `abort`) is `ApiError::Exception`.
//! 2. `command % parameters` is [`super::py_percent`] (the port of CPython 3.13's `str % args`; its
//!    own doc lists what is `unimplemented!` there and what the top edge must do).
//! 3. The `request`, the JSON parse and `get_json_command_from_request` are the top edge's.

use super::comm::py_truthy;
use super::filemanager::valid_file_type;
use super::settings::{Settings, settings};
use super::standard::{Printer, py_repr_value};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

/// `abort(status, description=...)`, or a Python exception that is not an `abort` (ours, 2)
#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    /// `abort(status, description=description)`
    Abort { status: u16, description: String },
    /// an exception Flask answers with a 500
    Exception(String),
}

fn abort(status: u16, description: &str) -> ApiError {
    ApiError::Abort {
        status,
        description: description.to_string(),
    }
}

/// `if not printer.is_operational(): abort(409, description="Printer is not operational")`
/// (:23, :73, :161, :174, :216, :236, :278, :398, :456)
pub fn check_operational(printer: &Printer) -> Result<(), ApiError> {
    if !printer.is_operational() {
        return Err(abort(409, "Printer is not operational"));
    }
    Ok(())
}

/// `printerState` (:21): `if not printer.is_operational(): abort(409, ...)` (:23)
pub fn printer_state(printer: &Printer) -> Result<(), ApiError> {
    check_operational(printer)
}

/// `printerToolCommand` (:71) before `get_json_command_from_request` (:73); the extrusion has its own
/// check, [`printer_tool_extrude`]
pub fn printer_tool_command(printer: &Printer) -> Result<(), ApiError> {
    check_operational(printer)
}

/// `printerToolCommand`, command `extrude` (:134-136)
pub fn printer_tool_extrude(printer: &Printer) -> Result<(), ApiError> {
    if printer.is_printing() {
        // do not extrude when a print job is running
        return Err(abort(409, "Printer is currently printing"));
    }
    Ok(())
}

/// `printerToolState` (:159, check :161)
pub fn printer_tool_state(printer: &Printer) -> Result<(), ApiError> {
    check_operational(printer)
}

fn heated(printer: &Printer, key: &str) -> bool {
    py_truthy(&printer.printer_profile_manager().get_current_or_default()[key])
}

/// `printerBedCommand` (:172, checks :174, :177)
pub fn printer_bed_command(printer: &Printer) -> Result<(), ApiError> {
    check_operational(printer)?;
    if !heated(printer, "heatedBed") {
        return Err(abort(409, "Printer does not have a heated bed"));
    }
    Ok(())
}

/// `printerBedState` (:214, checks :216, :219)
pub fn printer_bed_state(printer: &Printer) -> Result<(), ApiError> {
    printer_bed_command(printer)
}

/// `printerChamberCommand` (:234, checks :236, :239)
pub fn printer_chamber_command(printer: &Printer) -> Result<(), ApiError> {
    check_operational(printer)?;
    if !heated(printer, "heatedChamber") {
        return Err(abort(409, "Printer does not have a heated chamber"));
    }
    Ok(())
}

/// `printerChamberState` (:276, checks :278, :281)
pub fn printer_chamber_state(printer: &Printer) -> Result<(), ApiError> {
    printer_chamber_command(printer)
}

/// `printerPrintheadCommand` (:296), check :302-304; `command` is the validated command name (`jog`,
/// `home`, `feedrate`)
pub fn printer_printhead_command(printer: &Printer, command: &str) -> Result<(), ApiError> {
    if !printer.is_operational() || (printer.is_printing() && command != "feedrate") {
        // do not jog when a print job is running or we don't have a connection
        return Err(abort(
            409,
            "Printer is not operational or currently printing",
        ));
    }
    Ok(())
}

/// `activePrintjob = printer.is_printing() or printer.is_paused()` (`job.py:23`)
fn active_print_job(printer: &Printer) -> bool {
    printer.is_printing() || printer.is_paused()
}

/// `controlJob` (`job.py:16`), command `start` (:33-38): the operational check (:21) and the active job
pub fn job_start(printer: &Printer) -> Result<(), ApiError> {
    if !printer.is_operational() {
        return Err(abort(409, "Printer is not operational"));
    }
    if active_print_job(printer) {
        return Err(abort(
            409,
            "Printer already has an active print job, did you mean 'restart'?",
        ));
    }
    Ok(())
}

/// `controlJob`, command `restart` (:41-46)
pub fn job_restart(printer: &Printer) -> Result<(), ApiError> {
    if !printer.is_operational() {
        return Err(abort(409, "Printer is not operational"));
    }
    if !printer.is_paused() {
        return Err(abort(
            409,
            "Printer does not have an active print job or is not paused",
        ));
    }
    Ok(())
}

/// `controlJob`, command `pause` (:48-53), whatever its `action` (`pause`, `resume`, `toggle`)
pub fn job_pause(printer: &Printer) -> Result<(), ApiError> {
    if !printer.is_operational() {
        return Err(abort(409, "Printer is not operational"));
    }
    if !active_print_job(printer) {
        return Err(abort(
            409,
            "Printer is neither printing nor paused, 'pause' command cannot be performed",
        ));
    }
    Ok(())
}

/// `controlJob`, command `cancel` (:63-68)
pub fn job_cancel(printer: &Printer) -> Result<(), ApiError> {
    if !printer.is_operational() {
        return Err(abort(409, "Printer is not operational"));
    }
    if !active_print_job(printer) {
        return Err(abort(
            409,
            "Printer is neither printing nor paused, 'cancel' command cannot be performed",
        ));
    }
    Ok(())
}

/// `gcodeFileCommand` (`files.py:846`), command `select` (:871-888), for the file `filename`
pub fn file_select(printer: &Printer, filename: &str) -> Result<(), ApiError> {
    if valid_file_type(filename, Some("machinecode"), None) != Some(true) {
        return Err(abort(
            415,
            "Cannot select file for printing, not a machinecode file",
        ));
    }
    if !printer.is_ready() {
        return Err(abort(
            409,
            "Printer is already printing, cannot select a new file",
        ));
    }
    Ok(())
}

/// `abort(404)` without a description: werkzeug's `NotFound.description`
const NOT_FOUND: &str = "The requested URL was not found on the server. If you entered the URL manually please check your spelling and try again.";

/// `_verifyFileExists("sdcard", filename)` (`files.py:505`): `filename in (x["name"] for x in printer.get_sd_files())`
fn sd_file_exists(printer: &Printer, filename: &str) -> bool {
    printer
        .get_sd_files(false)
        .iter()
        .any(|x| x["name"].as_str() == Some(filename))
}

/// `gcodeFileCommand`, command `select`, target `sdcard` with `print` (:871-905): the 404 of `_verifyFileExists`
/// (:874), the machinecode type (:877), `is_ready` (:884) and, for `print`, `is_operational` (:891-896)
pub fn sd_file_select_and_print(printer: &Printer, filename: &str) -> Result<(), ApiError> {
    if !sd_file_exists(printer, filename) {
        return Err(abort(404, NOT_FOUND));
    }
    file_select(printer, filename)?;
    if !printer.is_operational() {
        return Err(abort(
            409,
            "Printer is not operational, cannot directly start printing",
        ));
    }
    Ok(())
}

/// `deleteGcodeFile` (`files.py:1315`) for the target `sdcard`: the 404 (:1319-1323) and `_isBusy` (:1329,
/// :519-527: the selected file of the sdcard is being printed or is paused). Whether the file is selected
/// and must be unselected first (:1334-1340) is the top edge's.
pub fn sd_file_delete(printer: &Printer, filename: &str) -> Result<(), ApiError> {
    if !sd_file_exists(printer, filename) {
        return Err(abort(404, NOT_FOUND));
    }
    if selected_sd_file(printer).as_deref() == Some(filename) && active_print_job(printer) {
        return Err(abort(
            409,
            "Trying to delete a file that is currently in use",
        ));
    }
    Ok(())
}

/// `_getCurrentFile()` (`files.py:1403`) for the sdcard: the name of the selected file when it is on it
pub fn selected_sd_file(printer: &Printer) -> Option<String> {
    let position = printer.comm()?.get_file_position()?;
    (position.origin == "sdcard").then(|| {
        position
            .filename
            .strip_prefix('/')
            .unwrap_or(&position.filename)
            .to_string()
    })
}

/// `settings().getBoolean(["feature", "sdSupport"])` (:357)
fn sd_support(settings: &Settings) -> bool {
    settings.get_boolean(&["feature", "sdSupport"]) == Some(true)
}

/// `printerSdCommand` (:356), checks :357-358 and :360-361
pub fn printer_sd_command(printer: &Printer) -> Result<(), ApiError> {
    if !sd_support(settings()) {
        return Err(abort(404, "SD support is disabled"));
    }

    if !printer.is_operational() || printer.is_printing() || printer.is_paused() {
        return Err(abort(409, "Printer is not operational or currently busy"));
    }
    Ok(())
}

/// `_get_temperature_data` (:454), check :456
pub fn get_temperature_data(printer: &Printer) -> Result<(), ApiError> {
    check_operational(printer)
}

/// `tags = {"source:api", "api:printer.command"}` (:413)
pub fn printer_command_tags() -> BTreeSet<String> {
    ["source:api", "api:printer.command"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// What `printerCommand` (:396) hands to the printer after its checks
#[derive(Debug, Clone, PartialEq)]
pub enum PrinterCommand {
    /// `printer.commands(commandsToSend, tags=tags)` (:432)
    Commands(Vec<Value>),
    /// `printer.script(script_name, context=context, tags=tags)` (:440); `Unknown script` is a 404 when the
    /// printer answers `UnknownScript` ([`unknown_script`])
    Script { name: String, context: Value },
}

/// `except UnknownScript: abort(404, description="Unknown script")` (:441)
pub fn unknown_script() -> ApiError {
    abort(404, "Unknown script")
}

/// Python `len(value)`; `None` is the `TypeError`
fn py_len(v: &Value) -> Option<usize> {
    match v {
        Value::String(s) => Some(s.chars().count()),
        Value::Array(a) => Some(a.len()),
        Value::Object(o) => Some(o.len()),
        _ => None,
    }
}

/// `printerCommand` (:396) from `if "command" in data and "commands" in data` (:403) to the call: the
/// validation of the posted JSON `data`
pub fn printer_command(data: &Value) -> Result<PrinterCommand, ApiError> {
    let has = |k: &str| match data {
        Value::Object(o) => o.contains_key(k),
        Value::Array(a) => a.iter().any(|x| x.as_str() == Some(k)),
        Value::String(s) => s.contains(k),
        other => panic!(
            "TypeError: argument of type {} is not iterable",
            py_repr_value(other)
        ),
    };

    if has("command") && has("commands") {
        return Err(abort(
            400,
            "'command' and 'commands' are mutually exclusive",
        ));
    } else if (has("command") || has("commands")) && has("script") {
        return Err(abort(
            400,
            "'command'/'commands' and 'script' are mutually exclusive",
        ));
    } else if !(has("command") || has("commands") || has("script")) {
        return Err(abort(400, "Need one of 'command', 'commands' or 'script'"));
    }

    // from here on `data[...]` of a non-dict is Python's `TypeError`
    let Value::Object(data) = data else {
        return Err(ApiError::Exception(
            "TypeError: list indices must be integers or slices, not str".to_string(),
        ));
    };

    let mut parameters: Value = Value::Object(Map::new());
    if let Some(p) = data.get("parameters") {
        parameters = p.clone();
    }

    if data.contains_key("command") || data.contains_key("commands") {
        let commands: Vec<Value> = if let Some(command) = data.get("command") {
            vec![command.clone()]
        } else {
            match &data["commands"] {
                Value::Array(a) => a.clone(),
                _ => return Err(abort(400, "commands is invalid")),
            }
        };

        let mut commands_to_send: Vec<Value> = Vec::new();
        for command in commands {
            commands_to_send.push(command_to_send(&command, &parameters)?);
        }

        Ok(PrinterCommand::Commands(commands_to_send))
    } else {
        // `elif "script" in data`
        let script_name = &data["script"];
        let mut context = Map::new();
        context.insert("parameters".to_string(), parameters);
        if let Some(c) = data.get("context") {
            context.insert("context".to_string(), c.clone());
        }

        match script_name {
            Value::String(name) if !name.is_empty() => Ok(PrinterCommand::Script {
                name: name.clone(),
                context: Value::Object(context),
            }),
            // `printer.script(...)`: `ValueError("name must be set")` / `name[0]` of a non-str
            other if !py_truthy(other) => Err(ApiError::Exception(
                "ValueError: name must be set".to_string(),
            )),
            other => Err(ApiError::Exception(format!(
                "TypeError: {} is not a script name",
                py_repr_value(other)
            ))),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// `command % parameters` (:424-432)
// ---------------------------------------------------------------------------------------------

/// `commandToSend = command; if len(parameters) > 0: commandToSend = command % parameters` (:423-425),
/// for one `command` of the posted JSON. `command % parameters` is [`super::py_percent`], the port of
/// CPython 3.13's `str % args`; a `command` that is no `str` is the `TypeError` of
/// `binary_op1` (`unsupported operand type(s) for %: ...`).
pub fn command_to_send(command: &Value, parameters: &Value) -> Result<Value, ApiError> {
    use super::py_percent::{percent_format, tp_name};
    let Some(parameter_count) = py_len(parameters) else {
        return Err(ApiError::Exception(format!(
            "TypeError: object of type '{}' has no len()",
            tp_name(parameters)
        )));
    };
    if parameter_count == 0 {
        return Ok(command.clone());
    }
    match command {
        Value::String(template) => percent_format(template, parameters)
            .map(Value::String)
            .map_err(|e| ApiError::Exception(e.to_string())),
        other => Err(ApiError::Exception(format!(
            "TypeError: unsupported operand type(s) for %: '{}' and '{}'",
            tp_name(other),
            tp_name(parameters)
        ))),
    }
}

#[cfg(test)]
mod tests {
    //! All OURS (OctoPrint 1.11.8 has no test for `server/api/printer.py`'s checks; they are tested
    //! through its HTTP tests, which are not ported). Each names the source line it checks. The `%`
    //! cases are those of `py_percent_expected.json`, made by CPython 3.13.12.
    use super::*;
    use crate::ports::octoprint::comm::MachineCom;
    use crate::ports::octoprint::profile::PrinterProfileManager;
    use crate::ports::octoprint::standard::{LocalFileManager, Printer};
    use serde_json::json;
    use std::sync::Arc;

    fn printer_in(state: Option<i64>, profile: Value) -> Arc<Printer> {
        let profile = Arc::new(PrinterProfileManager::with_current(profile));
        let printer = Printer::new(Arc::new(LocalFileManager), profile.clone());
        if let Some(state) = state {
            let comm = MachineCom::new(None, None, None, Some(profile));
            comm._change_state(state);
            printer.set_comm(Some(comm));
        }
        printer
    }

    fn profile(bed: bool, chamber: bool) -> Value {
        json!({"heatedBed": bed, "heatedChamber": chamber})
    }

    fn status(r: Result<(), ApiError>) -> Option<(u16, String)> {
        match r {
            Ok(()) => None,
            Err(ApiError::Abort {
                status,
                description,
            }) => Some((status, description)),
            Err(e) => panic!("{e:?}"),
        }
    }

    #[test]
    fn ours_not_operational_is_409() {
        let off = printer_in(None, profile(true, true));
        let expected = Some((409, "Printer is not operational".to_string()));
        assert_eq!(expected, status(printer_state(&off)));
        assert_eq!(expected, status(printer_tool_command(&off)));
        assert_eq!(expected, status(printer_tool_state(&off)));
        assert_eq!(expected, status(printer_bed_command(&off)));
        assert_eq!(expected, status(printer_bed_state(&off)));
        assert_eq!(expected, status(printer_chamber_command(&off)));
        assert_eq!(expected, status(printer_chamber_state(&off)));
        assert_eq!(expected, status(get_temperature_data(&off)));
        let on = printer_in(Some(MachineCom::STATE_OPERATIONAL), profile(true, true));
        for check in [
            printer_state,
            printer_tool_command,
            printer_tool_state,
            printer_bed_command,
            printer_bed_state,
            printer_chamber_command,
            printer_chamber_state,
            get_temperature_data,
        ] {
            assert_eq!(None, status(check(&on)));
        }
    }

    #[test]
    fn ours_no_heated_bed_or_chamber_is_409() {
        let p = printer_in(Some(MachineCom::STATE_OPERATIONAL), profile(false, false));
        let bed = Some((409, "Printer does not have a heated bed".to_string()));
        let chamber = Some((409, "Printer does not have a heated chamber".to_string()));
        assert_eq!(bed, status(printer_bed_command(&p)));
        assert_eq!(bed, status(printer_bed_state(&p)));
        assert_eq!(chamber, status(printer_chamber_command(&p)));
        assert_eq!(chamber, status(printer_chamber_state(&p)));
        // the tool endpoints do not look at the heaters
        assert_eq!(None, status(printer_tool_command(&p)));
    }

    #[test]
    fn ours_printing_and_busy() {
        let printing = printer_in(Some(MachineCom::STATE_PRINTING), profile(true, true));
        assert_eq!(
            Some((409, "Printer is currently printing".to_string())),
            status(printer_tool_extrude(&printing))
        );
        let busy = Some((
            409,
            "Printer is not operational or currently printing".to_string(),
        ));
        assert_eq!(busy, status(printer_printhead_command(&printing, "jog")));
        assert_eq!(busy, status(printer_printhead_command(&printing, "home")));
        assert_eq!(
            None,
            status(printer_printhead_command(&printing, "feedrate"))
        ); // :302
        let off = printer_in(None, profile(true, true));
        assert_eq!(busy, status(printer_printhead_command(&off, "feedrate")));
        let idle = printer_in(Some(MachineCom::STATE_OPERATIONAL), profile(true, true));
        assert_eq!(None, status(printer_tool_extrude(&idle)));
        assert_eq!(None, status(printer_printhead_command(&idle, "jog")));

        let sd_busy = Some((
            409,
            "Printer is not operational or currently busy".to_string(),
        ));
        assert_eq!(sd_busy, status(printer_sd_command(&printing)));
        assert_eq!(sd_busy, status(printer_sd_command(&off)));
        let paused = printer_in(Some(MachineCom::STATE_PAUSED), profile(true, true));
        assert_eq!(sd_busy, status(printer_sd_command(&paused)));
        assert_eq!(None, status(printer_sd_command(&idle)));
        // `abort(404, ...)` follows `sd_support` (the global settings have the default, `true`)
        assert!(sd_support(settings()));
        assert!(!sd_support(&Settings::from_defaults(
            json!({"feature": {"sdSupport": false}})
        )));
        assert!(!sd_support(&Settings::from_defaults(
            json!({"feature": {"sdSupport": null}})
        )));
    }

    /// K2: `job.py`'s and `files.py`'s refusals, with their texts
    #[test]
    fn ours_job_and_file_select_refusals() {
        let off = printer_in(None, profile(true, true));
        let idle = printer_in(Some(MachineCom::STATE_OPERATIONAL), profile(true, true));
        let printing = printer_in(Some(MachineCom::STATE_PRINTING), profile(true, true));
        let paused = printer_in(Some(MachineCom::STATE_PAUSED), profile(true, true));
        let not_operational = Some((409, "Printer is not operational".to_string()));
        let conflict = |text: &str| Some((409, text.to_string()));

        // start (:33-38)
        assert_eq!(not_operational, status(job_start(&off)));
        assert_eq!(None, status(job_start(&idle)));
        let active = conflict("Printer already has an active print job, did you mean 'restart'?");
        assert_eq!(active, status(job_start(&printing)));
        assert_eq!(active, status(job_start(&paused)));
        // restart (:41-46)
        let not_paused = conflict("Printer does not have an active print job or is not paused");
        assert_eq!(not_paused, status(job_restart(&idle)));
        assert_eq!(not_paused, status(job_restart(&printing)));
        assert_eq!(None, status(job_restart(&paused)));
        assert_eq!(not_operational, status(job_restart(&off)));
        // pause and cancel (:48-68)
        let neither = |what: &str| {
            conflict(&format!(
                "Printer is neither printing nor paused, '{what}' command cannot be performed"
            ))
        };
        assert_eq!(neither("pause"), status(job_pause(&idle)));
        assert_eq!(None, status(job_pause(&printing)));
        assert_eq!(None, status(job_pause(&paused)));
        assert_eq!(not_operational, status(job_pause(&off)));
        assert_eq!(neither("cancel"), status(job_cancel(&idle)));
        assert_eq!(None, status(job_cancel(&printing)));
        assert_eq!(None, status(job_cancel(&paused)));
        assert_eq!(not_operational, status(job_cancel(&off)));
        // select (files.py:877-888)
        assert_eq!(None, status(file_select(&idle, "benchy.gcode")));
        assert_eq!(
            Some((
                415,
                "Cannot select file for printing, not a machinecode file".to_string()
            )),
            status(file_select(&idle, "benchy.stl"))
        );
        assert_eq!(
            Some((
                415,
                "Cannot select file for printing, not a machinecode file".to_string()
            )),
            status(file_select(&idle, "job-42"))
        );
        assert_eq!(
            conflict("Printer is already printing, cannot select a new file"),
            status(file_select(&printing, "benchy.gcode"))
        );
    }

    #[test]
    fn ours_printer_command_validation() {
        let abort400 = |d: &str| {
            Err(ApiError::Abort {
                status: 400,
                description: d.to_string(),
            })
        };
        assert_eq!(
            abort400("'command' and 'commands' are mutually exclusive"),
            printer_command(&json!({"command": "M105", "commands": ["M105"]}))
        );
        assert_eq!(
            abort400("'command'/'commands' and 'script' are mutually exclusive"),
            printer_command(&json!({"command": "M105", "script": "x"}))
        );
        assert_eq!(
            abort400("'command'/'commands' and 'script' are mutually exclusive"),
            printer_command(&json!({"commands": ["M105"], "script": "x"}))
        );
        assert_eq!(
            abort400("Need one of 'command', 'commands' or 'script'"),
            printer_command(&json!({"parameters": {}}))
        );
        assert_eq!(
            abort400("commands is invalid"),
            printer_command(&json!({"commands": "M105"}))
        );
        assert_eq!(
            Ok(PrinterCommand::Commands(vec![json!("M105")])),
            printer_command(&json!({"command": "M105"}))
        );
        assert_eq!(
            Ok(PrinterCommand::Commands(vec![json!("M105"), json!("M114")])),
            printer_command(&json!({"commands": ["M105", "M114"]}))
        );
        // `{}` and a falsy parameters leave the command alone (:424)
        assert_eq!(
            Ok(PrinterCommand::Commands(vec![json!("M104 S%d")])),
            printer_command(&json!({"command": "M104 S%d", "parameters": {}}))
        );
        assert_eq!(
            Ok(PrinterCommand::Commands(vec![json!("M104 S200")])),
            printer_command(&json!({"command": "M104 S%(t)d", "parameters": {"t": 200}}))
        );
        // the script and its context (:434-440)
        assert_eq!(
            Ok(PrinterCommand::Script {
                name: "myScript".into(),
                context: json!({"parameters": {}})
            }),
            printer_command(&json!({"script": "myScript"}))
        );
        assert_eq!(
            Ok(PrinterCommand::Script {
                name: "myScript".into(),
                context: json!({"parameters": {"a": 1}, "context": {"b": 2}})
            }),
            printer_command(
                &json!({"script": "myScript", "parameters": {"a": 1}, "context": {"b": 2}})
            )
        );
        assert_eq!(
            ApiError::Abort {
                status: 404,
                description: "Unknown script".into()
            },
            unknown_script()
        );
        assert_eq!(
            ["api:printer.command", "source:api"],
            printer_command_tags()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()[..]
        );
    }

    /// `printer.py:424-432` for a command and parameters of every JSON kind, against CPython 3.13.12
    /// (`scripts/octoprint-percent-check.py`, `wire` of `py_percent_expected.json`)
    #[test]
    fn matches_cpython_3_13_wire() {
        let fixture: Value =
            serde_json::from_str(include_str!("py_percent_expected.json")).expect("fixture");
        let cases = fixture["wire"].as_array().expect("wire");
        assert!(cases.len() > 400);
        let mut failures = Vec::new();
        for case in cases {
            let actual = match command_to_send(&case["command"], &case["parameters"]) {
                Ok(v) => json!({"ok": v}),
                Err(ApiError::Exception(e)) => json!({"error": e}),
                Err(other) => panic!("{other:?}"),
            };
            if actual != case["expected"] {
                failures.push(format!(
                    "{} % {} -> {actual}, CPython: {}",
                    case["command"], case["parameters"], case["expected"]
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} differ:\n{}",
            failures.len(),
            cases.len(),
            failures.join("\n")
        );
    }

    /// what CPython 3.13.12 answers and the port refuses (`unimplemented!`, listed in `py_percent.rs`)
    #[test]
    fn cases_the_port_refuses_are_unimplemented() {
        let fixture: Value =
            serde_json::from_str(include_str!("py_percent_expected.json")).expect("fixture");
        for case in fixture["unimplemented"].as_array().expect("unimplemented") {
            let (format, args) = (
                case["format"].as_str().unwrap().to_string(),
                case["args"].clone(),
            );
            let result = std::panic::catch_unwind(|| {
                crate::ports::octoprint::py_percent::percent_format(&format, &args)
            });
            let err = result.expect_err(&format!("{format:?} % {args} must be unimplemented!"));
            let text = err
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            assert!(
                text.contains("not implemented"),
                "{format:?} % {args}: {text}"
            );
        }
    }
}
