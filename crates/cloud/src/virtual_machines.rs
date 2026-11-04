//! Virtual machines: simulated in the cloud, one per profile the owner picks, to try Maker
//! Connect without a machine at hand. They take the same G-code as a real printer on built-in
//! Prusa Connect (homing, moves, heating, the fan) and report like one: their head moves
//! at its feed rate, their nozzle and bed heat and cool, their fan spins. Asked to measure
//! their bed (`G80`, `G29`), they visit the points of a grid and report a mesh of a gently
//! warped bed, as a real one would.

use crate::Shared;
use crate::profiles::{self, Profile};
use crate::store::CommandState;
use maker_connect_protocol::{MachineKind, MachineState, Telemetry};
use std::collections::{HashMap, VecDeque};
use std::time::Duration;

/// How often the machines move on (seconds).
const TICK: f32 = 0.5;
const ROOM_C: f32 = 21.0;

/// Runs every virtual machine in the store, for as long as the cloud runs.
pub async fn run(store: Shared) {
    let mut machines: HashMap<String, Sim> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_secs_f32(TICK));
    loop {
        tick.tick().await;
        let mut store = store.lock().unwrap();
        let Ok(list) = store.virtual_machines() else {
            continue;
        };
        machines.retain(|id, _| list.iter().any(|(other, ..)| other == id));
        for (id, token, profile) in list {
            let sim = machines
                .entry(id)
                .or_insert_with(|| Sim::new(profile.as_deref().and_then(profiles::find)));
            if sim.running.is_none()
                && let Ok(Some((command, gcode))) = store.next_command(&token)
            {
                let _ =
                    store.command_went(&token, command, CommandState::Accepted, None, Vec::new());
                sim.start(command, &gcode);
            }
            if let Some(done) = sim.step(TICK) {
                let said = std::mem::take(&mut sim.said);
                let _ = store.command_went(&token, done, CommandState::Finished, None, said);
            }
            let mut telemetry = sim.telemetry();
            if sim.running.is_none() {
                telemetry.mesh = sim.mesh.take();
            }
            let _ = store.report(&token, &telemetry);
        }
    }
}

/// One move of the head: where to, at how many mm/s.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Move {
    to: [f32; 3],
    speed: f32,
}

/// A virtual machine's state.
#[derive(Debug, Clone)]
struct Sim {
    kind: MachineKind,
    size: [f32; 3],
    position: [f32; 3],
    /// Where the head will be once the moves planned so far are done.
    planned: [f32; 3],
    relative: bool,
    /// mm/s.
    feed: f32,
    nozzle: f32,
    nozzle_target: f32,
    bed: f32,
    bed_target: f32,
    /// 0-255, as M106 sets it.
    fan: u8,
    moves: VecDeque<Move>,
    running: Option<u32>,
    /// A mesh measured during the running command, reported once it is done.
    mesh: Option<maker_connect_protocol::Mesh>,
    /// The last mesh measured, for `G81` and `M420 V` to print.
    measured: Option<maker_connect_protocol::Mesh>,
    /// What it says while carrying out the running command, as Marlin would.
    said: Vec<String>,
    model: String,
    /// This machine's own warp, so its meshes look alike from one time to the next.
    warp: [f32; 4],
}

impl Sim {
    fn new(profile: Option<&Profile>) -> Self {
        let size = profile.map_or([200.0; 3], |p| p.build_volume_mm.map(|v| v as f32));
        Sim {
            kind: profile.map_or(MachineKind::Fdm, |p| p.kind),
            size,
            // Somewhere in the middle, as a machine is left.
            position: [size[0] / 2.0, size[1] / 2.0, size[2].min(20.0)],
            planned: [size[0] / 2.0, size[1] / 2.0, size[2].min(20.0)],
            relative: false,
            feed: 50.0,
            nozzle: ROOM_C,
            nozzle_target: 0.0,
            bed: ROOM_C,
            bed_target: 0.0,
            fan: 0,
            moves: VecDeque::new(),
            running: None,
            mesh: None,
            measured: None,
            said: Vec::new(),
            model: profile.map_or("Virtual machine".to_owned(), |p| p.model.clone()),
            warp: {
                use rand::Rng;
                let mut rng = rand::thread_rng();
                [
                    rng.gen_range(-0.12..0.12),
                    rng.gen_range(-0.08..0.08),
                    rng.gen_range(0.05..0.18),
                    rng.gen_range(-0.06..0.06),
                ]
            },
        }
    }

