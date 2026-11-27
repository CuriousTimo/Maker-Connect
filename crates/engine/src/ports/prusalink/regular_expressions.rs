//! PrusaLink's `printer_adapter/structures/regular_expressions.py` (`reference/Prusa-Link`,
//! `171148ce`): Contains every regular expression used in the app as a constant (#242, section A).
//! Name by name in the source's order; each pattern is the text Python builds, compiled with
//! `fancy_regex` (Python `re`, lookaheads included; owner's approval M5, as the OctoPrint port).
//!
//! Python's adjacent string literals are `concat!` here, its f-string `format!`.
//!
//! **Not covered:** Python's `$` (no `MULTILINE`) also matches before a final `\n`, `fancy_regex`'s
//! only at the end. PrusaLink matches the printer's lines after `decode_line`
//! (`util.py:229-231`, `.strip()`), so a line never ends in `\n` there.
#![allow(non_upper_case_globals)]

use std::sync::LazyLock;

use fancy_regex::Regex;

use super::consts::MMU_PROGRESS_MAP;

fn compile(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|e| panic!("pattern {pattern:?} does not compile: {e}"))
}

/// `re.escape` (CPython 3.9 `re.py`, `_special_chars_map`, as 3.13): a backslash before each of
/// `()[]{}?*+-|^$\.&~# \t\n\r\v\f`.
fn re_escape(pattern: &str) -> String {
    let mut out = String::new();
    for c in pattern.chars() {
        if "()[]{}?*+-|^$\\.&~# \t\n\r\x0b\x0c".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

macro_rules! regex {
    ($name:ident, $pattern:expr) => {
        pub static $name: LazyLock<Regex> = LazyLock::new(|| compile(&$pattern));
    };
}

regex!(
    OPEN_RESULT_REGEX,
    concat!(
        r"^((?P<ok>File opened): (?P<sdn_lfn>.*) Size: (?P<size>\d+))",
        r"|(?P<nok>open failed).*"
    )
);

regex!(PRINTER_TYPE_REGEX, r"^(?P<code>\d{3,5})$");
regex!(FW_REGEX, r"^(?P<version>\d+\.\d+\.\d+-.*)$");
regex!(
    SN_REGEX,
    concat!(
        r"^(?P<sn>^CZPX\d{4}X\d{3}X.\d{5})|",
        r"(?P<invalid>SN invalid)|(?P<gibberish>.*)$"
    )
);
regex!(VALID_SN_REGEX, r"^(?P<sn>^CZPX\d{4}X\d{3}X.\d{5})$");
regex!(
    NEW_SN_REGEX,
    r"^(?P<sn>^SN(?!20)[2-9][0-9](004|017|022|023|024|025)[K,C]\d{6})$"
);
regex!(NOZZLE_REGEX, r"^(?P<size>\d\.\d+)$");
regex!(PERCENT_REGEX, r"^(?P<percent>\d{0,3})%$");

regex!(
    VALID_USERNAME_REGEX,
    r"^[!#-9;-~][ -!#-9;-~]{1,254}[!#-9;-~]$"
);

// Three options of the password format
// >= 8 chars, one lowercase letter, one uppercase letter, one number
pub const PASS_OPT1: &str = r"((?=.*[a-z])(?=.*[A-Z])(?=.*\d))[\w]{8,}$";
// >= 8 chars, one non-alphanumeric character
pub const PASS_OPT2: &str = r"((?=.*\W)(?=.*[\w])[\w\W]{8,})$";
// >= 15 chars
pub const PASS_OPT3: &str = r"[\w\W]{15,}$";

regex!(
    VALID_PASSWORD_REGEX,
    format!("^({PASS_OPT1}|{PASS_OPT2}|{PASS_OPT3})")
);

regex!(
    LFN_CAPTURE,
    concat!(
        r"^(?P<begin>Begin file list)|",
        r#"(?P<dir_enter>DIR_ENTER: (?P<sdn>/[^ ]*/) \"(?P<ldn>[^\"]*)\")|"#,
        r"(?P<file>(?P<sfn>.*\.(?P<extension>GCO|G)) ",
        r"((0x(?P<m_time>[0-9a-fA-F]+) ?)|(?P<size>\d+ ?)|",
        r#"(\"(?P<lfn>[^\"]*)\") ?)*)|"#,
        r"(?P<dir_exit>DIR_EXIT)|",
        r"(?P<end>End file list)$"
    )
);

regex!(
    SD_PRESENT_REGEX,
    concat!(
        r"^(?P<ok>echo:SD card ok)|",
        r"(?P<fail>(echo:SD init fail)|",
        r"(Error:volume\.init failed)|",
        r"(Error:openRoot failed))$"
    )
);
regex!(SD_EJECTED_REGEX, r"^(echo:SD card released)$");

regex!(ANY_REGEX, r".*");
regex!(CONFIRMATION_REGEX, r"^(ok.*)|(Done saving file\.)$"); // highest priority

// ---CAUTION---
// These are handled by special_commands component
// If you use them without, you'll get false positive print starts
// when the special menu is used
regex!(
    FILE_OPEN_REGEX,
    r#"^echo:enqueing \"M23 (?P<sfn>[^\"]+)\"$"#
);
regex!(START_PRINT_REGEX, r#"^echo:enqueing \"M24\"$"#);
regex!(PRINT_DONE_REGEX, r"^Done printing file$");
// ----------------------------------------

regex!(
    REJECTION_REGEX,
    concat!(
        r#"^(?P<unknown>(echo:Unknown command: (\"[^\"]*\"))|"#,
        r"(Unknown \S code: .*))|",
        r"(?P<cold>echo: cold extrusion prevented)$"
    )
);

regex!(BUSY_REGEX, "^echo:busy: processing$");
regex!(ATTENTION_REGEX, "^echo:busy: paused for user$");
regex!(PAUSE_PRINT_REGEX, r"^// ?action:pause$");
regex!(PAUSED_REGEX, r"^// ?action:paused$");
regex!(RESUME_PRINT_REGEX, "^// ?action:resume$");
regex!(RESUMED_REGEX, "^// ?action:resumed$");
regex!(CANCEL_REGEX, "^// ?action:cancel$");
regex!(READY_REGEX, "^// ?action:ready$");
regex!(NOT_READY_REGEX, "^// ?action:not_ready$");
regex!(REPRINT_REGEX, "^// ?action:start$");
// This girthy regexp tries to capture all error messages requiring printer
// reset using M999 or manual button, with connect, only manual reset shall
// be accepted

regex!(
    ERROR_REGEX,
    concat!(
        r"(Error:(",
        r"(?P<kill>Printer halted\. kill\(\) called!)|",
        // There's another one ending in Supervision required
        r"(?P<stop>Printer stopped due to errors\. Fix.*)))"
    )
);

regex!(
    ERROR_REASON_REGEX,
    concat!(
        // flake8: noqa
        r"(Error:(",
        r"(?P<temp>(0: )?Heaters switched off\. ",
        r"M((?P<mintemp>IN)|(?P<maxtemp>AX))TEMP (?P<bed>BED )?triggered!)|",
        r"(?P<runaway>( ((?P<hotend_runaway>HOTEND)|",
        r"(?P<heatbed_runaway>HEATBED)))? THERMAL RUNAWAY( \( ?PREHEAT ",
        r"((?P<preheat_hotend>HOTEND)|(?P<preheat_heatbed>HEATBED))\))?)))"
    )
);

regex!(
    ATTENTION_REASON_REGEX,
    concat!(
        r"(?P<mbl_too_high>Bed leveling failed. Sensor triggered too high)|",
        r"(?P<mbl_didnt_trigger>Bed leveling failed\. Sensor didn't trigger\. ",
        r"Debris on nozzle\? Waiting for reset\.)|",
        r"(?P<tm_error>TM: error triggered!)"
    )
);

regex!(
    TEMPERATURE_REGEX,
    concat!(
        r"^T:(?P<ntemp>-?\d+\.\d+) /(?P<set_ntemp>-?\d+\.\d+)\s+",
        r"B:(?P<btemp>-?\d+\.\d+) /(?P<set_btemp>-?\d+\.\d+)\s+",
        r"T0:(-?\d+\.\d+) /(-?\d+\.\d+)\s+",
        r"@:(?P<tpwm>-?\d+)\s+",
        r"B@:(?P<bpwm>-?\d+)",
        // 'ptemp' is optional due to legacy probe configurations where this value may not be reported.
        r"( P:(?P<ptemp>-?\d+\.\d+))?",
        r"( A:(?P<atemp>-?\d+\.\d+))?",
        r"$"
    )
);

regex!(
    POSITION_REGEX,
    concat!(
        r"^X:(?P<x>-?\d+\.\d+) Y:(?P<y>-?\d+\.\d+) Z:(?P<z>-?\d+\.\d+) ",
        r"E:(?P<e>-?\d+\.\d+) Count X: (?P<count_x>-?\d+\.\d+) ",
        r"Y:(?P<count_y>-?\d+\.\d+) Z:(?P<count_z>-?\d+\.\d+) ",
        r"E:(?P<count_e>-?\d+\.\d+)$"
    )
);
regex!(
    FAN_REGEX,
    concat!(
        r"E0:(?P<hotend_rpm>\d+) RPM PRN1:(?P<print_rpm>\d+) RPM ",
        r"E0@:(?P<hotend_power>\d+) PRN1@:(?P<print_power>\d+)"
    )
);
// This one takes some explaining
// I cannot assign multiple regular expressions to a single instruction
// The `M27 P` has more lines, the first one containing a status report or
// a file path. The optional second line contains info about
// which byte is being printed and the last one contains the print timer
// Expressions below shall be in the order they appear in the output
regex!(
    M27_OUTPUT_REGEX,
    concat!(
        r"^(?P<sdn_lfn>/.*\..*)|(?P<no_print>Not SD printing)|",
        r"(?P<serial_paused>Print saved)|(?P<sd_paused>SD print paused)|",
        r"(?P<byte_pos>SD printing byte (?P<current>\d+)/(?P<sum>\d+))|",
        r"(?P<printing_time>(?P<hours>\d+):(?P<minutes>\d{2}))$"
    )
);
regex!(
    PRINT_INFO_REGEX,
    concat!(
        r"^(?P<mode>(SILENT)|(NORMAL)) MODE: ",
        r"Percent done: (?P<progress>-?\d+); ",
        r"[pP]rint time remaining in mins: (?P<remaining>-?\d+); ",
        r"Change in mins: (?P<change_in>-?\d+)"
    )
);
regex!(
    HEATING_REGEX,
    r"^T:(?P<ntemp>\d+\.\d+) E:\d+ B:(?P<btemp>\d+\.\d+)$"
);
regex!(
    HEATING_HOTEND_REGEX,
    r"^T:(?P<ntemp>\d+\.\d+) E:([?]|\d+) W:([?]|\d+)$"
);

regex!(RESEND_REGEX, r"^Resend: ?(?P<cmd_number>\d+)$");
regex!(PRINTER_BOOT_REGEX, r"^start$");
regex!(POWER_PANIC_REGEX, r"^INT4$");
regex!(LCD_UPDATE_REGEX, r"^LCD status changed$");
regex!(M110_REGEX, r"^(N\d+)? *M110 ?N(?P<cmd_number>-?\d*)$");
regex!(
    FAN_ERROR_REGEX,
    r"^(?P<fan_name>Extruder|Hotend|Print) fan speed is lower than expected$"
);
regex!(
    D3_OUTPUT_REGEX,
    r"^(?P<address>\w{2,}) {2}(?P<data>([0-9a-fA-F]{2} ?)+)$"
);
regex!(
    MBL_REGEX,
    concat!(
        r"^(?P<no_mbl>Mesh bed leveling not active.)|",
        r"(Num X,Y: (?P<num_x>\d+),(?P<num_y>\d+))|",
        r"(?P<mbl_row>([ ]*-?\d+\.\d+)+)$"
    )
);
regex!(
    MBL_TRIGGER_REGEX,
    concat!(
        r"^(tmc\d+_home_enter\(axes_mask=0x..\))|",
        r#"(echo:enqueing \"G80\")"#
    )
);
regex!(
    TM_ERROR_LOG_REGEX,
    concat!(
        r"TM: error \|(?P<deviation>-?\d+\.?\d*)\|",
        r"[<>](?P<threshold>-?\d+\.?\d*)"
    )
);
regex!(TM_ERROR_CLEARED, r"^TM: error cleared$");

regex!(URLS_FOR_WIZARD, r"/(\d{1,3})?/?");

regex!(TM_CAL_START_REGEX, r"^TM: calibration start$");
regex!(
    TM_CAL_END_REGEX,
    concat!(r"^(TM: calibr\. failed!)|", r"(Thermal Model settings:)$")
);

regex!(
    MMU_MAJOR_REGEX,
    r"^echo:MMU[23]:<R0 A(?P<number>[0-9a-fA-F]+)\*[0-9a-f]{1,2}\.$"
);
regex!(
    MMU_MINOR_REGEX,
    r"^echo:MMU[23]:<R1 A(?P<number>[0-9a-fA-F]+)\*[0-9a-f]{1,2}\.$"
);
regex!(
    MMU_REVISION_REGEX,
    r"^echo:MMU[23]:<R2 A(?P<number>[0-9a-fA-F]+)\*[0-9a-f]{1,2}\.$"
);
regex!(
    MMU_BUILD_REGEX,
    r"^echo:MMU[23]:<R3 A(?P<number>[0-9a-fA-F]+)\*[0-9a-f]{1,2}\.$"
);
regex!(MMU_SLOT_REGEX, r"^echo:MMU2:MMU2tool=(?P<slot>\d{1,2})$");
// This can report an error or a command in progress,
// we don't know before parsing
regex!(
    MMU_Q0_RESPONSE_REGEX,
    concat!(
        r"^echo:MMU[23]:<(?P<command>[A-Z][0-9a-fA-F]+) ",
        r"(?P<progress>[EFP]([0-9a-fA-F]{0,4}))\*[0-9a-f]{1,2}\.$"
    )
);
regex!(MMU_Q0_REGEX, r"^echo:MMU[23]:>Q0\*[0-9a-f]{1,2}\.$");

regex!(
    MMU_PROGRESS_REGEX,
    format!(
        "{}{}{}",
        r"echo:MMU2:(?P<message>",
        MMU_PROGRESS_MAP
            .iter()
            .map(|(key, _)| re_escape(key))
            .collect::<Vec<_>>()
            .join(r"|"),
        r")"
    )
);

regex!(RESET_ACTIVATED_REGEX, r"^Reset mode activated$");
regex!(RESET_DEACTIVATED_REGEX, r"^Reset mode deactivated$");
regex!(PP_RECOVER_REGEX, r"^// ?action:uvlo_recovery_ready$");
regex!(
    PP_AUTO_RECOVER_REGEX,
    r"^// ?action:uvlo_auto_recovery_ready$"
);

#[cfg(test)]
mod tests;
