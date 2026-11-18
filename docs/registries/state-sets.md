# State sets

<!-- Generated from crates/device/src/registry.rs. Do not edit: run `MC_WRITE_REGISTRIES=1 cargo test -p maker-connect-device --test registry_docs`. -->

## marlin

| State | Accepts | |
|---|---|---|
| `NONE` | - | STATE_NONE 0, "Offline". |
| `OPEN_SERIAL` | - | STATE_OPEN_SERIAL 1, "Opening serial connection". |
| `DETECT_SERIAL` | - | STATE_DETECT_SERIAL 2, "Detecting serial connection". |
| `CONNECTING` | - | STATE_CONNECTING 3, "Connecting". |
| `OPERATIONAL` | mc.printer.gcode, core.command.set_nozzle_temperature, core.command.set_bed_temperature, core.command.set_speed, core.command.set_flow, mc.printer.fan, core.command.delete_file, core.command.home, core.command.move, core.command.move_z, core.command.move_e, core.command.disable_steppers, core.command.start_print, mc.printer.sd_list | STATE_OPERATIONAL 4, "Operational". |
| `STARTING` | mc.printer.gcode, core.command.set_nozzle_temperature, core.command.set_bed_temperature, core.command.set_speed, core.command.set_flow, mc.printer.fan, core.command.delete_file, core.command.pause_print, core.command.stop_print | STATE_STARTING 5, "Starting". |
| `PRINTING` | mc.printer.gcode, core.command.set_nozzle_temperature, core.command.set_bed_temperature, core.command.set_speed, core.command.set_flow, mc.printer.fan, core.command.delete_file, core.command.pause_print, core.command.stop_print | STATE_PRINTING 6, "Printing". |
| `PAUSED` | mc.printer.gcode, core.command.set_nozzle_temperature, core.command.set_bed_temperature, core.command.set_speed, core.command.set_flow, mc.printer.fan, core.command.delete_file, core.command.home, core.command.move, core.command.move_z, core.command.move_e, core.command.disable_steppers, core.command.pause_print, core.command.resume_print, core.command.stop_print | STATE_PAUSED 7, "Paused". |
| `PAUSING` | mc.printer.gcode, core.command.set_nozzle_temperature, core.command.set_bed_temperature, core.command.set_speed, core.command.set_flow, mc.printer.fan, core.command.delete_file, core.command.pause_print, core.command.stop_print | STATE_PAUSING 8, "Pausing". |
| `RESUMING` | mc.printer.gcode, core.command.set_nozzle_temperature, core.command.set_bed_temperature, core.command.set_speed, core.command.set_flow, mc.printer.fan, core.command.delete_file, core.command.pause_print, core.command.stop_print | STATE_RESUMING 9, "Resuming". |
| `FINISHING` | mc.printer.gcode, core.command.set_nozzle_temperature, core.command.set_bed_temperature, core.command.set_speed, core.command.set_flow, mc.printer.fan, core.command.delete_file, core.command.pause_print, core.command.stop_print | STATE_FINISHING 10, "Finishing". |
| `CLOSED` | - | STATE_CLOSED 11, "Offline". |
| `ERROR` | - | STATE_ERROR 12, "Error". |
| `CLOSED_WITH_ERROR` | - | STATE_CLOSED_WITH_ERROR 13, "Offline after error". |
| `TRANSFERING_FILE` | mc.printer.gcode, core.command.set_nozzle_temperature, core.command.set_bed_temperature, core.command.set_speed, core.command.set_flow, mc.printer.fan, core.command.delete_file, core.command.home, core.command.move, core.command.move_z, core.command.move_e, core.command.disable_steppers, mc.printer.sd_list | STATE_TRANSFERING_FILE 14, "Transferring file to SD". |
| `CANCELLING` | mc.printer.gcode, core.command.set_nozzle_temperature, core.command.set_bed_temperature, core.command.set_speed, core.command.set_flow, mc.printer.fan, core.command.delete_file, core.command.pause_print, core.command.stop_print | STATE_CANCELLING 15, "Cancelling". |

| From | To | Verb |
|---|---|---|

## labelmanager