    /// Visits the points of an n × n grid, zigzag, and plans the mesh it will report.
    fn probe(&mut self, points: usize) {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let [width, depth, _] = self.size;
        let margin = 10.0_f32;
        let at =
            |i: usize, size: f32| margin + (size - 2.0 * margin) * i as f32 / (points - 1) as f32;
        for row in 0..points {
            for step in 0..points {
                let column = if row % 2 == 0 {
                    step
                } else {
                    points - 1 - step
                };
                let to = [at(column, width), at(row, depth), 5.0];
                self.moves.push_back(Move { to, speed: 120.0 });
                self.planned = to;
            }
        }
        // A bed is rarely flat: tilted a little, bowed in the middle, a twist, and noise.
        let [tilt_x, tilt_y, bow, twist] = self.warp;
        let rows = (0..points)
            .map(|row| {
                let v = row as f32 / (points - 1) as f32 - 0.5;
                (0..points)
                    .map(|column| {
                        let u = column as f32 / (points - 1) as f32 - 0.5;
                        let z = tilt_x * u + tilt_y * v - bow * (u * u + v * v)
                            + twist * u * v
                            + rng.gen_range(-0.01..0.01);
                        Some((z * 1000.0).round() / 1000.0)
                    })
                    .collect()
            })
            .collect();
        let mesh = maker_connect_protocol::Mesh { rows };
        self.measured = Some(mesh.clone());
        self.mesh = Some(mesh);
    }

    /// What Marlin says to a line before its `ok` (most say nothing).
    fn answer(&self, code: &str) -> Vec<String> {
        let p = self.planned;
        match code {
            "M105" => vec![format!(
                "T:{:.1} /{:.1} B:{:.1} /{:.1}",
                self.nozzle, self.nozzle_target, self.bed, self.bed_target
            )],
            "M114" => vec![format!("X:{:.2} Y:{:.2} Z:{:.2} E:0.00", p[0], p[1], p[2])],
            "M115" => vec![format!(
                "FIRMWARE_NAME:Maker Connect virtual machine MACHINE_TYPE:{}",
                self.model
            )],
            // Prusa's way: the back row first, no row numbers.
            "G81" => match &self.measured {
                Some(mesh) => {
                    let mut lines = vec![
                        format!("Num X,Y: {},{}", mesh.rows[0].len(), mesh.rows.len()),
                        "Measured points:".to_owned(),
                    ];
                    lines.extend(mesh.rows.iter().rev().map(|row| {
                        row.iter()
                            .map(|z| format!("  {:.5}", z.unwrap_or(0.0)))
                            .collect::<String>()
                    }));
                    lines
                }
                None => vec!["Mesh bed leveling not active.".to_owned()],
            },
            // Marlin's way: a header of column numbers, each row after its number.
            "M420" => match &self.measured {
                Some(mesh) => {
                    let mut lines = vec!["Bilinear Leveling Grid:".to_owned()];
                    lines.push(
                        (0..mesh.rows[0].len())
                            .map(|i| format!("{i:>7}"))
                            .collect::<String>(),
                    );
                    lines.extend(mesh.rows.iter().enumerate().map(|(j, row)| {
                        format!(
                            "{j:>2} {}",
                            row.iter()
                                .map(|z| format!("{:+.3} ", z.unwrap_or(0.0)))
                                .collect::<String>()
                        )
                    }));
                    lines
                }
                None => vec!["Bed Leveling OFF".to_owned()],
            },
            _ => Vec::new(),
        }
    }

