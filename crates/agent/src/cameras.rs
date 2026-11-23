//! The device `maker-connect.cameras` (`docs/design/prusa-sdk-extension.md`, part 5, "The device that
//! carries an agent's cameras"): the SDK's `CameraController` and `CameraConfigurator` with the nokhwa
//! driver ([`crate::camera::NOKHWA`]), run as PrusaLink runs them.
//!
//! - The controller and its send loop: the SDK's `Printer` (`__init__.py:175-176` makes the controller
//!   with `self.send_cb`, which puts on `self.queue`; `loop`, `:764-776`, calls `camera_controller.tick()`
//!   then `loop_step`; `loop_step`'s HTTP path, `:832-866`, sends a `CameraRegister` and sets the camera's
//!   token from a `200`), at Prusa-Connect-SDK-Printer `c0d7d233`.
//! - The configurator: PrusaLink's (`printer_adapter/prusa_link.py:180-190`), its drivers ours
//!   (nokhwa in place of `V4L2Driver` and `PiCameraDriver`), `auto_detect` PrusaLink's default `True`
//!   (`config.py:197-202`, `[cameras] auto_detect`).
//! - The governor: PrusaLink's `CameraGovernor` (`camera_governor.py`): every `CAMERA_SCAN_INTERVAL`
//!   (30 s, `const.py:97`) `disconnect_stuck_cameras()` then `load_cameras()`, run by `loop_until`
//!   (`util.py:51-73`).
//! - The snapshots: PrusaLink's `snapshot_sender` thread (`sdk_augmentation/printer.py:48-50`,
//!   `:211-214`), `camera_controller.snapshot_loop()`.
//!
//! Ours, each a difference:
//! 1. The configurator's ini is `cameras.ini` in the device's folder. PrusaLink keeps the cameras in its
//!    printer settings file (`config=self.settings`, `config_file_path=self.cfg.printer.settings`); the
//!    agent's `Settings::save` writes that file whole, which would drop the cameras' sections.
//! 2. The `CameraRegister` goes over its own queue and thread, beside the device's event queue: the
//!    agent sends events over the socket from a queue of payloads (`loop_step` in `main.rs`), where the
//!    SDK has one queue of `LoopObject`s for both.
//! 3. Its headers are the agent's (`Agent::headers`), as for `/p/register`: `Timestamp` in whole seconds.
//! 4. Turning the module off stops the threads and disconnects every camera, so another program can open
//!    it. PrusaLink has no module to turn off.
//! 5. Every connected camera without a token is registered after each `load_cameras` ("it registers
//!    every camera that the configurator's auto-detect finds", `prusa-sdk-extension.md` part 5).
//!    PrusaLink registers a camera only when its user asks (`web/cameras.py:296`,
//!    `camera_controller.register_camera`).

use crate::{Agent, camera};
use maker_connect_sdk::camera_configurator::CameraConfigurator;
use maker_connect_sdk::camera_controller::{CameraController, SendCb};
use maker_connect_sdk::configparser::ConfigParser;
use maker_connect_sdk::consts;
use maker_connect_sdk::models::LoopObject;
use maker_connect_sdk::util::{RetryingSession, Session};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

/// PrusaLink's `CAMERA_SCAN_INTERVAL` (`const.py:97`), seconds.
const CAMERA_SCAN_INTERVAL: u64 = 30;

/// The configurator's file (difference 1).
pub(crate) const CAMERAS_INI: &str = "cameras.ini";

/// The running cameras of the device: what `module_on` starts and `module_off` stops.
pub(crate) struct Cameras {
    pub(crate) controller: Arc<CameraController>,
    pub(crate) configurator: Arc<CameraConfigurator>,
    /// `_governance_quit_event` and the send loop's: set when the module goes off.
    quit: Arc<(Mutex<bool>, Condvar)>,
}

/// The SDK's `queue` for the controller's `LoopObject`s (difference 2).
#[derive(Default)]
struct Queue {
    items: Mutex<VecDeque<LoopObject>>,
    put: Condvar,
}

impl Queue {
    /// `queue.get(timeout=const.TIMESTAMP_PRECISION)`
    fn get(&self) -> Option<LoopObject> {
        let items = self.items.lock().unwrap();
        let (mut items, _) = self
            .put
            .wait_timeout_while(
                items,
                Duration::from_secs_f64(consts::TIMESTAMP_PRECISION),
                |items| items.is_empty(),
            )
            .unwrap();
        items.pop_front()
    }
}

