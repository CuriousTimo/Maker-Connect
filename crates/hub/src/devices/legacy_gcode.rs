use crate::{profiles, store};

/// The steps the jog pad offers (mm), and its speeds (mm/min): others are refused.
pub(crate) const JOG_STEPS: [f64; 4] = [0.1, 1.0, 10.0, 50.0];
pub(crate) const JOG_FEEDS: [f64; 4] = [600.0, 1500.0, 3000.0, 6000.0];

/// A move of the jog pad as G-code (printers): a relative move. Its label and G-code.
pub(crate) fn jog_command(action: &str, step: f64, feed: f64) -> Option<(String, String)> {
    let (label, [x, y, z], feed) = jog_move(action, step, feed)?;
    let mut axes = String::new();
    for (name, mm) in [("X", x), ("Y", y), ("Z", z)] {
        if mm != 0.0 {
            axes += &format!(" {name}{mm}");
        }
    }
    Some((label, format!("G91\nG1{axes} F{feed}\nG90")))
}

/// A move of the jog pad (lasers, CNCs, plotters: printers keep their buttons):
/// `jog:<x>:<y>`, each -1, 0 or 1 (diagonals too), or a printer's `jog:0:0:<z>`, by a step and
/// at a speed it offers. Its label, the move in mm per axis, and the speed.
pub(crate) fn jog_move(action: &str, step: f64, feed: f64) -> Option<(String, [f64; 3], f64)> {
    if !JOG_STEPS.contains(&step) || !JOG_FEEDS.contains(&feed) {
        return None;
    }
    let dir = |t: &str| match t {
        "-1" => Some(-1.0),
        "0" => Some(0.0),
        "1" => Some(1.0),
        _ => None,
    };
    // `jog:<x>:<y>`, or `jog:0:0:<z>` (a printer's pad: Z on its own, at the speed its Z
    // buttons always had, F600).
    let parts: Vec<&str> = action.strip_prefix("jog:")?.split(':').collect();
    let (x, y, z) = match parts[..] {
        [x, y] => (dir(x)?, dir(y)?, 0.0),
        ["0", "0", z] => (0.0, 0.0, dir(z)?),
        _ => return None,
    };
    if (x, y, z) == (0.0, 0.0, 0.0) {
        return None;
    }
    let feed = if z != 0.0 { 600.0 } else { feed };
    let mut label = String::from("Move");
    for (name, d) in [("X", x), ("Y", y), ("Z", z)] {
        if d != 0.0 {
            label += &format!(" {name}{}{step}", if d > 0.0 { "+" } else { "−" });
        }
    }
    label += " mm";
    Some((label, [x * step, y * step, z * step], feed))
}

/// The same, of a machine at hand.
pub(crate) fn prusa_mk3_machine(machine: &store::Machine) -> bool {
    let by_profile = machine
        .profile
        .as_deref()
        .and_then(profiles::find)
        .and_then(|p| p.firmware.clone())
        .is_some_and(|f| f == "prusa");
    let by_firmware = machine
        .telemetry
        .as_ref()
        .and_then(|t| t.firmware.as_deref())
        .is_some_and(|f| f.starts_with("Prusa-Firmware"));
    by_profile || by_firmware
}