    /// Takes on a command: its G-code planned, line by line.
    fn start(&mut self, command: u32, gcode: &str) {
        self.running = Some(command);
        for line in gcode.lines() {
            let line = line
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_uppercase();
            let mut words = line.split_whitespace();
            let Some(code) = words.next() else { continue };
            self.said.push(format!("> {line}"));
            let value = |letter: char| {
                line.split_whitespace()
                    .skip(1)
                    .find_map(|w| w.strip_prefix(letter).and_then(|v| v.parse::<f32>().ok()))
            };
            match code {
                "G28" => {
                    self.planned = [0.0; 3];
                    self.moves.push_back(Move {
                        to: [0.0; 3],
                        speed: 60.0,
                    });
                }
                "G90" => self.relative = false,
                "G91" => self.relative = true,
                "G0" | "G1" => {
                    if let Some(f) = value('F') {
                        self.feed = f / 60.0;
                    }
                    let mut to = self.planned;
                    for (axis, letter) in ['X', 'Y', 'Z'].into_iter().enumerate() {
                        if let Some(v) = value(letter) {
                            let target = if self.relative { to[axis] + v } else { v };
                            // Soft end stops: the head stays in the machine.
                            to[axis] = target.clamp(0.0, self.size[axis].max(0.0));
                        }
                    }
                    self.planned = to;
                    self.moves.push_back(Move {
                        to,
                        speed: self.feed.max(1.0),
                    });
                }
                // Prusa's 8-bit firmware measures 7 × 7; Marlin here 5 × 5.
                "G80" => self.probe(7),
                "G29" => self.probe(5),
                "M105" | "M114" | "M115" | "G81" | "M420" => {}
                _ if !code.starts_with(['G', 'M', 'T']) => {
                    self.said.push(format!("echo:Unknown command: \"{line}\""));
                }
                "M104" | "M109" => self.nozzle_target = value('S').unwrap_or(0.0),
                "M140" | "M190" => self.bed_target = value('S').unwrap_or(0.0),
                "M106" => self.fan = value('S').unwrap_or(255.0).clamp(0.0, 255.0) as u8,
                "M107" => self.fan = 0,
                _ => {}
            }
            let answer = self.answer(code);
            self.said.extend(answer);
            self.said.push("ok".to_owned());
        }
    }

    /// Moves on by `seconds`; the command it finished, if it did.
    fn step(&mut self, seconds: f32) -> Option<u32> {
        let approach = |now: f32, target: f32, heating: f32, cooling: f32| {
            let (goal, rate) = if target > 0.0 {
                (target, heating)
            } else {
                (ROOM_C, cooling)
            };
            now + (goal - now) * (1.0 - (-rate * seconds).exp())
        };
        if self.kind == MachineKind::Fdm {
            self.nozzle = approach(self.nozzle, self.nozzle_target, 0.12, 0.03);
            self.bed = approach(self.bed, self.bed_target, 0.025, 0.006);
        }
        let mut left = seconds;
        while let Some(next) = self.moves.front().copied() {
            let gap: Vec<f32> = (0..3).map(|a| next.to[a] - self.position[a]).collect();
            let distance = gap.iter().map(|d| d * d).sum::<f32>().sqrt();
            let reach = next.speed * left;
            if reach >= distance {
                self.position = next.to;
                self.moves.pop_front();
                left -= distance / next.speed;
            } else {
                for (axis, d) in gap.iter().enumerate() {
                    self.position[axis] += d / distance * reach;
                }
                break;
            }
        }
        if self.moves.is_empty() {
            self.running.take()
        } else {
            None
        }
    }

