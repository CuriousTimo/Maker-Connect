//! Implementation of CameraController (`camera_controller.py`).
//!
//! Reference: the SDK at `c0d7d233` (branch `websockets`), `prusa/connect/printer/
//! camera_controller.py`, ported function by function.
//!
//! **Where this port differs:** a trigger pile is a Python `set` of cameras, iterated in the
//! order of their memory addresses; here the order they joined it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use maker_connect_gcode_metadata::py::PyErr;

use crate::camera::{Camera, Error, SharedSnapshot, Value, key_error};
use crate::clock;
use crate::consts::{CameraError, TIMESTAMP_PRECISION, TRIGGER_SCHEME_TO_SECONDS, TriggerScheme};
use crate::models::{CameraRegister, LoopObject};
use crate::util::Session;

/// A callback for sending LoopObjects to Connect
pub type SendCb = Arc<dyn Fn(LoopObject) + Send + Sync>;

struct ControllerState {
    cameras: Vec<(String, Arc<Camera>)>,
    camera_order: Vec<String>,
    trigger_piles: Vec<(TriggerScheme, Vec<Arc<Camera>>)>,
    layer_trigger_counter: u64,
    last_trigger: Vec<(TriggerScheme, f64)>,
}

impl ControllerState {
    fn pile(&mut self, scheme: &Value) -> Result<&mut Vec<Arc<Camera>>, PyErr> {
        let Value::Scheme(scheme) = scheme else {
            return Err(PyErr::KeyError(scheme.to_py_str()));
        };
        Ok(&mut self
            .trigger_piles
            .iter_mut()
            .find(|(s, _)| s == scheme)
            .expect("a pile for every TriggerScheme")
            .1)
    }

    fn camera(&self, camera_id: &str) -> Option<Arc<Camera>> {
        self.cameras
            .iter()
            .find(|(id, _)| id == camera_id)
            .map(|(_, camera)| camera.clone())
    }
}

/// `set.remove(camera)`: `KeyError` if it is not there.
fn remove_from(pile: &mut Vec<Arc<Camera>>, camera: &Arc<Camera>) -> Result<(), PyErr> {
    match pile.iter().position(|c| Arc::ptr_eq(c, camera)) {
        Some(index) => {
            pile.remove(index);
            Ok(())
        }
        None => Err(PyErr::KeyError(format!("{camera:?}"))),
    }
}

/// `set.add(camera)`
fn add_to(pile: &mut Vec<Arc<Camera>>, camera: &Arc<Camera>) {
    if !pile.iter().any(|c| Arc::ptr_eq(c, camera)) {
        pile.push(camera.clone());
    }
}

/// This component harbors functioning cameras, triggers them, sends out images to connect and
/// should contain functionality needed for operating with functional cameras
pub struct CameraController {
    /// A callback for sending LoopObjects to Connect
    pub send_cb: Mutex<SendCb>,
    pub session: Session,
    pub server: Mutex<Option<String>>,
    snapshot_queue: Mutex<VecDeque<SharedSnapshot>>,
    snapshot_ready: Condvar,
    state: Mutex<ControllerState>,
    running: AtomicBool,
}

impl CameraController {
    /// `session`: Current Session connection, `server`: Connect server URL, `send_cb`: a
    /// callback for sending LoopObjects
    pub fn new(session: Session, server: Option<String>, send_cb: SendCb) -> Arc<CameraController> {
        let now = clock::time();
        Arc::new(CameraController {
            send_cb: Mutex::new(send_cb),
            session,
            server: Mutex::new(server),
            snapshot_queue: Mutex::new(VecDeque::new()),
            snapshot_ready: Condvar::new(),
            state: Mutex::new(ControllerState {
                cameras: Vec::new(),
                camera_order: Vec::new(),
                trigger_piles: TriggerScheme::ALL
                    .iter()
                    .map(|s| (*s, Vec::new()))
                    .collect(),
                layer_trigger_counter: 0,
                last_trigger: TRIGGER_SCHEME_TO_SECONDS
                    .iter()
                    .map(|(scheme, _)| (*scheme, now))
                    .collect(),
            }),
            running: AtomicBool::new(false),
        })
    }

