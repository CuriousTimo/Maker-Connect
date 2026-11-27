use super::*;
use crate::ports::prusalink::python;
use serde_json::{Value, json};

enum Item {
    Re(&'static LazyLock<Regex>),
    Text(&'static str),
}
use Item::{Re, Text};

/// Every name the module defines, in its order.
fn ours() -> Vec<(&'static str, Item)> {
    vec![
        ("OPEN_RESULT_REGEX", Re(&OPEN_RESULT_REGEX)),
        ("PRINTER_TYPE_REGEX", Re(&PRINTER_TYPE_REGEX)),
        ("FW_REGEX", Re(&FW_REGEX)),
        ("SN_REGEX", Re(&SN_REGEX)),
        ("VALID_SN_REGEX", Re(&VALID_SN_REGEX)),
        ("NEW_SN_REGEX", Re(&NEW_SN_REGEX)),
        ("NOZZLE_REGEX", Re(&NOZZLE_REGEX)),
        ("PERCENT_REGEX", Re(&PERCENT_REGEX)),
        ("VALID_USERNAME_REGEX", Re(&VALID_USERNAME_REGEX)),
        ("PASS_OPT1", Text(PASS_OPT1)),
        ("PASS_OPT2", Text(PASS_OPT2)),
        ("PASS_OPT3", Text(PASS_OPT3)),
        ("VALID_PASSWORD_REGEX", Re(&VALID_PASSWORD_REGEX)),
        ("LFN_CAPTURE", Re(&LFN_CAPTURE)),
        ("SD_PRESENT_REGEX", Re(&SD_PRESENT_REGEX)),
        ("SD_EJECTED_REGEX", Re(&SD_EJECTED_REGEX)),
        ("ANY_REGEX", Re(&ANY_REGEX)),
        ("CONFIRMATION_REGEX", Re(&CONFIRMATION_REGEX)),
        ("FILE_OPEN_REGEX", Re(&FILE_OPEN_REGEX)),
        ("START_PRINT_REGEX", Re(&START_PRINT_REGEX)),
        ("PRINT_DONE_REGEX", Re(&PRINT_DONE_REGEX)),
        ("REJECTION_REGEX", Re(&REJECTION_REGEX)),
        ("BUSY_REGEX", Re(&BUSY_REGEX)),
        ("ATTENTION_REGEX", Re(&ATTENTION_REGEX)),
        ("PAUSE_PRINT_REGEX", Re(&PAUSE_PRINT_REGEX)),
        ("PAUSED_REGEX", Re(&PAUSED_REGEX)),
        ("RESUME_PRINT_REGEX", Re(&RESUME_PRINT_REGEX)),
        ("RESUMED_REGEX", Re(&RESUMED_REGEX)),
        ("CANCEL_REGEX", Re(&CANCEL_REGEX)),
        ("READY_REGEX", Re(&READY_REGEX)),
        ("NOT_READY_REGEX", Re(&NOT_READY_REGEX)),
        ("REPRINT_REGEX", Re(&REPRINT_REGEX)),
        ("ERROR_REGEX", Re(&ERROR_REGEX)),
        ("ERROR_REASON_REGEX", Re(&ERROR_REASON_REGEX)),
        ("ATTENTION_REASON_REGEX", Re(&ATTENTION_REASON_REGEX)),
        ("TEMPERATURE_REGEX", Re(&TEMPERATURE_REGEX)),
        ("POSITION_REGEX", Re(&POSITION_REGEX)),
        ("FAN_REGEX", Re(&FAN_REGEX)),
        ("M27_OUTPUT_REGEX", Re(&M27_OUTPUT_REGEX)),
        ("PRINT_INFO_REGEX", Re(&PRINT_INFO_REGEX)),
        ("HEATING_REGEX", Re(&HEATING_REGEX)),
        ("HEATING_HOTEND_REGEX", Re(&HEATING_HOTEND_REGEX)),
        ("RESEND_REGEX", Re(&RESEND_REGEX)),
        ("PRINTER_BOOT_REGEX", Re(&PRINTER_BOOT_REGEX)),
        ("POWER_PANIC_REGEX", Re(&POWER_PANIC_REGEX)),
        ("LCD_UPDATE_REGEX", Re(&LCD_UPDATE_REGEX)),
        ("M110_REGEX", Re(&M110_REGEX)),
        ("FAN_ERROR_REGEX", Re(&FAN_ERROR_REGEX)),
        ("D3_OUTPUT_REGEX", Re(&D3_OUTPUT_REGEX)),
        ("MBL_REGEX", Re(&MBL_REGEX)),
        ("MBL_TRIGGER_REGEX", Re(&MBL_TRIGGER_REGEX)),
        ("TM_ERROR_LOG_REGEX", Re(&TM_ERROR_LOG_REGEX)),
        ("TM_ERROR_CLEARED", Re(&TM_ERROR_CLEARED)),
        ("URLS_FOR_WIZARD", Re(&URLS_FOR_WIZARD)),
        ("TM_CAL_START_REGEX", Re(&TM_CAL_START_REGEX)),
        ("TM_CAL_END_REGEX", Re(&TM_CAL_END_REGEX)),
        ("MMU_MAJOR_REGEX", Re(&MMU_MAJOR_REGEX)),
        ("MMU_MINOR_REGEX", Re(&MMU_MINOR_REGEX)),
        ("MMU_REVISION_REGEX", Re(&MMU_REVISION_REGEX)),
        ("MMU_BUILD_REGEX", Re(&MMU_BUILD_REGEX)),
        ("MMU_SLOT_REGEX", Re(&MMU_SLOT_REGEX)),
        ("MMU_Q0_RESPONSE_REGEX", Re(&MMU_Q0_RESPONSE_REGEX)),
        ("MMU_Q0_REGEX", Re(&MMU_Q0_REGEX)),
        ("MMU_PROGRESS_REGEX", Re(&MMU_PROGRESS_REGEX)),
        ("RESET_ACTIVATED_REGEX", Re(&RESET_ACTIVATED_REGEX)),
        ("RESET_DEACTIVATED_REGEX", Re(&RESET_DEACTIVATED_REGEX)),
        ("PP_RECOVER_REGEX", Re(&PP_RECOVER_REGEX)),
        ("PP_AUTO_RECOVER_REGEX", Re(&PP_AUTO_RECOVER_REGEX)),
    ]
}

/// PrusaLink's own module, imported: every top-level name of its source in its order, each
/// pattern the text Python built, character for character. The flags are `re.UNICODE` (32) on
/// every one, Python's own for a `str` pattern; `fancy_regex` is Unicode as well.
#[test]
fn identical_to_prusalink() {
    let Some(theirs) = python::run(
        include_str!("../driver_constants.py"),
        &json!({"modules": ["prusa.link.printer_adapter.structures.regular_expressions"]}),
    ) else {
        return;
    };
    let theirs = &theirs["prusa.link.printer_adapter.structures.regular_expressions"];
    let ours = ours();
    let names: Vec<&str> = ours.iter().map(|(name, _)| *name).collect();
    let their_names: Vec<&str> = theirs["names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert_eq!(names, their_names, "the names, in the source's order");
    for (name, item) in &ours {
        let value: Value = match item {
            Re(regex) => json!({"pattern": regex.as_str(), "flags": 32}),
            Text(text) => json!(text),
        };
        assert_eq!(&value, &theirs["values"][*name], "{name}");
    }
}