    fn telemetry(&self) -> Telemetry {
        let fdm = self.kind == MachineKind::Fdm;
        let round = |v: f32| (v * 10.0).round() / 10.0;
        Telemetry {
            state: if self.running.is_some() {
                MachineState::Busy
            } else {
                MachineState::Idle
            },
            firmware: Some("Maker Connect virtual machine".to_owned()),
            nozzle_c: fdm.then(|| round(self.nozzle)),
            nozzle_target_c: fdm.then_some(self.nozzle_target),
            bed_c: fdm.then(|| round(self.bed)),
            bed_target_c: fdm.then_some(self.bed_target),
            print_fan_rpm: fdm.then(|| u32::from(self.fan) * 5200 / 255),
            hotend_fan_rpm: fdm.then_some(if self.nozzle > 50.0 { 6800 } else { 0 }),
            position_mm: Some(self.position.map(round)),
            speed_percent: Some(100.0),
            flow_percent: fdm.then_some(100.0),
            material: fdm.then(|| "PLA".to_owned()),
            ..Telemetry::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A virtual MINI homes, moves 10 mm at its feed rate, stays inside its frame, and
    /// heats towards its target.
    #[test]
    fn a_virtual_printer_moves_and_heats() {
        let mut mini = Sim::new(profiles::find("prusa/mini"));
        mini.start(1, "G28");
        while mini.step(TICK).is_none() {}
        assert_eq!(mini.position, [0.0; 3]);

        mini.start(2, "G91\nG1 X10 F600\nG1 Y-50\nG90");
        assert_eq!(mini.step(0.5), None, "10 mm at 10 mm/s takes a second");
        assert_eq!(mini.step(0.5), Some(2));
        assert_eq!(mini.position, [10.0, 0.0, 0.0], "Y stops at 0");

        mini.start(3, "M104 S215\nM140 S60\nM106");
        for _ in 0..120 {
            mini.step(TICK);
        }
        let t = mini.telemetry();
        assert!(t.nozzle_c.unwrap() > 200.0, "{t:?}");
        assert!(t.bed_c.unwrap() > 40.0 && t.bed_c.unwrap() < 60.0, "{t:?}");
        assert_eq!(t.print_fan_rpm, Some(5200));
    }

    /// Measuring the bed visits every point of the grid, then gives a 7 × 7 mesh.
    #[test]
    fn a_virtual_printer_measures_its_bed() {
        let mut mk3 = Sim::new(profiles::find("prusa/mk3s"));
        mk3.start(1, "G28\nG80\nG81");
        assert_eq!(mk3.moves.len(), 1 + 49);
        while mk3.step(TICK).is_none() {}
        let mesh = mk3.mesh.take().unwrap();
        assert_eq!(mesh.rows.len(), 7);
        let (low, high) = mesh.range().unwrap();
        assert!(high - low > 0.01 && high - low < 0.5, "{low} {high}");
    }

    /// It answers as Marlin would: what it runs, then any answer, then ok; G81 prints the
    /// mesh in Prusa's way, which the agent's reader turns back the right way round.
    #[test]
    fn a_virtual_printer_answers_like_marlin() {
        let mut mk3 = Sim::new(profiles::find("prusa/mk3s"));
        mk3.start(1, "M115\nG80\nG81\nFOO");
        let said = mk3.said.join("\n");
        assert!(said.starts_with("> M115\nFIRMWARE_NAME:"), "{said}");
        assert!(said.contains("Measured points:"), "{said}");
        assert!(said.contains("echo:Unknown command: \"FOO\""), "{said}");
        let rows = mk3.measured.as_ref().unwrap().rows.clone();
        let printed = said
            .lines()
            .skip_while(|l| !l.contains("Measured points:"))
            .skip(1);
        let back_row: Vec<f32> = printed
            .clone()
            .next()
            .unwrap()
            .split_whitespace()
            .map(|v| v.parse().unwrap())
            .collect();
        assert_eq!(back_row[0], rows[6][0].unwrap(), "the back row first");
    }

    /// A laser has no temperatures to report.
    #[test]
    fn a_virtual_laser_has_no_heaters() {
        let laser = Sim::new(profiles::find("ortur/lm2-pro-s2"));
        let t = laser.telemetry();
        assert_eq!(t.nozzle_c, None);
        assert!(t.position_mm.is_some());
    }
}