    /// Adds a camera. This camera has to be functional
    pub fn add_camera(self: &Arc<Self>, camera: Arc<Camera>) -> Result<(), Error> {
        let camera_id = camera.camera_id();
        let scheme = camera.trigger_scheme()?;
        {
            let mut state = self.state.lock().unwrap();
            match state.cameras.iter_mut().find(|(id, _)| *id == camera_id) {
                Some(entry) => entry.1 = camera.clone(),
                None => state.cameras.push((camera_id, camera.clone())),
            }
            add_to(state.pile(&scheme)?, &camera);
        }
        let controller = self.clone();
        camera.set_scheme_cb(Arc::new(move |camera, old, new| {
            controller.scheme_handler(camera, old, new)
        }));
        let controller = self.clone();
        camera.set_photo_cb(Arc::new(move |snapshot| {
            controller.photo_handler(snapshot);
            Ok(())
        }));
        Ok(())
    }

    /// Removes the camera, either on request, or because it became disconnected
    pub fn remove_camera(&self, camera_id: &str) -> Result<(), Error> {
        let Some(camera) = self.state.lock().unwrap().camera(camera_id) else {
            return Ok(());
        };
        let scheme = camera.trigger_scheme()?;
        let mut state = self.state.lock().unwrap();
        remove_from(state.pile(&scheme)?, &camera)?;
        state.cameras.retain(|(id, _)| id != camera_id);
        Ok(())
    }

    /// Gets the camera by its ID
    pub fn get_camera(&self, camera_id: &str) -> Result<Arc<Camera>, PyErr> {
        self.state
            .lock()
            .unwrap()
            .camera(camera_id)
            .ok_or_else(|| key_error(camera_id))
    }

    /// Does the camera manager know about a camera with given ID? (`__contains__`)
    pub fn contains(&self, camera_id: &str) -> bool {
        self.state.lock().unwrap().camera(camera_id).is_some()
    }

    /// Calls disconnect on any cameras deemed stuck by being busy for longer than
    /// Camera.busy_timeout
    pub fn disconnect_stuck_cameras(&self) -> Result<usize, Error> {
        let cameras: Vec<Arc<Camera>> = self
            .state
            .lock()
            .unwrap()
            .cameras
            .iter()
            .map(|(_, camera)| camera.clone())
            .collect();
        let to_disconnect: Vec<Arc<Camera>> = cameras
            .into_iter()
            .filter(|camera| camera.is_stuck())
            .collect();

        for camera in &to_disconnect {
            log::warn!(
                target: "camera_controller",
                "Camera: {} id: {} looks stuck. Disconnecting",
                camera.name()?,
                camera.camera_id()
            );
            camera.disconnect()?;
        }
        Ok(to_disconnect.len())
    }

    /// Iterates over functional cameras in the configured order
    pub fn cameras_in_order(&self) -> Vec<Arc<Camera>> {
        let state = self.state.lock().unwrap();
        state
            .camera_order
            .iter()
            .filter_map(|camera_id| state.camera(camera_id))
            .collect()
    }

    /// Usually called by the CameraConfigurator to order the SDK cameras
    pub fn set_camera_order(&self, camera_order: Vec<String>) {
        self.state.lock().unwrap().camera_order = camera_order;
    }

    /// Passes the camera to SDK for registration
    pub fn register_camera(&self, camera_id: &str) {
        if !self.contains(camera_id) {
            log::warn!(
                target: "camera_controller",
                "Tried registering a camera id: {camera_id} that's not tracked by this controller"
            );
            return;
        }
        let camera = self.get_camera(camera_id).expect("checked above");
        let send_cb = self.send_cb.lock().unwrap().clone();
        send_cb(LoopObject::CameraRegister(CameraRegister::new(camera)));
    }

    /// Is it time to trigger the time based trigger scheme pile?
    fn interval_elapsed(&self, trigger_scheme: TriggerScheme) -> bool {
        let interval = TRIGGER_SCHEME_TO_SECONDS
            .iter()
            .find(|(scheme, _)| *scheme == trigger_scheme)
            .map(|(_, seconds)| *seconds as f64)
            .expect("a time based trigger scheme");
        let mut state = self.state.lock().unwrap();
        let last = state
            .last_trigger
            .iter_mut()
            .find(|(scheme, _)| *scheme == trigger_scheme)
            .expect("a time based trigger scheme");
        if clock::time() - last.1 < interval {
            return false;
        }
        last.1 = clock::time();
        true
    }