pub(crate) fn action(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "home" => ("Home", "G28"),
        "x+" => ("X +10 mm", "G91\nG1 X10 F3000\nG90"),
        "x-" => ("X −10 mm", "G91\nG1 X-10 F3000\nG90"),
        "y+" => ("Y +10 mm", "G91\nG1 Y10 F3000\nG90"),
        "y-" => ("Y −10 mm", "G91\nG1 Y-10 F3000\nG90"),
        "z+" => ("Z +10 mm", "G91\nG1 Z10 F600\nG90"),
        "z-" => ("Z −10 mm", "G91\nG1 Z-10 F600\nG90"),
        "preheat-pla" => ("Preheat PLA (215/60 °C)", "M104 S215\nM140 S60"),
        "preheat-petg" => ("Preheat PETG (230/85 °C)", "M104 S230\nM140 S85"),
        "cooldown" => ("Cool down", "M104 S0\nM140 S0"),
        "fan-on" => ("Print fan on", "M106 S255"),
        "fan-off" => ("Print fan off", "M107"),
        "motors-off" => ("Motors off", "M18"),
        // Where the head is, when a person asks: firmware without `Cap:AUTOREPORT_POS` tells it
        // only to an M114 (OctoPrint sends M154 only to firmware with it, comm.py; part 9).
        "read-position" => ("Read position", "M114"),
        // As PrusaLink extrudes (web/controls.py): relative E, at its E feed (100 mm/min).
        "extrude" => ("Extrude 10 mm", "M83\nG1 F100 E10"),
        "retract" => ("Retract 10 mm", "M83\nG1 F100 E-10"),
        "set-origin" => ("Set origin here", "G92 X0 Y0"),
        "go-origin" => ("Go to origin", "G90\nG0 X0 Y0"),
        _ => return None,
    })
}

/// The label of measuring the bed where the mesh comes back to the page.
pub(crate) const PROBE_LABEL: &str = "Probe the bed";

/// Prusa Connect's `core.command.mesh_bed_leveling`, as Prusa's server sends it to a printer on
/// Buddy (`reference/prusa-connect` `internal/connector/xbuddy/verbs.go:41`, `:303-308`: a G-code
/// frame, `fixedGCode("G28\r\nG29")`, a full home first; the same bytes in `wire_test.go:29`).
/// - Who: all 13 profiles in `internal/printer/profile/profiles.json` have
///   `core.command.mesh_bed_leveling` with `"flag": true`: CoreOne (`:96`), CoreOneL (`:558`),
///   CoreOneL_INDX (`:1020`), CoreOne_INDX (`:1486`), IX (`:1944`), MINI (`:2368`), MK3.5
///   (`:2784`), MK3.5S (`:3208`), MK3.9 (`:3632`), MK3.9S (`:4056`), MK4 (`:4480`), MK4S
///   (`:4904`), XL (`:5328`). The MINI's profile covers firmware from `6.4.0-0` up
///   (`:2286-2291`); a firmware no profile covers gets the driver's baseline bounded by what the
///   profiles cover between them (`internal/printer/capability/resolver.go:9-11`), which has it
///   too. So the server offers it to every Buddy printer on any firmware: no model or version
///   gate here.
/// - When: not a `stateChecked` verb (`verbs.go:190-193`): the server sends it in any state, and
///   the firmware decides.
/// - By whom: an operate action (`internal/authz/generatedverbs.go:80`), as our Controls are.
/// - Label (ours): the server has only the key; "Mesh bed leveling" is that key's words.
///
/// No mesh comes back over Prusa Connect.
pub(crate) const MESH_BED_LEVELING: (&str, &str) = ("Mesh bed leveling", "G28\r\nG29");

