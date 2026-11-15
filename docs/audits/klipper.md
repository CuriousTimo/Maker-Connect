# Audit: the hub against Moonraker's `[simplyprint]` component

The hub (`crates/hub/src/simplyprint.rs`) plays the server of the protocol Moonraker's
SimplyPrint component speaks: one WebSocket, every message `{"type": ..., "data": ...}`. The
SimplyPrint server is closed source, so the reference is the open client: what Moonraker sends,
what it accepts and what it reads of it, in `reference/moonraker/moonraker/components/simplyprint.py`
(1671 lines, cited as `simplyprint.py:LINE`). Our side is cited as `simplyprint.rs:LINE`.

**Rule (the user's):** follow the reference to the letter. Nothing "another way", "in effect",
"on purpose". Nothing is claimed without the checker.

**How this list was made (2026-11-10):** every item is extracted mechanically from `simplyprint.py`
by `scripts/klipper-audit-check.py` (message types from the `send_sp` calls and what their data
holds, types and demands from `_process_message` and `_process_demand`, the fields read, the
constants, config options, URL forms, headers, timeouts). Each item is named in backticks below.
Run `python3 -I scripts/klipper-audit-check.py`: it prints what is missing, refuses rows done "another
way", requires a named test (`fn name(` in `crates/**/*.rs`, containing the row's terms) for any row
that says same or fixed, and checks the other direction (section 6).

**Names:** `sent:` is Moonraker to the server; `accepted:` is the server to Moonraker
(`accepted:demand:` for a demand, `#` for a condition in front of it); `value:` is a value a field
takes; `klipper:` is a Klipper object Moonraker subscribes to; `config:`, `const:`, `url:`, `header:`,
`timeout:`, `reconnect:`, `interval:`, `ping:`, `gate:`, `pre-setup:`, `ws:`, `msg:`, `sent-envelope:`,
`accepted-envelope:` are connection details.

**Status:** every row is `UNCHECKED`. This is the list, not a verdict: no row says same or fixed
until a test proves it.

**Not covered:** see section 7.

## 1. Message types Moonraker sends

| item | what Moonraker does | what our hub does now | status |
|---|---|---|---|
| `sent:ambient` | Sends its ambient temperature (a number) (simplyprint.py:1106, simplyprint.py:1291) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:connection` | Sends `{new: connected} or {new: disconnected}` for its link to Klippy (simplyprint.py:648, simplyprint.py:659) | Read: only `new == disconnected`, which sets Disconnected (simplyprint.rs:189-191) | `UNCHECKED` |
| `sent:cpu` | Sends CPU use changes, at most every `intervals[cpu]` (simplyprint.py:811) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:cpu_info` | Sends the whole CPU cache after `connected` (simplyprint.py:1105) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:filament_sensor` | Sends `loaded` or `runout` for the configured filament sensor (simplyprint.py:608, simplyprint.py:836, simplyprint.py:1110) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:file_progress` | Sends the state of a file it is fetching for a `file` demand (simplyprint.py:1499, simplyprint.py:1517, simplyprint.py:1545, simplyprint.py:1563) | Read only when `state` is `error` (simplyprint.rs:260-263); other states ignored | `UNCHECKED` |
| `sent:firmware` | Sends `{fw: {...}, raw: false, unsafe: ...}` on Klippy start and after `connected` (simplyprint.py:1060, simplyprint.py:1094) | Read: `fw.firmware_version` (and `fw.firmware`) into the firmware text; nothing if `firmware_version` is absent (simplyprint.rs:192-199) | `UNCHECKED` |
| `sent:gcode_executed` | Answers a `gcode` demand that has an `identifier` (simplyprint.py:414) | Read; counts only when `identifier` equals the command the hub sent (simplyprint.rs:271-282) | `UNCHECKED` |
| `sent:job_info` | Sends job events and progress: `started`, `finished`, `cancelled`, `failed`, `paused`, `progress`, `time`, `layer`, ... (simplyprint.py:720, simplyprint.py:866, simplyprint.py:915, simplyprint.py:988) | Read: `filename`, `progress`, `time`, `finished`, `cancelled`, `failed`, `error` (simplyprint.rs:229-259); a finished/cancelled/failed job ends the hub's job | `UNCHECKED` |
| `sent:latency` | Sends the round trip it measured, in answer to `pong` (simplyprint.py:331) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:machine_data` | Sends its computer and UI data after `connected` and when the web UI changes (simplyprint.py:631, simplyprint.py:640, simplyprint.py:1034) | Read into the agent info: `api_version`, `os`, `machine`, `hostname`, `local_ip`; the rest ignored (simplyprint.rs:201-212) | `UNCHECKED` |
| `sent:mesh_data` | Sends the whole `bed_mesh` status object it subscribed to (simplyprint.py:984) | Read, but from `probed_matrix` (simplyprint.rs:215), a key Moonraker does not send: a mesh from Moonraker never arrives | `UNCHECKED` |
| `sent:objects` | Sends the object list of `exclude_object` (simplyprint.py:857) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:ping` | Sends `ping` with no data every `intervals[ping]` after `connected` (simplyprint.py:846) | Answered with `pong` (no data) (simplyprint.rs:132) | `UNCHECKED` |
| `sent:power_controller` | Sends the state of its configured power device (simplyprint.py:618, simplyprint.py:1087) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:printer_error` | Sends Klippy's error text when Klippy is not ready or shuts down (simplyprint.py:647, simplyprint.py:655) | Read: `error` becomes the problem text (simplyprint.rs:186-188) | `UNCHECKED` |
| `sent:shutdown` | Sends `shutdown` (no data) when Moonraker closes, then closes the socket with 1001 (simplyprint.py:1172) | Ignored (`_ => {}`, simplyprint.rs:283): the printer stays as last reported until the socket closes | `UNCHECKED` |
| `sent:state_change` | Sends `{new: <state>}` when its state changes, and once after `connected` (`_push_initial_state`) (simplyprint.py:976, simplyprint.py:1090) | Read: `new` mapped to a machine state (`state`, simplyprint.rs:300-308; `offline` and unknown values become Disconnected) | `UNCHECKED` |
| `sent:stream` | Sends a webcam picture (base64) after a `webcam_snapshot` demand with `timer` (simplyprint.py:1457) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:temps` | Sends `{tool0: [now, target], bed: [now, target]}`, whole numbers; only `[now]` when the target did not change; only what changed (simplyprint.py:959, simplyprint.py:1092) | Read: `tool0` and `bed` only, each `[now, target]`; an absent target keeps the old one; other keys skipped (simplyprint.rs:163-179) | `UNCHECKED` |
| `sent:term_update` | Sends the terminal: Klipper's responses (`response`) and received commands (`command`), only when `terminal` is enabled (simplyprint.py:781, simplyprint.py:786) | Read: `response` lines are kept as the command's output (simplyprint.rs:264-270) | `UNCHECKED` |
| `sent:tool` | Sends the active extruder as `T0`, `T1`... (simplyprint.py:1069, simplyprint.py:1103) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:webcam` | Sends flip/rotate settings of its webcam (simplyprint.py:1078) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:webcam_status` | Sends whether its snapshot URL answers (simplyprint.py:528, simplyprint.py:1113) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |

The data of every message is its own dict (the fields are in section 2). `temps` values are whole numbers (`int(x + .5)`, simplyprint.py:924-927).


## 2. Fields inside the messages it sends

| item | what Moonraker does | what our hub does now | status |
|---|---|---|---|
| `sent:ambient.new` | `self.send_sp("ambient", {"new": self.amb_detect.ambient})` (simplyprint.py:1106, simplyprint.py:1291) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:connection.new` | `self.send_sp("connection", {"new": "connected"})` (simplyprint.py:648, simplyprint.py:659) | Read only for `disconnected` (simplyprint.rs:189); `connected` is ignored | `UNCHECKED` |
| `sent:cpu.flags` | `self.send_sp("cpu", diff)` (simplyprint.py:811) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:cpu.memory` | `self.send_sp("cpu", diff)` (simplyprint.py:811) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:cpu.temp` | `self.send_sp("cpu", diff)` (simplyprint.py:811) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:cpu.usage` | `self.send_sp("cpu", diff)` (simplyprint.py:811) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:cpu_info.flags` | `self.send_sp("cpu_info", self.cache.cpu_info)` (simplyprint.py:1105) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:cpu_info.memory` | `self.send_sp("cpu_info", self.cache.cpu_info)` (simplyprint.py:1105) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:cpu_info.temp` | `self.send_sp("cpu_info", self.cache.cpu_info)` (simplyprint.py:1105) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:cpu_info.usage` | `self.send_sp("cpu_info", self.cache.cpu_info)` (simplyprint.py:1105) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:filament_sensor.state` | `self.send_sp("filament_sensor", {"state": fstate})` (simplyprint.py:608, simplyprint.py:836, simplyprint.py:1110) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:file_progress.job_id` | Added by `send_sp` to every `job_info` and `file_progress` while a job_id from a `file` demand is tracked (simplyprint.py:1134) | Not read | `UNCHECKED` |
| `sent:file_progress.message` | `self.simplyprint.send_sp( "file_progress", {"state": "error", "message": "GCode Path not Registered"} )` (simplyprint.py:1499, simplyprint.py:1517, simplyprint.py:1545, simplyprint.py:1563) | Read as the job's failure reason when `state` is `error` (simplyprint.rs:261) | `UNCHECKED` |
| `sent:file_progress.percent` | `self.simplyprint.send_sp( "file_progress", {"state": "downloading", "percent": percent} )` (simplyprint.py:1613) | Not read | `UNCHECKED` |
| `sent:file_progress.state` | `self.simplyprint.send_sp( "file_progress", {"state": "error", "message": "GCode Path not Registered"} )` (simplyprint.py:1499, simplyprint.py:1517, simplyprint.py:1545, simplyprint.py:1563) | Only `error` is read (simplyprint.rs:260); `downloading`, `started`, `ready` are ignored | `UNCHECKED` |
| `sent:firmware.fw` | `self.send_sp( "firmware", {"fw": diff, "raw": False, "unsafe": unsafe} )` (simplyprint.py:1060, simplyprint.py:1094) | Read (simplyprint.rs:193) | `UNCHECKED` |
| `sent:firmware.fw.firmware` | `self.send_sp( "firmware", {"fw": diff, "raw": False, "unsafe": unsafe} )` (simplyprint.py:1060, simplyprint.py:1094) | Read (simplyprint.rs:197) | `UNCHECKED` |
| `sent:firmware.fw.firmware_date` | `self.send_sp( "firmware", {"fw": diff, "raw": False, "unsafe": unsafe} )` (simplyprint.py:1060, simplyprint.py:1094) | Not read | `UNCHECKED` |
| `sent:firmware.fw.firmware_link` | `self.send_sp( "firmware", {"fw": diff, "raw": False, "unsafe": unsafe} )` (simplyprint.py:1060, simplyprint.py:1094) | Not read | `UNCHECKED` |
| `sent:firmware.fw.firmware_version` | `self.send_sp( "firmware", {"fw": diff, "raw": False, "unsafe": unsafe} )` (simplyprint.py:1060, simplyprint.py:1094) | Read (simplyprint.rs:194) | `UNCHECKED` |
| `sent:firmware.fw.ui` | `self.send_sp( "firmware", {"fw": self.cache.firmware_info, "raw": False})` (simplyprint.py:1094) | Not read | `UNCHECKED` |
| `sent:firmware.fw.ui_version` | `self.send_sp( "firmware", {"fw": self.cache.firmware_info, "raw": False})` (simplyprint.py:1094) | Not read | `UNCHECKED` |
| `sent:firmware.raw` | `self.send_sp( "firmware", {"fw": diff, "raw": False, "unsafe": unsafe} )` (simplyprint.py:1060, simplyprint.py:1094) | Not read | `UNCHECKED` |
| `sent:firmware.unsafe` | `self.send_sp( "firmware", {"fw": diff, "raw": False, "unsafe": unsafe} )` (simplyprint.py:1060) | Not read | `UNCHECKED` |
| `sent:gcode_executed.identifier` | `self.send_sp( "gcode_executed", { "identifier": ident, "success": success, "message": msg } )` (simplyprint.py:414) | Read as text, parsed to the command id (simplyprint.rs:272) | `UNCHECKED` |
| `sent:gcode_executed.message` | `self.send_sp( "gcode_executed", { "identifier": ident, "success": success, "message": msg } )` (simplyprint.py:414) | Read as the failure reason (simplyprint.rs:278) | `UNCHECKED` |
| `sent:gcode_executed.success` | `self.send_sp( "gcode_executed", { "identifier": ident, "success": success, "message": msg } )` (simplyprint.py:414) | Read: `== true` (simplyprint.rs:277) | `UNCHECKED` |
| `sent:job_info.cancelled` | `self.send_sp("job_info", job_info)` (simplyprint.py:988, simplyprint.py:1100) | Read (simplyprint.rs:248) | `UNCHECKED` |
| `sent:job_info.delay` | `self.send_sp("job_info", job_info)` (simplyprint.py:988, simplyprint.py:1100) | Not read | `UNCHECKED` |
| `sent:job_info.error` | `self.send_sp("job_info", job_info)` (simplyprint.py:988, simplyprint.py:1100) | Read (simplyprint.rs:251) | `UNCHECKED` |
| `sent:job_info.failed` | `self.send_sp("job_info", job_info)` (simplyprint.py:988, simplyprint.py:1100) | Read (simplyprint.rs:250) | `UNCHECKED` |
| `sent:job_info.filament` | `self.send_sp("job_info", job_info)` (simplyprint.py:988, simplyprint.py:1100) | Not read | `UNCHECKED` |
| `sent:job_info.filename` | `self.send_sp("job_info", job_info)` (simplyprint.py:988, simplyprint.py:1100) | Read (simplyprint.rs:237) | `UNCHECKED` |
| `sent:job_info.finished` | `self.send_sp("job_info", job_info)` (simplyprint.py:988, simplyprint.py:1100) | Read (simplyprint.rs:246) | `UNCHECKED` |
| `sent:job_info.job_id` | Added by `send_sp` to every `job_info` and `file_progress` while a job_id from a `file` demand is tracked (simplyprint.py:1134) | Not read; the hub tracks its own job id (`printer.job`) | `UNCHECKED` |
| `sent:job_info.layer` | `self.send_sp("job_info", job_info)` (simplyprint.py:915, simplyprint.py:988, simplyprint.py:1100) | Not read | `UNCHECKED` |
| `sent:job_info.object` | `self.send_sp("job_info", job_data)` (simplyprint.py:866) | Not read | `UNCHECKED` |
| `sent:job_info.paused` | `self.send_sp("job_info", {"paused": True})` (simplyprint.py:720) | Not read (`state_change` paused is read instead) | `UNCHECKED` |
| `sent:job_info.progress` | `self.send_sp("job_info", job_info)` (simplyprint.py:915, simplyprint.py:988, simplyprint.py:1100) | Read as a whole number (`as_u64`, simplyprint.rs:240) | `UNCHECKED` |
| `sent:job_info.skipped_objects` | `self.send_sp("job_info", job_data)` (simplyprint.py:866) | Not read | `UNCHECKED` |
| `sent:job_info.started` | `self.send_sp("job_info", job_info)` (simplyprint.py:988, simplyprint.py:1100) | Not read (the hub marks its own job from the `file` demand) | `UNCHECKED` |
| `sent:job_info.time` | Estimated time at job start, later the time left in seconds (simplyprint.py:915, simplyprint.py:988, simplyprint.py:1100) | Read as time LEFT (`remaining_s`, simplyprint.rs:243); Moonraker's first `time` is the estimate of the whole job | `UNCHECKED` |
| `sent:latency.ms` | `self.send_sp("latency", {"ms": int(diff * 1000 + .5)})` (simplyprint.py:331) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:machine_data.api` | `self.send_sp("machine_data", data)` (simplyprint.py:1034) | Not read | `UNCHECKED` |
| `sent:machine_data.api_version` | `self.send_sp("machine_data", data)` (simplyprint.py:1034) | Read (simplyprint.rs:203) | `UNCHECKED` |
| `sent:machine_data.core_count` | `self.send_sp("machine_data", data)` (simplyprint.py:1034) | Not read | `UNCHECKED` |
| `sent:machine_data.hostname` | `self.send_sp("machine_data", data)` (simplyprint.py:1034) | Read (simplyprint.rs:208) | `UNCHECKED` |
| `sent:machine_data.is_ethernet` | `self.send_sp("machine_data", data)` (simplyprint.py:1034) | Not read | `UNCHECKED` |
| `sent:machine_data.local_ip` | `self.send_sp("machine_data", data)` (simplyprint.py:1034) | Read (simplyprint.rs:209) | `UNCHECKED` |
| `sent:machine_data.machine` | `self.send_sp("machine_data", data)` (simplyprint.py:1034) | Read (simplyprint.rs:207) | `UNCHECKED` |
| `sent:machine_data.os` | `self.send_sp("machine_data", data)` (simplyprint.py:1034) | Read (simplyprint.rs:206) | `UNCHECKED` |
| `sent:machine_data.python_version` | `self.send_sp("machine_data", data)` (simplyprint.py:1034) | Not read | `UNCHECKED` |
| `sent:machine_data.sp_version` | `self.send_sp("machine_data", data)` (simplyprint.py:1034) | Not read | `UNCHECKED` |
| `sent:machine_data.ssid` | `self.send_sp("machine_data", data)` (simplyprint.py:1034) | Not read | `UNCHECKED` |
| `sent:machine_data.total_memory` | `self.send_sp("machine_data", data)` (simplyprint.py:1034) | Not read | `UNCHECKED` |
| `sent:machine_data.ui` | `self.send_sp("machine_data", ui_data)` (simplyprint.py:631, simplyprint.py:640, simplyprint.py:1034) | Not read | `UNCHECKED` |
| `sent:machine_data.ui_version` | `self.send_sp("machine_data", ui_data)` (simplyprint.py:631, simplyprint.py:640, simplyprint.py:1034) | Not read | `UNCHECKED` |
| `sent:mesh_data.mesh_matrix` | Klipper's `bed_mesh.mesh_matrix` (subscribed), sent inside `mesh_data` (simplyprint.py:984) | Not read. The hub reads `probed_matrix` instead (simplyprint.rs:215). Gap | `UNCHECKED` |
| `sent:mesh_data.mesh_max` | Klipper's `bed_mesh.mesh_max` (subscribed) (simplyprint.py:984) | Not read | `UNCHECKED` |
| `sent:mesh_data.mesh_min` | Klipper's `bed_mesh.mesh_min` (subscribed) (simplyprint.py:984) | Not read | `UNCHECKED` |
| `sent:objects.*` | Sends the `objects` list of `exclude_object` as the data (a list, not a dict) (simplyprint.py:857) | Ignored | `UNCHECKED` |
| `sent:power_controller.on` | `self.send_sp("power_controller", {"on": is_on})` (simplyprint.py:618, simplyprint.py:1087) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:printer_error.error` | `self.send_sp("printer_error", {"error": kconn.state.message})` (simplyprint.py:647, simplyprint.py:655) | Read (simplyprint.rs:187) | `UNCHECKED` |
| `sent:state_change.new` | `self.send_sp("state_change", {"new": new_state})` (simplyprint.py:976, simplyprint.py:1090) | Read (simplyprint.rs:181) | `UNCHECKED` |
| `sent:stream.base` | `self.simplyprint.send_sp("stream", {"base": img})` (simplyprint.py:1457) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:temps.bed` | `self.heaters[htr] = "bed"` (simplyprint.py:566) | Read (simplyprint.rs:169) | `UNCHECKED` |
| `sent:temps.tool0` | `tool_id = "tool0"` (simplyprint.py:560) | Read (simplyprint.rs:168) | `UNCHECKED` |
| `sent:temps.tool<N>` | Key `tool` + the number of each extra extruder (`extruder1` is `tool1`) (simplyprint.py:562) | Not read: only `tool0` is matched (`_ => continue`, simplyprint.rs:170); extra tools are dropped | `UNCHECKED` |
| `sent:term_update.command` | `self.send_sp("term_update", {"command": cmds})` (simplyprint.py:786) | Not read | `UNCHECKED` |
| `sent:term_update.response` | `self.send_sp("term_update", {"response": resp})` (simplyprint.py:781) | Read (simplyprint.rs:265) | `UNCHECKED` |
| `sent:tool.new` | `self.send_sp("tool", {"new": tool})` (simplyprint.py:1069, simplyprint.py:1103) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:webcam.flipH` | `self.send_sp("webcam", wc_data)` (simplyprint.py:1078) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:webcam.flipV` | `self.send_sp("webcam", wc_data)` (simplyprint.py:1078) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:webcam.rotate90` | `self.send_sp("webcam", wc_data)` (simplyprint.py:1078) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |
| `sent:webcam_status.connected` | `self.send_sp( "webcam_status", {"connected": self.webcam_stream.connected} )` (simplyprint.py:528, simplyprint.py:1113) | Ignored: the type is not read (`_ => {}`, simplyprint.rs:283) | `UNCHECKED` |

### 2b. Values the fields take

| item | what Moonraker does | what our hub does now | status |
|---|---|---|---|
| `value:connection.new=connected` | `self.send_sp("connection", {"new": "connected"})` (simplyprint.py:648) | Not read | `UNCHECKED` |
| `value:connection.new=disconnected` | `self.send_sp("connection", {"new": "disconnected"})` (simplyprint.py:659) | Read as a value (simplyprint.rs:189) | `UNCHECKED` |
| `value:filament_sensor.state=loaded` | `fstate = "loaded" if detected else "runout"` (simplyprint.py:606, simplyprint.py:834) | Not read | `UNCHECKED` |
| `value:filament_sensor.state=runout` | `fstate = "loaded" if detected else "runout"` (simplyprint.py:606, simplyprint.py:834) | Not read | `UNCHECKED` |
| `value:file_progress.message=Failed to start print` | `data["message"] = "Failed to start print"` (simplyprint.py:1587) | Not read | `UNCHECKED` |
| `value:file_progress.message=GCode Finalization Failed: {e}` | `{"state": "error", "message": f"GCode Finalization Failed: {e}"}` (simplyprint.py:1547) | Not read | `UNCHECKED` |
| `value:file_progress.message=GCode Path not Registered` | `{"state": "error", "message": "GCode Path not Registered"}` (simplyprint.py:1501) | Not read | `UNCHECKED` |
| `value:file_progress.message=Network Error` | `{"state": "error", "message": "Network Error"}` (simplyprint.py:1519) | Not read | `UNCHECKED` |
| `value:file_progress.message=Pending print timed out` | `{"state": "error", "message": "Pending print timed out"}` (simplyprint.py:1565) | Not read | `UNCHECKED` |
| `value:file_progress.state=downloading` | `"file_progress", {"state": "downloading", "percent": percent}` (simplyprint.py:1614) | Not read | `UNCHECKED` |
| `value:file_progress.state=error` | `{"state": "error", "message": "GCode Path not Registered"}` (simplyprint.py:1501, simplyprint.py:1519, simplyprint.py:1547, simplyprint.py:1565) | Read as a value (simplyprint.rs:260) | `UNCHECKED` |
| `value:file_progress.state=pending` | `state = "pending"` (simplyprint.py:1538) | Not read | `UNCHECKED` |
| `value:file_progress.state=ready` | `state = "ready"` (simplyprint.py:1556, simplyprint.py:1572) | Not read | `UNCHECKED` |
| `value:file_progress.state=started` | `state = "started"` (simplyprint.py:1552, simplyprint.py:1581) | Not read | `UNCHECKED` |
| `value:state_change.new=cancelling` | `self._update_state("cancelling")` (simplyprint.py:515, simplyprint.py:774) | Read as a value (simplyprint.rs:300) | `UNCHECKED` |
| `value:state_change.new=error` | `self._update_state("error")` (simplyprint.py:644, simplyprint.py:652, simplyprint.py:967) | Read as a value (simplyprint.rs:300) | `UNCHECKED` |
| `value:state_change.new=offline` | `self._update_state("offline")` (simplyprint.py:658, simplyprint.py:969, simplyprint.py:1188) | Not read | `UNCHECKED` |
| `value:state_change.new=operational` | `self._update_state("operational")` (simplyprint.py:537, simplyprint.py:965) | Read as a value (simplyprint.rs:300) | `UNCHECKED` |
| `value:state_change.new=paused` | `self._update_state("paused")` (simplyprint.py:721) | Read as a value (simplyprint.rs:300) | `UNCHECKED` |
| `value:state_change.new=pausing` | `self._update_state("pausing")` (simplyprint.py:506, simplyprint.py:764) | Read as a value (simplyprint.rs:300) | `UNCHECKED` |
| `value:state_change.new=printing` | `self._update_state("printing")` (simplyprint.py:679, simplyprint.py:725) | Read as a value (simplyprint.rs:300) | `UNCHECKED` |
| `value:state_change.new=resuming` | `self._update_state("resuming")` (simplyprint.py:511, simplyprint.py:769) | Read as a value (simplyprint.rs:300) | `UNCHECKED` |

### 2c. Klipper objects Moonraker subscribes to (they fill `temps`, `mesh_data`, `job_info`, `tool`, `objects`, `filament_sensor`)

| item | what Moonraker does | what our hub does now | status |
|---|---|---|---|
| `klipper:[cfg_amb_sensor].temperature` | `sub_objs[cfg_amb_sensor] = ["temperature"]` (simplyprint.py:571) | Nothing of it is read directly; only through the messages above | `UNCHECKED` |
| `klipper:[htr].target` | `sub_objs[htr] = ["temperature", "target"]` (simplyprint.py:558, simplyprint.py:565) | Nothing of it is read directly; only through the messages above | `UNCHECKED` |
| `klipper:[htr].temperature` | `sub_objs[htr] = ["temperature", "target"]` (simplyprint.py:558, simplyprint.py:565) | Nothing of it is read directly; only through the messages above | `UNCHECKED` |
| `klipper:[self.filament_sensor].filament_detected` | `sub_objs[self.filament_sensor] = ["filament_detected"]` (simplyprint.py:581) | Nothing of it is read directly; only through the messages above | `UNCHECKED` |
| `klipper:bed_mesh.mesh_matrix` | `sub_objs = { "display_status": ["progress"], "virtual_sdcard": ["file_position", "progress"], "bed_mesh": ["mesh_matrix", "mesh_min", "mesh_max"], "toolhead": ["extruder"` (simplyprint.py:540) | Moonraker sends it in `mesh_data`; the hub reads `probed_matrix` instead (simplyprint.rs:215) | `UNCHECKED` |
| `klipper:bed_mesh.mesh_max` | `sub_objs = { "display_status": ["progress"], "virtual_sdcard": ["file_position", "progress"], "bed_mesh": ["mesh_matrix", "mesh_min", "mesh_max"], "toolhead": ["extruder"` (simplyprint.py:540) | Nothing of it is read directly; only through the messages above | `UNCHECKED` |
| `klipper:bed_mesh.mesh_min` | `sub_objs = { "display_status": ["progress"], "virtual_sdcard": ["file_position", "progress"], "bed_mesh": ["mesh_matrix", "mesh_min", "mesh_max"], "toolhead": ["extruder"` (simplyprint.py:540) | Nothing of it is read directly; only through the messages above | `UNCHECKED` |
| `klipper:display_status.progress` | `sub_objs = { "display_status": ["progress"], "virtual_sdcard": ["file_position", "progress"], "bed_mesh": ["mesh_matrix", "mesh_min", "mesh_max"], "toolhead": ["extruder"` (simplyprint.py:540) | Nothing of it is read directly; only through the messages above | `UNCHECKED` |
| `klipper:exclude_object.current_object` | `sub_objs = { "display_status": ["progress"], "virtual_sdcard": ["file_position", "progress"], "bed_mesh": ["mesh_matrix", "mesh_min", "mesh_max"], "toolhead": ["extruder"` (simplyprint.py:540) | Nothing of it is read directly; only through the messages above | `UNCHECKED` |
| `klipper:exclude_object.excluded_objects` | `sub_objs = { "display_status": ["progress"], "virtual_sdcard": ["file_position", "progress"], "bed_mesh": ["mesh_matrix", "mesh_min", "mesh_max"], "toolhead": ["extruder"` (simplyprint.py:540) | Nothing of it is read directly; only through the messages above | `UNCHECKED` |
| `klipper:exclude_object.objects` | `sub_objs = { "display_status": ["progress"], "virtual_sdcard": ["file_position", "progress"], "bed_mesh": ["mesh_matrix", "mesh_min", "mesh_max"], "toolhead": ["extruder"` (simplyprint.py:540) | Nothing of it is read directly; only through the messages above | `UNCHECKED` |
| `klipper:gcode_move.gcode_position` | `sub_objs = { "display_status": ["progress"], "virtual_sdcard": ["file_position", "progress"], "bed_mesh": ["mesh_matrix", "mesh_min", "mesh_max"], "toolhead": ["extruder"` (simplyprint.py:540) | Nothing of it is read directly; only through the messages above | `UNCHECKED` |
| `klipper:query heaters` | `query = await self.klippy_apis.query_objects({"heaters": None}, None)` (simplyprint.py:539) | Nothing of it is read directly; only through the messages above | `UNCHECKED` |
| `klipper:toolhead.extruder` | `sub_objs = { "display_status": ["progress"], "virtual_sdcard": ["file_position", "progress"], "bed_mesh": ["mesh_matrix", "mesh_min", "mesh_max"], "toolhead": ["extruder"` (simplyprint.py:540) | Nothing of it is read directly; only through the messages above | `UNCHECKED` |
| `klipper:virtual_sdcard.file_position` | `sub_objs = { "display_status": ["progress"], "virtual_sdcard": ["file_position", "progress"], "bed_mesh": ["mesh_matrix", "mesh_min", "mesh_max"], "toolhead": ["extruder"` (simplyprint.py:540) | Nothing of it is read directly; only through the messages above | `UNCHECKED` |
| `klipper:virtual_sdcard.progress` | `sub_objs = { "display_status": ["progress"], "virtual_sdcard": ["file_position", "progress"], "bed_mesh": ["mesh_matrix", "mesh_min", "mesh_max"], "toolhead": ["extruder"` (simplyprint.py:540) | Nothing of it is read directly; only through the messages above | `UNCHECKED` |

## 3. Message types and demands Moonraker accepts, and the conditions in front of them

| item | what Moonraker does | what our hub does now | status |
|---|---|---|---|
| `accepted:complete_setup` | Saves `printer_id`, switches its URL to `{ep}/{printer_id}/{token}` (simplyprint.py:309) | Never sent (module doc, simplyprint.rs:24-25) | `UNCHECKED` |
| `accepted:connected` | Marks the link up, resets the reconnect delay to 1 s, then `_push_initial_state` and starts the ping timer (simplyprint.py:259) | Sent first, right after the upgrade, with `name` and `interval` (simplyprint.rs:108-111) | `UNCHECKED` |
| `accepted:demand` | Pops `demand` from the data and handles it in `_process_demand`; the rest of the data is its args (simplyprint.py:320) | Sent through `demand()`, which adds the `demand` key to the data (simplyprint.rs:154-157) | `UNCHECKED` |
| `accepted:error` | Logs the data, sets `reconnect_delay` 30 s, drops the reconnect token (simplyprint.py:282) | Never sent | `UNCHECKED` |
| `accepted:interval_change` | Sets the report intervals (milliseconds, divided by 1000) (simplyprint.py:326) | Never sent | `UNCHECKED` |
| `accepted:new_token` | Saves the token and switches its URL to SimplyPrint's own `{ep}/...` form (simplyprint.py:286) | Never sent (module doc, simplyprint.rs:24-25) | `UNCHECKED` |
| `accepted:pong` | Answers with `latency` (round trip since its last `ping`) (simplyprint.py:329) | Sent (`{"type": "pong"}`, no data) when `ping` arrives (simplyprint.rs:132) | `UNCHECKED` |
| `accepted:demand:api_restart` | Restarts the Moonraker service (simplyprint.py:382) | Never sent | `UNCHECKED` |
| `accepted:demand:api_shutdown` | Stops the Moonraker service (simplyprint.py:384) | Never sent | `UNCHECKED` |
| `accepted:demand:cancel` | Cancels the print if `printing` or `paused` (state becomes `cancelling`) (simplyprint.py:341) | Sent as `demand` `cancel` with no args, when the job control says so (simplyprint.rs:359-366) | `UNCHECKED` |
| `accepted:demand:file` | Fetches `url` (GET), puts it in the gcodes folder; `auto_start` starts it; tracks `job_id` (simplyprint.py:358) | Sent for a job the hub gives out (simplyprint.rs:352-358) | `UNCHECKED` |
| `accepted:demand:gcode` | Joins `list` with newlines, runs it in Klipper; answers `gcode_executed` if there is an `identifier` (simplyprint.py:348) | Sent for the next queued command, one at a time, only while the hub's state is Idle and no job is running (simplyprint.rs:338-350) | `UNCHECKED` |
| `accepted:demand:pause` | Pauses the print if `printing` (state becomes `pausing`) (simplyprint.py:341) | Sent as `demand` `pause` with no args, when the job control says so (simplyprint.rs:359-366) | `UNCHECKED` |
| `accepted:demand:psu_off` | Switches its configured power device off (simplyprint.py:388) | Never sent | `UNCHECKED` |
| `accepted:demand:psu_on` | Switches its configured power device on (simplyprint.py:386) | Never sent | `UNCHECKED` |
| `accepted:demand:resume` | Resumes the print if `paused` (state becomes `resuming`) (simplyprint.py:341) | Sent as `demand` `resume` with no args, when the job control says so (simplyprint.rs:359-366) | `UNCHECKED` |
| `accepted:demand:start_print` | Starts the file fetched earlier, if Klipper is connected and `operational` (simplyprint.py:368) | Never sent: the hub sends `file` with `auto_start` 1 instead | `UNCHECKED` |
| `accepted:demand:system_restart` | Reboots the machine (`machine.reboot`) (simplyprint.py:376) | Never sent | `UNCHECKED` |
| `accepted:demand:system_shutdown` | Shuts the machine down (`machine.shutdown`) (simplyprint.py:379) | Never sent | `UNCHECKED` |
| `accepted:demand:terminal` | Switches the terminal reports (`term_update`) on or off (simplyprint.py:345) | Sent once on connecting with `enabled: true` (simplyprint.rs:112) | `UNCHECKED` |
| `accepted:demand:test_webcam` | Tests the snapshot URL, answers `webcam_status` (simplyprint.py:390) | Never sent | `UNCHECKED` |
| `accepted:demand:webcam_snapshot` | Posts a snapshot to `endpoint` (`id`) or sends a `stream` after `timer` ms (simplyprint.py:356) | Never sent | `UNCHECKED` |
| `accepted:demand:cancel#klippy-connected` | `if demand in ["pause", "resume", "cancel"]:` (simplyprint.py:341) | Not known to the hub: sent without looking at Klippy's link | `UNCHECKED` |
| `accepted:demand:cancel#state=paused` | `elif action == "cancel":` (simplyprint.py:513) | The hub does not look at its state here (simplyprint.rs:359-366); decided in `agent_job` (not read for this table) | `UNCHECKED` |
| `accepted:demand:cancel#state=printing` | `elif action == "cancel":` (simplyprint.py:513) | The hub does not look at its state here (simplyprint.rs:359-366); decided in `agent_job` (not read for this table) | `UNCHECKED` |
| `accepted:demand:file#url-must-be-a-string` | `elif demand == "file":` (simplyprint.py:358) | A string (simplyprint.rs:356) | `UNCHECKED` |
| `accepted:demand:gcode#identifier-answers-with-gcode_executed` | `if ident is not None:` (simplyprint.py:413) | Always sent with an `identifier` (simplyprint.rs:348) and waited for (simplyprint.rs:273) | `UNCHECKED` |
| `accepted:demand:gcode#klippy-connected` | `elif demand == "gcode":` (simplyprint.py:348) | Not known to the hub: it sends `gcode` when it believes the state is Idle (simplyprint.rs:338-341). If Moonraker drops the demand, no `gcode_executed` comes and `printer.command` stays set (simplyprint.rs:273); no timeout in this file | `UNCHECKED` |
| `accepted:demand:gcode#list-nonempty` | `elif demand == "gcode":` (simplyprint.py:348) | Lines come from `gcode.lines()` (simplyprint.rs:345); an empty command would be dropped by Moonraker without an answer; not checked here | `UNCHECKED` |
| `accepted:demand:pause#klippy-connected` | `if demand in ["pause", "resume", "cancel"]:` (simplyprint.py:341) | Not known to the hub: sent without looking at Klippy's link | `UNCHECKED` |
| `accepted:demand:pause#state=printing` | `if action == "pause":` (simplyprint.py:504) | The hub does not look at its state here (simplyprint.rs:359-366); decided in `agent_job` (not read for this table) | `UNCHECKED` |
| `accepted:demand:resume#klippy-connected` | `if demand in ["pause", "resume", "cancel"]:` (simplyprint.py:341) | Not known to the hub: sent without looking at Klippy's link | `UNCHECKED` |
| `accepted:demand:resume#state=paused` | `elif action == "resume":` (simplyprint.py:508) | The hub does not look at its state here (simplyprint.rs:359-366); decided in `agent_job` (not read for this table) | `UNCHECKED` |
| `accepted:demand:start_print#klippy-connected` | `elif demand == "start_print":` (simplyprint.py:368) | n/a: never sent | `UNCHECKED` |
| `accepted:demand:start_print#state=operational` | `elif demand == "start_print":` (simplyprint.py:368) | n/a: never sent | `UNCHECKED` |

## 4. Fields Moonraker reads

Intervals are milliseconds; Moonraker divides by 1000 (simplyprint.py:446).

| item | what Moonraker does | what our hub does now | status |
|---|---|---|---|
| `accepted:complete_setup.printer_id` | `printer_id = data.get("printer_id")` (simplyprint.py:313) | Never sent | `UNCHECKED` |
| `accepted:connected.in_setup` | `if data.get("in_setup", 0) == 1:` (simplyprint.py:264) | Never sent | `UNCHECKED` |
| `accepted:connected.interval` | `interval = data.get("interval")` (simplyprint.py:272) | Sent: job 2000, temps 2000, temps_target 1000, cpu 30000, ping 2000 (milliseconds) (simplyprint.rs:110) | `UNCHECKED` |
| `accepted:connected.interval.ai` | `"cpu": 10.,` (simplyprint.py:89) | Not sent (Moonraker keeps its default 0) | `UNCHECKED` |
| `accepted:connected.interval.cpu` | `"temps_target": .25,` (simplyprint.py:88) | Sent as 30000 ms (simplyprint.rs:110) | `UNCHECKED` |
| `accepted:connected.interval.job` | `self.intervals: Dict[str, float] = { "job": 1., "temps": 1., "temps_target": .25, "cpu": 10., "ai": 0., "ping": 20., }` (simplyprint.py:85) | Sent as 2000 ms (simplyprint.rs:110) | `UNCHECKED` |
| `accepted:connected.interval.ping` | `"cpu": 10.,` (simplyprint.py:89) | Sent as 2000 ms (simplyprint.rs:110) | `UNCHECKED` |
| `accepted:connected.interval.temps` | `"job": 1.,` (simplyprint.py:86) | Sent as 2000 ms (simplyprint.rs:110) | `UNCHECKED` |
| `accepted:connected.interval.temps_target` | `"temps": 1.,` (simplyprint.py:87) | Sent as 1000 ms (simplyprint.rs:110) | `UNCHECKED` |
| `accepted:connected.name` | `name = data.get("name")` (simplyprint.py:276) | Sent: the printer's name in the hub (simplyprint.rs:109) | `UNCHECKED` |
| `accepted:connected.reconnect_token` | `self.reconnect_token = data.get("reconnect_token")` (simplyprint.py:275) | Never sent | `UNCHECKED` |
| `accepted:connected.short_id` | `if "short_id" in data:` (simplyprint.py:268) | Never sent | `UNCHECKED` |
| `accepted:demand.demand` | The name of the demand; popped from the data before the args are used (simplyprint.py:324) | Added by `demand()` (simplyprint.rs:155) | `UNCHECKED` |
| `accepted:demand:file.auto_start` | `start = bool(args.get("auto_start", 0))` (simplyprint.py:363) | Sent as `1` (simplyprint.rs:356) | `UNCHECKED` |
| `accepted:demand:file.job_id` | `self._current_job_id = int(args.get("job_id", 0)) or None` (simplyprint.py:366) | Sent as the hub's job id, a number (simplyprint.rs:356) | `UNCHECKED` |
| `accepted:demand:file.url` | `url: Optional[str] = args.get("url")` (simplyprint.py:359) | Sent: `http://<Host header>/simplyprint/<token>/jobs/<id>/file` (simplyprint.rs:82-87, 356) | `UNCHECKED` |
| `accepted:demand:gcode.identifier` | `ident: Optional[str] = args.get("identifier", None)` (simplyprint.py:352) | Sent: the command id as text (simplyprint.rs:348) | `UNCHECKED` |
| `accepted:demand:gcode.list` | `script_list: List[str] = args.get("list", [])` (simplyprint.py:351) | Sent: the command's lines (simplyprint.rs:345-348) | `UNCHECKED` |
| `accepted:demand:terminal.enabled` | `if "enabled" in args:` (simplyprint.py:346) | Sent as `true` (simplyprint.rs:112) | `UNCHECKED` |
| `accepted:demand:webcam_snapshot.endpoint` | `url = payload.get("endpoint", SP_SNAPSHOT_URL)` (simplyprint.py:1443) | Never sent | `UNCHECKED` |
| `accepted:demand:webcam_snapshot.id` | `uid: Optional[str] = payload.get("id")` (simplyprint.py:1439) | Never sent | `UNCHECKED` |
| `accepted:demand:webcam_snapshot.timer` | `timer: Optional[int] = payload.get("timer")` (simplyprint.py:1440) | Never sent | `UNCHECKED` |
| `accepted:interval_change.ai` | `"cpu": 10.,` (simplyprint.py:89) | Never sent | `UNCHECKED` |
| `accepted:interval_change.cpu` | `"temps_target": .25,` (simplyprint.py:88) | Never sent | `UNCHECKED` |
| `accepted:interval_change.job` | `self.intervals: Dict[str, float] = { "job": 1., "temps": 1., "temps_target": .25, "cpu": 10., "ai": 0., "ping": 20., }` (simplyprint.py:85) | Never sent | `UNCHECKED` |
| `accepted:interval_change.ping` | `"cpu": 10.,` (simplyprint.py:89) | Never sent | `UNCHECKED` |
| `accepted:interval_change.temps` | `"job": 1.,` (simplyprint.py:86) | Never sent | `UNCHECKED` |
| `accepted:interval_change.temps_target` | `"temps": 1.,` (simplyprint.py:87) | Never sent | `UNCHECKED` |
| `accepted:new_token.no_exist` | `if data.get("no_exist", False) is True and self.is_set_up:` (simplyprint.py:290) | Never sent | `UNCHECKED` |
| `accepted:new_token.short_id` | `if "short_id" in data:` (simplyprint.py:301) | Never sent | `UNCHECKED` |
| `accepted:new_token.token` | `token: Optional[str] = data.get("token")` (simplyprint.py:293) | Never sent | `UNCHECKED` |

## 5. Connection: configuration, addresses, headers, timeouts, keep-alive, reconnecting

Constants (`const:`) are Moonraker's own values (endpoints `PROD_ENDPOINT`/`TEST_ENDPOINT`, ambient and layer values, snapshot URLs); with `url` configured the endpoints are not used.

| item | what Moonraker does | what our hub does now | status |
|---|---|---|---|
| `accepted-envelope:data` | `data: Optional[Dict[str, Any]] = packet.get("data")` (simplyprint.py:258) | Sent with `connected` and every `demand`; `pong` has no `data` (simplyprint.rs:132) | `UNCHECKED` |
| `accepted-envelope:type` | `event: str = packet.get("type", "")` (simplyprint.py:257) | Sent in every message (simplyprint.rs:108, 132, 156) | `UNCHECKED` |
| `config:ambient_sensor` | `self._sensor_name: str = config.get("ambient_sensor", "")` (simplyprint.py:1234) | n/a to the hub: Moonraker-side; `ambient` messages are ignored | `UNCHECKED` |
| `config:filament_sensor` | `fsensor = config.get("filament_sensor", None)` (simplyprint.py:138) | n/a to the hub: Moonraker-side; `filament_sensor` messages are ignored | `UNCHECKED` |
| `config:get_prefix_sections` | `power_pfx = config.get_prefix_sections("power ")` (simplyprint.py:132) | n/a to the hub: Moonraker-side (finds its power device) | `UNCHECKED` |
| `config:has_section` | `if not config.has_section(f"power {self.power_id}"):` (simplyprint.py:125) | n/a to the hub: Moonraker-side | `UNCHECKED` |
| `config:power_device` | `power_id: Optional[str] = config.get("power_device", None)` (simplyprint.py:120) | n/a to the hub: Moonraker-side; never read or sent (`psu_on`/`psu_off` are never sent) | `UNCHECKED` |
| `config:url` | `connect_url = config.get("url", None)` (simplyprint.py:112) | The hub serves `/simplyprint/<token>` (simplyprint.rs:43) and the doc comment tells the user to set `url: ws://<host>:8460/simplyprint/<token>` (simplyprint.rs:5-8) | `UNCHECKED` |
| `config:use_test_endpoint` | `self.test = config.get("use_test_endpoint", False)` (simplyprint.py:111) | n/a to the hub: Moonraker's own option; with `url` set the endpoints are not used | `UNCHECKED` |
| `config:webcam_name` | `self.webcam_name = config.get("webcam_name", "")` (simplyprint.py:1385) | n/a to the hub: Moonraker-side; the hub has no webcam handling here | `UNCHECKED` |
| `const:AMBIENT_CHECK_TIME` | `AMBIENT_CHECK_TIME = 5. * 60.` (simplyprint.py:1211) | n/a to the hub: Moonraker's own constant | `UNCHECKED` |
| `const:COMPONENT_VERSION` | `COMPONENT_VERSION = "0.0.1"` (simplyprint.py:50) | n/a to the hub: Moonraker's own constant | `UNCHECKED` |
| `const:CONNECTION_ERROR_LOG_TIME` | `CONNECTION_ERROR_LOG_TIME = 60.` (simplyprint.py:55) | n/a to the hub: Moonraker's own constant | `UNCHECKED` |
| `const:FALLBACK_URL` | `FALLBACK_URL = "http://127.0.0.1:8080/?action=snapshot"` (simplyprint.py:1375) | n/a to the hub: Moonraker's own constant | `UNCHECKED` |
| `const:INITIAL_AMBIENT` | `INITIAL_AMBIENT = 85` (simplyprint.py:1210) | n/a to the hub: Moonraker's own constant | `UNCHECKED` |
| `const:PRE_SETUP_EVENTS` | `PRE_SETUP_EVENTS = [ "connection", "state_change", "shutdown", "machine_data", "firmware", "ping" ]` (simplyprint.py:56) | n/a to the hub: Moonraker's own constant | `UNCHECKED` |
| `const:PROD_ENDPOINT` | `PROD_ENDPOINT = f"wss://ws.simplyprint.io/{SP_VERSION}/p"` (simplyprint.py:53) | n/a to the hub: Moonraker's own constant | `UNCHECKED` |
| `const:SAMPLE_CHECK_TIME` | `SAMPLE_CHECK_TIME = 20.` (simplyprint.py:1213) | n/a to the hub: Moonraker's own constant | `UNCHECKED` |
| `const:SP_SNAPSHOT_URL` | `SP_SNAPSHOT_URL = "https://api.simplyprint.io/jobs/ReceiveSnapshot"` (simplyprint.py:1376) | n/a to the hub: Moonraker's own constant | `UNCHECKED` |
| `const:SP_VERSION` | `SP_VERSION = "0.1"` (simplyprint.py:51) | n/a to the hub: Moonraker's own constant | `UNCHECKED` |
| `const:TARGET_CHECK_TIME` | `TARGET_CHECK_TIME = 60. * 60.` (simplyprint.py:1212) | n/a to the hub: Moonraker's own constant | `UNCHECKED` |
| `const:TEST_ENDPOINT` | `TEST_ENDPOINT = f"wss://testws3.simplyprint.io/{SP_VERSION}/p"` (simplyprint.py:52) | n/a to the hub: Moonraker's own constant | `UNCHECKED` |
| `gate:not self._check_setup_event(evt_name)` | `if ( not self.connected or self.ws is None or self.ws.protocol is None or not self._check_setup_event(evt_name) ):` (simplyprint.py:1123) | With `url` configured Moonraker counts as set up (simplyprint.py:112-115), so every event passes; the hub never sends `in_setup` | `UNCHECKED` |
| `gate:not self.connected` | `if ( not self.connected or self.ws is None or self.ws.protocol is None or not self._check_setup_event(evt_name) ):` (simplyprint.py:1123) | n/a to the hub: Moonraker sends nothing without a link | `UNCHECKED` |
| `gate:self.ws is None` | `if ( not self.connected or self.ws is None or self.ws.protocol is None or not self._check_setup_event(evt_name) ):` (simplyprint.py:1123) | n/a to the hub: Moonraker sends nothing without a link | `UNCHECKED` |
| `gate:self.ws.protocol is None` | `if ( not self.connected or self.ws is None or self.ws.protocol is None or not self._check_setup_event(evt_name) ):` (simplyprint.py:1123) | n/a to the hub: Moonraker sends nothing without a link | `UNCHECKED` |
| `header:Accept: image/jpeg` | `headers = {"Accept": "image/jpeg"}` (simplyprint.py:1418, simplyprint.py:1428) | n/a to the hub: Moonraker's request to its webcam | `UNCHECKED` |
| `header:Accept: text/plain,applicaton/octet-stream` | `accept = "text/plain,applicaton/octet-stream"` (simplyprint.py:1504) | Sent by Moonraker when it fetches the `file` URL; the hub's `crate::files::job_file` answers (simplyprint.rs:55); whether it looks at `Accept` is not read for this table | `UNCHECKED` |
| `header:Content-Type: application/x-www-form-urlencoded` | `headers = { "User-Agent": "Mozilla/5.0", "Content-Type": "application/x-www-form-urlencoded" }` (simplyprint.py:1445) | n/a to the hub: same (snapshot POST) | `UNCHECKED` |
| `header:User-Agent: Mozilla/5.0` | `headers = { "User-Agent": "Mozilla/5.0", "Content-Type": "application/x-www-form-urlencoded" }` (simplyprint.py:1445) | n/a to the hub: Moonraker's POST to the snapshot endpoint (`webcam_snapshot` is never sent) | `UNCHECKED` |
| `interval:ai` | `"cpu": 10.,` (simplyprint.py:89) | Not sent | `UNCHECKED` |
| `interval:cpu` | `"temps_target": .25,` (simplyprint.py:88) | The hub asks for 30000 ms (simplyprint.rs:110) | `UNCHECKED` |
| `interval:job` | `self.intervals: Dict[str, float] = { "job": 1., "temps": 1., "temps_target": .25, "cpu": 10., "ai": 0., "ping": 20., }` (simplyprint.py:85) | The hub asks for 2000 ms (simplyprint.rs:110) | `UNCHECKED` |
| `interval:ping` | `"cpu": 10.,` (simplyprint.py:89) | The hub asks for 2000 ms (simplyprint.rs:110) | `UNCHECKED` |
| `interval:temps` | `"job": 1.,` (simplyprint.py:86) | The hub asks for 2000 ms (simplyprint.rs:110) | `UNCHECKED` |
| `interval:temps_target` | `"temps": 1.,` (simplyprint.py:87) | The hub asks for 1000 ms (simplyprint.rs:110) | `UNCHECKED` |
| `msg:invalid-JSON-ignored` | `except jsonw.JSONDecodeError:` (simplyprint.py:254) | The hub skips invalid JSON (`continue`, simplyprint.rs:130) | `UNCHECKED` |
| `msg:json-text-frame` | `await self.ws.write_message(jsonw.dumps(packet))` (simplyprint.py:1145) | The hub sends `value.to_string()` as a Text frame (simplyprint.rs:150-152) | `UNCHECKED` |
| `msg:text-frames-only` | `if isinstance(message, str):` (simplyprint.py:227) | The hub reads Text only (simplyprint.rs:129) and sends Text (simplyprint.rs:151) | `UNCHECKED` |
| `ping:_handle_sp_ping` | `self.ping_sp_timer = self.eventloop.register_timer(self._handle_sp_ping)` (simplyprint.py:101) | Moonraker pings every `intervals[ping]`; the hub answers each `ping` with `pong` (simplyprint.rs:132) and asks for 2000 ms (simplyprint.rs:110) | `UNCHECKED` |
| `ping:_last_ping_received` | `self._last_ping_received: float = 0.` (simplyprint.py:106) | n/a to the hub: used for a log line only | `UNCHECKED` |
| `ping:_last_sp_ping` | `self._last_sp_ping: float = 0.` (simplyprint.py:100) | n/a to the hub: Moonraker's clock for the `latency` it sends | `UNCHECKED` |
| `ping:on_ping` | `setattr(self.ws, "on_ping", self._on_ws_ping)` (simplyprint.py:205) | Moonraker notes WebSocket ping frames from the server (only for a log line); the hub sends no ping frames (none in simplyprint.rs) | `UNCHECKED` |
| `ping:ping_sp_timer.start` | `self.ping_sp_timer.start()` (simplyprint.py:281) | Moonraker starts pinging after `connected`; the hub sends `connected` first (simplyprint.rs:108) | `UNCHECKED` |
| `ping:ping_sp_timer.stop` | `self.ping_sp_timer.stop()` (simplyprint.py:230, simplyprint.py:1171) | n/a to the hub: Moonraker-side timer | `UNCHECKED` |
| `pre-setup:connection` | `PRE_SETUP_EVENTS = [ "connection", "state_change", "shutdown", "machine_data", "firmware", "ping" ]` (simplyprint.py:56) | Moonraker sends it even before setup; the hub never sends `in_setup`, so all events pass | `UNCHECKED` |
| `pre-setup:firmware` | `PRE_SETUP_EVENTS = [ "connection", "state_change", "shutdown", "machine_data", "firmware", "ping" ]` (simplyprint.py:56) | Moonraker sends it even before setup; the hub never sends `in_setup`, so all events pass | `UNCHECKED` |
| `pre-setup:machine_data` | `PRE_SETUP_EVENTS = [ "connection", "state_change", "shutdown", "machine_data", "firmware", "ping" ]` (simplyprint.py:56) | Moonraker sends it even before setup; the hub never sends `in_setup`, so all events pass | `UNCHECKED` |
| `pre-setup:ping` | `PRE_SETUP_EVENTS = [ "connection", "state_change", "shutdown", "machine_data", "firmware", "ping" ]` (simplyprint.py:56) | Moonraker sends it even before setup; the hub never sends `in_setup`, so all events pass | `UNCHECKED` |
| `pre-setup:shutdown` | `PRE_SETUP_EVENTS = [ "connection", "state_change", "shutdown", "machine_data", "firmware", "ping" ]` (simplyprint.py:56) | Moonraker sends it even before setup; the hub never sends `in_setup`, so all events pass | `UNCHECKED` |
| `pre-setup:state_change` | `PRE_SETUP_EVENTS = [ "connection", "state_change", "shutdown", "machine_data", "firmware", "ping" ]` (simplyprint.py:56) | Moonraker sends it even before setup; the hub never sends `in_setup`, so all events pass | `UNCHECKED` |
| `reconnect:reconnect_delay=1.` | `self.reconnect_delay: float = 1.` (simplyprint.py:98, simplyprint.py:279) | Moonraker waits 1 s after the link drops; the hub sends no `error`, so it stays 1 s | `UNCHECKED` |
| `reconnect:reconnect_delay=30.` | `self.reconnect_delay = 30.` (simplyprint.py:284) | Set by an `error` message; the hub never sends `error` | `UNCHECKED` |
| `reconnect:reconnect_token=None` | `self.reconnect_token = None` (simplyprint.py:262, simplyprint.py:285) | n/a to the hub: Moonraker-side | `UNCHECKED` |
| `reconnect:reconnect_token=data.get("reconnect_token")` | `self.reconnect_token = data.get("reconnect_token")` (simplyprint.py:275) | Never sent by the hub | `UNCHECKED` |
| `reconnect:sleep(reconnect_delay)` | `await asyncio.sleep(self.reconnect_delay)` (simplyprint.py:221) | n/a to the hub: Moonraker-side wait between connection attempts | `UNCHECKED` |
| `sent-envelope:data` | `packet = {"type": evt_name, "data": data}` (simplyprint.py:1139) | Read: `packet["data"]` (simplyprint.rs:135) | `UNCHECKED` |
| `sent-envelope:type` | `packet = {"type": evt_name, "data": data}` (simplyprint.py:1139) | Read: `packet["type"]` (simplyprint.rs:131) | `UNCHECKED` |
| `timeout:connect_timeout=15.` | `url, connect_timeout=15.,` (simplyprint.py:203, simplyprint.py:1510) | Moonraker's time limit for connecting its WebSocket (simplyprint.py:203) and for connecting to the file URL (simplyprint.py:1510); the hub sets no time limit in simplyprint.rs | `UNCHECKED` |
| `timeout:request_timeout=3600.` | `request_timeout=3600.,` (simplyprint.py:1511) | The hub's `job_file` (`files.rs`) serves the file; the hub sets no timeout in simplyprint.rs | `UNCHECKED` |
| `timeout:wait_for(self._print_request_event.wait(), 1.)` | `await asyncio.wait_for(self._print_request_event.wait(), 1.)` (simplyprint.py:521) | n/a to the hub: Moonraker-side | `UNCHECKED` |
| `timeout:wait_for(self.connection_task, 2.)` | `await asyncio.wait_for(self.connection_task, 2.)` (simplyprint.py:1182) | n/a to the hub: Moonraker-side | `UNCHECKED` |
| `timeout:wait_for(self.print_ready_event.wait(), 10.)` | `await asyncio.wait_for(self.print_ready_event.wait(), 10.)` (simplyprint.py:1560) | n/a to the hub: Moonraker-side wait for `operational` before a pending file starts | `UNCHECKED` |
| `url:connect_url` | `self.connect_url = connect_url` (simplyprint.py:114) | The `url` option is used as is, so the hub's route is what the user puts in it (simplyprint.rs:43) | `UNCHECKED` |
| `url:{ep}/0/0` | `self.connect_url = f"{ep}/0/0"` (simplyprint.py:436) | Not served: only used when no `url` is configured (simplyprint.py:112-117); the hub's route has a single `<token>` segment | `UNCHECKED` |
| `url:{ep}/0/{token}` | `self.connect_url = f"{ep}/0/{token}"` (simplyprint.py:439) | Not served: only used when no `url` is configured (simplyprint.py:112-117); the hub's route has a single `<token>` segment | `UNCHECKED` |
| `url:{ep}/{printer_id}/{token}` | `self.connect_url = f"{ep}/{printer_id}/{token}"` (simplyprint.py:442) | Not served: only used when no `url` is configured (simplyprint.py:112-117); the hub's route has a single `<token>` segment | `UNCHECKED` |
| `url:{self.connect_url}/{self.reconnect_token}` | `url = f"{self.connect_url}/{self.reconnect_token}"` (simplyprint.py:197) | Route `/simplyprint/{token}/{*rest}` accepts it (simplyprint.rs:48); the hub never sends a `reconnect_token`, so Moonraker never uses this form | `UNCHECKED` |
| `ws:close(1001, "Client Shutdown")` | `self.ws.close(1001, "Client Shutdown")` (simplyprint.py:1176) | The hub ends the session when the socket closes (`break`, simplyprint.rs:128); no close code is read | `UNCHECKED` |
| `ws:read_message` | `message = await self.ws.read_message()` (simplyprint.py:226) | The hub sends Text frames only (simplyprint.rs:151) | `UNCHECKED` |
| `ws:websocket_connect` | `self.ws = await tornado.websocket.websocket_connect( url, connect_timeout=15., )` (simplyprint.py:202) | The hub accepts a WebSocket upgrade on the route; an unknown token gets 401 (simplyprint.rs:67-88); no headers or sub-protocol are read except `host` | `UNCHECKED` |
| `ws:write_message` | `await self.ws.write_message(jsonw.dumps(packet))` (simplyprint.py:1145) | The hub reads Text frames only; binary is skipped (simplyprint.rs:129) | `UNCHECKED` |

## 6. What we send or read that Moonraker does not

Mechanically listed by `scripts/klipper-audit-check.py` from `crates/hub/src/simplyprint.rs` (the code before
`#[cfg(test)] mod tests`): every message type, demand and field the hub sends, every type, field and value it reads.
Each must be something Moonraker sends or accepts, or be named here with why. Everything else in the hub's
code is already Moonraker's (the checker counts them).

| item | where | why it is not Moonraker's | status |
|---|---|---|---|
| `hub-reads:mesh_data.probed_matrix` | simplyprint.rs:215 | The hub reads Klipper's `probed_matrix`; Moonraker sends `mesh_matrix`, `mesh_min`, `mesh_max` (the `bed_mesh` fields it subscribes to, simplyprint.py:543, 984). A key Moonraker does not send: the mesh can never arrive. A gap, not an excuse | `UNCHECKED` |
| `hub-route:/simplyprint/{token}` | simplyprint.rs:43 | The address the user puts in Moonraker's `url` option; Moonraker uses `url` as given (simplyprint.py:112-114, 195) | `UNCHECKED` |
| `hub-route:/simplyprint/{token}/{*rest}` | simplyprint.rs:48 | Takes the form Moonraker uses after a `reconnect_token` (simplyprint.py:196-197); the hub never sends one | `UNCHECKED` |
| `hub-route:/simplyprint/{token}/jobs/{job}/file` | simplyprint.rs:45 | The URL the hub puts in the `file` demand; Moonraker fetches whatever `url` it gets (simplyprint.py:359, 1508) | `UNCHECKED` |
| `hub-header:host` | simplyprint.rs:82 | Read from the upgrade request to build the file URL; not a message field. Moonraker's WebSocket client sends it (tornado), not visible in simplyprint.py | `UNCHECKED` |
| `hub-status:UNAUTHORIZED` | simplyprint.rs:77 | The hub refuses an unknown token with 401; Moonraker's code only logs a failed connect and retries after `reconnect_delay` (simplyprint.py:210-215, 221) | `UNCHECKED` |

## 7. Not covered by this audit

- The SimplyPrint server itself (closed): what it sends beyond what Moonraker accepts, and how it
  reacts to what Moonraker sends.
- The Moonraker code outside `components/simplyprint.py` (`job_state`, `klippy_apis`, `file_manager`,
  `http_client`, `webcam`, `machine`): what they do is only cited where simplyprint.py calls them.
- The HTTP side of the file fetch (`crates/hub/src/files.rs`, `job_file`): not read for this table.
- How the hub decides on jobs and commands (`crate::agent_job`, `store.next_command`): not read for this table.
- The shape of values (e.g. `temps` pairs, `time` units): described in the rows, not extracted.
- Moonraker's logging, ambient detection and layer detection logic: only the messages they send are listed.