    /// Called every layer, triggers the layer dependant trigger schemes
    pub fn layer_trigger(&self) -> Result<(), Error> {
        let counter = {
            let mut state = self.state.lock().unwrap();
            state.layer_trigger_counter += 1;
            state.layer_trigger_counter
        };
        self.trigger_pile(TriggerScheme::EachLayer)?;
        if counter % 5 == 0 {
            self.trigger_pile(TriggerScheme::FifthLayer)?;
            self.state.lock().unwrap().layer_trigger_counter = 0;
        }
        Ok(())
    }

    /// Called periodically by the SDK to let us trigger cameras when it's the right time
    pub fn tick(&self) -> Result<(), Error> {
        for (scheme, _) in TRIGGER_SCHEME_TO_SECONDS {
            if self.interval_elapsed(scheme) {
                self.trigger_pile(scheme)?;
            }
        }
        Ok(())
    }

    /// Triggers a pile of cameras (cameras are piled by their trigger scheme)
    pub fn trigger_pile(&self, scheme: TriggerScheme) -> Result<(), Error> {
        let cameras = self
            .state
            .lock()
            .unwrap()
            .pile(&Value::Scheme(scheme))
            .expect("a pile for every TriggerScheme")
            .clone();
        for camera in cameras {
            match camera.trigger_a_photo(None) {
                Err(Error::Camera(CameraError::CameraBusy(_))) => {
                    log::warn!(
                        target: "camera_controller",
                        "Skipping camera {} because it's busy",
                        camera.name()?
                    );
                }
                other => other?,
            }
        }
        Ok(())
    }

    /// Transfers cameras between the triggering scheme piles
    pub fn scheme_handler(
        &self,
        camera: &Arc<Camera>,
        old: &Value,
        new: &Value,
    ) -> Result<(), Error> {
        let mut state = self.state.lock().unwrap();
        remove_from(state.pile(old)?, camera)?;
        add_to(state.pile(new)?, camera);
        Ok(())
    }

    /// Puts a snapshot received from the callback into a queue for sending
    pub fn photo_handler(&self, snapshot: SharedSnapshot) {
        if !snapshot.lock().unwrap().is_sendable() {
            return;
        }
        self.snapshot_queue.lock().unwrap().push_back(snapshot);
        self.snapshot_ready.notify_one();
    }

    /// `snapshot_queue.get(timeout=TIMESTAMP_PRECISION)`; `None` is `Empty`.
    fn next_snapshot(&self) -> Option<SharedSnapshot> {
        let queue = self.snapshot_queue.lock().unwrap();
        let (mut queue, _) = self
            .snapshot_ready
            .wait_timeout_while(
                queue,
                Duration::from_secs_f64(TIMESTAMP_PRECISION),
                |queue| queue.is_empty(),
            )
            .unwrap();
        queue.pop_front()
    }

    /// The snapshots waiting to be sent (`snapshot_queue.qsize()`).
    pub fn queued_snapshots(&self) -> usize {
        self.snapshot_queue.lock().unwrap().len()
    }

    /// Gets an item Snapshot from queue and sends it
    pub fn snapshot_loop(&self) {
        self.running.store(true, Ordering::SeqCst);
        while self.running.load(Ordering::SeqCst) {
            // Get the item to send
            let Some(item) = self.next_snapshot() else {
                continue;
            };
            if let Err(err) = self.send_snapshot(&item) {
                log::error!(
                    target: "camera_controller",
                    "Unexpected exception caught in SDK snapshot loop!\n{err}"
                );
            }
        }
    }

    fn send_snapshot(&self, item: &SharedSnapshot) -> Result<(), String> {
        let snapshot = item.lock().unwrap().clone();
        let server = self.server.lock().unwrap().clone();
        let Some(server) = server else {
            return Err(
                "TypeError: unsupported operand type(s) for +: 'NoneType' and 'str'".into(),
            );
        };
        // Send it
        let res = snapshot
            .send(&self.session, &server)
            .map_err(|err| err.to_string())?;
        if res.status_code == 401 || res.status_code == 403 {
            log::error!(
                target: "camera_controller",
                "Failed to authorize request, resetting camera token"
            );
            let camera_id = snapshot.camera_id.clone().unwrap_or_default();
            let camera = self
                .get_camera(&camera_id)
                .map_err(|err| format!("KeyError: {}", err.message()))?;
            camera.set_token(None).map_err(|err| err.to_string())?;
        }
        if res.status_code > 400 {
            log::warn!(target: "camera_controller", "{}", res.text);
        } else if res.status_code == 400 {
            log::debug!(target: "camera_controller", "{}", res.text);
        }
        Ok(())
    }

    /// Signals to the loop to stop
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}