/// What has a machine measure its bed (its label in the Commands list, and the G-code), by its
/// link and firmware; or why it cannot be done.
pub(crate) fn probe_gcode(
    machine: &store::Machine,
) -> Result<(&'static str, &'static str), &'static str> {
    // Klipper measures with BED_MESH_CALIBRATE; Moonraker sends the mesh by itself.
    if machine.link == "simplyprint" {
        return Ok((PROBE_LABEL, "G28\nBED_MESH_CALIBRATE"));
    }
    // A Marlin printer on the agent (extension part 9): as a Marlin below, when its M115 says it
    // measures (`Cap:AUTOLEVEL`) and keeps a mesh (`Cap:LEVELING_DATA`, ui.md "Bed level").
    if crate::prusa_connect::marlin_agent(machine) {
        let can = |cap: &str| machine.details.as_ref().is_some_and(|d| d.can(cap));
        return if can("AUTOLEVEL") && can("LEVELING_DATA") {
            Ok((PROBE_LABEL, "G28\nG29\nM420 V"))
        } else {
            Err("Its firmware does not say it measures the bed (M115: AUTOLEVEL, LEVELING_DATA)")
        };
    }
    if machine.link == "prusa-connect" {
        return Ok(MESH_BED_LEVELING);
    }
    let firmware = machine
        .profile
        .as_deref()
        .and_then(profiles::find)
        .and_then(|p| p.firmware.as_deref());
    match firmware {
        // Prusa's MK3 family: PrusaLink measures no bed (its mesh is read with M420 after the
        // printer measured it itself).
        Some("prusa") => Err(
            "PrusaLink measures no bed: start it on the printer (Calibration); its mesh is read after",
        ),
        // Marlin (and Prusa's Buddy, which is Marlin): G29 measures, M420 V prints.
        Some("marlin" | "prusa-buddy") => Ok((PROBE_LABEL, "G28\nG29\nM420 V")),
        // Klipper measures with BED_MESH_CALIBRATE, through Moonraker: not connected yet.
        Some("klipper") => {
            Err("measuring the bed of a printer on Klipper follows, with Klipper itself")
        }
        _ => Err("choose the machine's profile first: its firmware says how to measure"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The jog pad: diagonals, its steps and speeds only; a printer's as G-code.
    #[test]
    fn the_jog_pad_moves_by_its_steps() {
        assert_eq!(
            jog_move("jog:1:-1", 0.1, 600.0),
            Some(("Move X+0.1 Y−0.1 mm".to_owned(), [0.1, -0.1, 0.0], 600.0))
        );
        assert_eq!(
            jog_command("jog:-1:0", 50.0, 6000.0).unwrap().1,
            "G91\nG1 X-50 F6000\nG90"
        );
        assert_eq!(jog_move("jog-z:1", 10.0, 3000.0), None, "no such move");
        assert_eq!(jog_move("jog:0:0", 10.0, 3000.0), None);
        // A printer's Z, at F600 whatever the speed.
        assert_eq!(
            jog_command("jog:0:0:-1", 0.1, 3000.0),
            Some((
                "Move Z−0.1 mm".to_owned(),
                "G91\nG1 Z-0.1 F600\nG90".to_owned()
            ))
        );
        assert_eq!(jog_move("jog:1:0:1", 1.0, 3000.0), None);
        assert_eq!(jog_move("jog:2:0", 10.0, 3000.0), None);
        assert_eq!(
            jog_move("jog:1:0", 500.0, 3000.0),
            None,
            "not a step offered"
        );
        assert_eq!(jog_move("jog:1:0", 10.0, 1e9), None, "not a speed offered");
    }

    /// The move buttons send G91, a G1 move at a feed, then G90 again.
    /// Proves the row `G90`/`G91` + `G1 F… X Y Z` of docs/audits/prusalink.md.
    #[test]
    fn the_move_buttons_send_g91_g1_g90() {
        for (button, code) in [
            ("x+", "G91\nG1 X10 F3000\nG90"),
            ("x-", "G91\nG1 X-10 F3000\nG90"),
            ("y+", "G91\nG1 Y10 F3000\nG90"),
            ("y-", "G91\nG1 Y-10 F3000\nG90"),
            ("z+", "G91\nG1 Z10 F600\nG90"),
            ("z-", "G91\nG1 Z-10 F600\nG90"),
        ] {
            assert_eq!(action(button).unwrap().1, code, "{button}");
        }
    }

    /// "Motors off" sends M18, which is the same as M84 in Marlin.
    /// Proves the row `M84` of docs/audits/prusalink.md.
    #[test]
    fn motors_go_off_with_m18() {
        // PrusaLink sends M84; Marlin treats M18 and M84 alike.
        assert_eq!(action("motors-off").unwrap().1, "M18");
    }
}