impl Cameras {
    /// Starts the controller, the configurator (which loads and auto-detects the cameras), the
    /// governor, the snapshot sender and the send loop. `None` when the configurator cannot start,
    /// logged.
    pub(crate) fn start(agent: &Arc<Agent>) -> Option<Cameras> {
        let queue = Arc::new(Queue::default());
        let send_cb: SendCb = {
            let queue = queue.clone();
            Arc::new(move |item| {
                queue.items.lock().unwrap().push_back(item);
                queue.put.notify_one();
            })
        };
        let controller = CameraController::new(Session::new(), agent.settings().server(), send_cb);
        let path = agent.dir.join(CAMERAS_INI);
        let mut config = ConfigParser::new();
        if let Err(e) = config.read(&path) {
            log::warn!("cameras: {} cannot be read: {e:?}", path.display());
        }
        let configurator = match CameraConfigurator::new(
            controller.clone(),
            Arc::new(Mutex::new(config)),
            path,
            vec![&camera::NOKHWA],
            true,
        ) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("cameras: the configurator did not start: {e:?}");
                return None;
            }
        };
        let quit = Arc::new((Mutex::new(false), Condvar::new()));
        // The governor (`CameraGovernor.start`, `loop_until`).
        {
            let quit = quit.clone();
            let controller = controller.clone();
            let configurator = Arc::downgrade(&configurator);
            std::thread::Builder::new()
                .name("camera_governance".into())
                .spawn(move || govern(&quit, &controller, &configurator))
                .ok();
        }
        // The snapshot sender (`snapshot_loop`).
        {
            let controller = controller.clone();
            std::thread::Builder::new()
                .name("snapshot_sender".into())
                .spawn(move || controller.snapshot_loop())
                .ok();
        }
        // The SDK's `loop`, for the controller's part.
        {
            let quit = quit.clone();
            let controller = controller.clone();
            let agent = Arc::downgrade(agent);
            std::thread::Builder::new()
                .name("camera_loop".into())
                .spawn(move || send_loop(&quit, &controller, &queue, &agent))
                .ok();
        }
        Some(Cameras {
            controller,
            configurator,
            quit,
        })
    }

    /// Stops the threads and disconnects the cameras (difference 4).
    pub(crate) fn stop(self) {
        *self.quit.0.lock().unwrap() = true;
        self.quit.1.notify_all();
        self.controller.stop();
        for (id, driver) in self.configurator.loaded() {
            if let Err(e) = driver.disconnect() {
                log::warn!("cameras: {id} did not disconnect: {e:?}");
            }
        }
    }
}

fn quitting(quit: &(Mutex<bool>, Condvar)) -> bool {
    *quit.0.lock().unwrap()
}

/// `loop_until(loop_evt, run_every_sec=CAMERA_SCAN_INTERVAL, to_run=_govern)`
fn govern(
    quit: &(Mutex<bool>, Condvar),
    controller: &CameraController,
    configurator: &Weak<CameraConfigurator>,
) {
    while !quitting(quit) {
        let last_called = std::time::Instant::now();
        // `_govern`
        log::debug!("Running the camera governance routine");
        match controller.disconnect_stuck_cameras() {
            Ok(n) if n > 0 => log::info!("cameras: a stuck camera"),
            Ok(_) => {}
            Err(e) => log::warn!("cameras: {e:?}"),
        }
        let Some(configurator) = configurator.upgrade() else {
            return;
        };
        if let Err(e) = configurator.load_cameras(false) {
            log::warn!("cameras: {e:?}");
        }
        // difference 5
        for camera in controller.cameras_in_order() {
            if !camera.is_registered() {
                controller.register_camera(&camera.camera_id());
            }
        }
        drop(configurator);
        let run_again_in =
            Duration::from_secs(CAMERA_SCAN_INTERVAL).saturating_sub(last_called.elapsed());
        let stopped = quit.0.lock().unwrap();
        let _ = quit
            .1
            .wait_timeout_while(stopped, run_again_in, |stopped| !*stopped);
    }
}

/// The SDK's `loop` (`__init__.py:764-776`): `camera_controller.tick()`, then `loop_step` for the
/// controller's items: a `CameraRegister` over HTTP (`:832-866`).
fn send_loop(
    quit: &(Mutex<bool>, Condvar),
    controller: &CameraController,
    queue: &Queue,
    agent: &Weak<Agent>,
) {
    let conn = RetryingSession::new(1);
    while !quitting(quit) {
        if let Err(e) = controller.tick() {
            log::error!("Unexpected exception from the camera module caught in SDK loop!\n{e:?}");
        }
        let Some(item) = queue.get() else {
            continue;
        };
        let Some(agent) = agent.upgrade() else {
            return;
        };
        let s = agent.settings();
        let Some(server) = s.server() else {
            log::warn!("Server is not set, skipping item: {item:?}");
            continue;
        };
        if s.token.is_empty() {
            log::warn!("No token, skipping item: {item:?}");
            continue;
        }
        let headers: Vec<(String, String)> = agent
            .headers(true)
            .into_iter()
            .filter_map(|(k, v)| v.map(|v| (k, v)))
            .collect();
        match item.send(&conn, &server, headers) {
            Err(e) => log::error!("{e:?}"),
            Ok(res) => {
                if let LoopObject::CameraRegister(register) = &item {
                    if res.status_code == 200 {
                        let token = res.header("Token").map(str::to_owned);
                        if let Err(e) = register.camera.set_token(token) {
                            log::warn!("{e:?}");
                        }
                    } else {
                        log::warn!("{}", res.text);
                    }
                }
                if res.status_code > 400 {
                    log::warn!("{}", res.text);
                } else if res.status_code == 400 {
                    log::debug!("{}", res.text);
                }
            }
        }
    }
}