| State | Accepts | |
|---|---|---|
| `DISCONNECTED` | - | Ours: no LabelManager found. |
| `IDLE` | mc.label.print | Ours: a LabelManager is there (labelle's `is_ready`, dymo_labeler.py:304). |
| `BUSY` | - | Ours: a label prints. |

| From | To | Verb |
|---|---|---|

## labelwriter

| State | Accepts | |
|---|---|---|
| `IDLE` | mc.label.print | Ours: no job runs; nothing is sent while idle. |
| `PRINTING` | - | Ours: a job runs (`jsOK`). |
| `OUT_OF_PAPER` | - | `jsPaperOut`, `com.dymo.out-of-paper-error`. |
| `ERROR` | - | `jsError`, `com.dymo.general-error`. |

| From | To | Verb |
|---|---|---|

## grbl

| State | Accepts | |
|---|---|---|
| `IDLE` | mc.laser.gcode, mc.laser.jog, mc.laser.home, mc.laser.unlock, mc.laser.set_origin, mc.laser.go_origin, mc.laser.read_settings, mc.laser.frame, mc.laser.pause, mc.laser.resume, mc.laser.stop, mc.laser.start | GRBL `Idle`. |
| `RUN` | mc.laser.gcode, mc.laser.jog, mc.laser.home, mc.laser.unlock, mc.laser.set_origin, mc.laser.go_origin, mc.laser.read_settings, mc.laser.frame, mc.laser.pause, mc.laser.resume, mc.laser.stop | GRBL `Run`. |
| `HOLD` | mc.laser.gcode, mc.laser.jog, mc.laser.home, mc.laser.unlock, mc.laser.set_origin, mc.laser.go_origin, mc.laser.read_settings, mc.laser.frame, mc.laser.pause, mc.laser.resume, mc.laser.stop | GRBL `Hold`. |
| `JOG` | mc.laser.gcode, mc.laser.jog, mc.laser.home, mc.laser.unlock, mc.laser.set_origin, mc.laser.go_origin, mc.laser.read_settings, mc.laser.frame, mc.laser.pause, mc.laser.resume, mc.laser.stop | GRBL `Jog`. |
| `ALARM` | mc.laser.gcode, mc.laser.jog, mc.laser.home, mc.laser.unlock, mc.laser.set_origin, mc.laser.go_origin, mc.laser.read_settings, mc.laser.frame, mc.laser.pause, mc.laser.resume, mc.laser.stop | GRBL `Alarm`. |
| `DOOR` | mc.laser.gcode, mc.laser.jog, mc.laser.home, mc.laser.unlock, mc.laser.set_origin, mc.laser.go_origin, mc.laser.read_settings, mc.laser.frame, mc.laser.pause, mc.laser.resume, mc.laser.stop | GRBL `Door`. |
| `CHECK` | mc.laser.gcode, mc.laser.jog, mc.laser.home, mc.laser.unlock, mc.laser.set_origin, mc.laser.go_origin, mc.laser.read_settings, mc.laser.frame, mc.laser.pause, mc.laser.resume, mc.laser.stop | GRBL `Check`. |
| `HOME` | mc.laser.gcode, mc.laser.jog, mc.laser.home, mc.laser.unlock, mc.laser.set_origin, mc.laser.go_origin, mc.laser.read_settings, mc.laser.frame, mc.laser.pause, mc.laser.resume, mc.laser.stop | GRBL `Home`. |
| `SLEEP` | mc.laser.gcode, mc.laser.jog, mc.laser.home, mc.laser.unlock, mc.laser.set_origin, mc.laser.go_origin, mc.laser.read_settings, mc.laser.frame, mc.laser.pause, mc.laser.resume, mc.laser.stop | GRBL `Sleep`. |
| `UNKNOWN` | mc.laser.gcode, mc.laser.jog, mc.laser.home, mc.laser.unlock, mc.laser.set_origin, mc.laser.go_origin, mc.laser.read_settings, mc.laser.frame, mc.laser.pause, mc.laser.resume, mc.laser.stop, mc.laser.start | A status LaserFoundry does not know (`Unknown`). |

| From | To | Verb |
|---|---|---|

## scale

| State | Accepts | |
|---|---|---|
| `FAULT` | - | 0x01 "Fault" (usbscale.c:407-409). |
| `ZEROED` | - | 0x02 "Scale is zero'd" (usbscale.c:410-413). |
| `WEIGHING` | - | 0x03 "Weighing" (usbscale.c:414-417). |
| `STABLE` | - | 0x04: the weight, "the only final, successful status" (usbscale.c:419-431). |
| `UNDER_ZERO` | - | 0x05 "Under Zero" (usbscale.c:432-435). |
| `OVER_WEIGHT` | - | 0x06 "Over Weight" (usbscale.c:436-439). |
| `CALIBRATION_NEEDED` | - | 0x07 "Calibration Needed" (usbscale.c:440-443). |
| `REZEROING_NEEDED` | - | 0x08 "Re-zeroing Needed" (usbscale.c:444-447). |
| `UNKNOWN` | - | Any other byte, "Unknown status code" (usbscale.c:448-451). |

| From | To | Verb |
|---|---|---|
