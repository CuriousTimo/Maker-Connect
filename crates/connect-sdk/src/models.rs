//! Connect printer data models (`models.py`).

use std::sync::Arc;

use maker_connect_gcode_metadata::py::{Dict, PyErr, PyValue};
use serde_json::{Map, Value};

use crate::camera::{self, Camera};
use crate::clock;
use crate::consts::{self, Source, State};
use crate::util::{RequestError, Response, RetryingSession, get_timestamp, py_truthy};

/// 30 min
pub const CODE_TIMEOUT: i64 = 60 * 30;

/// Python's `**kwargs`: names to values, in call order.
pub type Kwargs = Map<String, Value>;

/// `EventCallback(event, source, timestamp=None, command_id=None, **kwargs)`
pub type EventCallback =
    Arc<dyn Fn(consts::Event, Source, Option<f64>, Option<i64>, Kwargs) + Send + Sync>;

/// `TelemetryCallback(state, timestamp=None, **kwargs)`
pub type TelemetryCallback = Arc<dyn Fn(State, Option<f64>, Kwargs) + Send + Sync>;

/// Returns object (dict, list, etc.) without null values recursively.
pub fn filter_null(obj: &Value) -> Value {
    match obj {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(_, val)| !val.is_null())
                .map(|(key, val)| (key.clone(), filter_null(val)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .filter(|val| !val.is_null())
                .map(filter_null)
                .collect(),
        ),
        other => other.clone(),
    }
}

/// A request to Connect to register the printer; does not need the token, this one is needed
/// to get it.
#[derive(Debug, Clone, PartialEq)]
pub struct Register {
    pub timestamp: f64,
    pub code: String,
    pub timeout: i64,
}

impl Register {
    pub fn new(code: &str) -> Register {
        Register {
            timestamp: get_timestamp(None),
            code: code.to_owned(),
            timeout: clock::time() as i64 + CODE_TIMEOUT,
        }
    }
}

/// Event object must contain at least Event type and source.
///
/// `timestamp`: if not set `int(time.time()*10)/10` is used. `command_id`: must be set for
/// answer to Connect command. Any other named attributes are stored in the `data` structure.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub timestamp: f64,
    pub event: consts::Event,
    pub source: Source,
    pub command_id: Option<i64>,
    pub job_id: Option<Value>,
    pub transfer_id: Option<Value>,
    pub reason: Option<Value>,
    pub state: Option<Value>,
    pub dialog_id: Option<Value>,
    pub data: Kwargs,
}

impl Event {
    /// `Event(event, source, timestamp, command_id, **kwargs)`: `job_id`, `transfer_id`,
    /// `reason`, `state` and `dialog_id` are taken out of `kwargs` as Python binds them by name.
    pub fn new(
        event: consts::Event,
        source: Source,
        timestamp: Option<f64>,
        command_id: Option<i64>,
        mut kwargs: Kwargs,
    ) -> Event {
        let mut take = |name: &str| kwargs.shift_remove(name);
        let job_id = take("job_id");
        let transfer_id = take("transfer_id");
        let reason = take("reason");
        let state = take("state");
        let dialog_id = take("dialog_id");
        Event {
            timestamp: get_timestamp(timestamp),
            event,
            source,
            command_id,
            job_id,
            transfer_id,
            reason,
            state,
            dialog_id,
            data: kwargs,
        }
    }

    /// Send event to connect. The attributes are only sent when truthy (so a `0` is dropped).
    pub fn to_payload(&self) -> Value {
        let mut data = Map::new();
        data.insert("event".into(), self.event.value().into());
        data.insert("source".into(), self.source.value().into());
        data.insert(
            "data".into(),
            filter_null(&Value::Object(self.data.clone())),
        );
        if let Some(command_id) = self.command_id.filter(|id| *id != 0) {
            data.insert("command_id".into(), command_id.into());
        }
        for (attr, value) in [
            ("job_id", &self.job_id),
            ("transfer_id", &self.transfer_id),
            ("reason", &self.reason),
            ("dialog_id", &self.dialog_id),
        ] {
            if let Some(value) = value.as_ref().filter(|v| py_truthy(v)) {
                data.insert(attr.into(), value.clone());
            }
        }
        if let Some(state) = self.state.as_ref().filter(|v| py_truthy(v)) {
            data.insert("state".into(), state.clone());
        }
        Value::Object(data)
    }
}

/// Telemetry object must contain at least Printer state
#[derive(Debug, Clone, PartialEq)]
pub struct Telemetry {
    pub timestamp: f64,
    data: Kwargs,
}

impl Telemetry {
    /// `timestamp`: if not set `int(time.time()*10)/10` is used.
    pub fn new(state: State, timestamp: Option<f64>, mut kwargs: Kwargs) -> Telemetry {
        kwargs.insert("state".into(), state.value().into());
        Telemetry {
            timestamp: get_timestamp(timestamp),
            data: kwargs,
        }
    }

    /// Returns telemetry payload data
    pub fn to_payload(&self) -> Value {
        filter_null(&Value::Object(self.data.clone()))
    }
}

/// A common object that can be sent out (`LoopObject` and its subclasses).
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // queued one by one, as the Python objects
pub enum LoopObject {
    Register(Register),
    Event(Event),
    Telemetry(Telemetry),
    CameraRegister(CameraRegister),
}

/// Why [`LoopObject::send`] did not send: `requests` failed, or the payload could not be made.
#[derive(Debug, Clone, PartialEq)]
pub enum SendError {
    Request(RequestError),
    Payload(camera::Error),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::Request(err) => write!(f, "{err}"),
            SendError::Payload(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SendError {}

impl From<RequestError> for SendError {
    fn from(err: RequestError) -> SendError {
        SendError::Request(err)
    }
}

impl LoopObject {
    pub fn endpoint(&self) -> &'static str {
        match self {
            LoopObject::Register(_) => "/p/register",
            LoopObject::Event(_) => "/p/events",
            LoopObject::Telemetry(_) => "/p/telemetry",
            LoopObject::CameraRegister(_) => "/p/camera",
        }
    }

    pub fn method(&self) -> &'static str {
        match self {
            LoopObject::Register(_) => "GET",
            LoopObject::Event(_) | LoopObject::Telemetry(_) | LoopObject::CameraRegister(_) => {
                "POST"
            }
        }
    }

    pub fn needs_token(&self) -> bool {
        !matches!(self, LoopObject::Register(_))
    }

    pub fn timestamp(&self) -> f64 {
        match self {
            LoopObject::Register(r) => r.timestamp,
            LoopObject::Event(e) => e.timestamp,
            LoopObject::Telemetry(t) => t.timestamp,
            LoopObject::CameraRegister(c) => c.timestamp,
        }
    }

    /// By default, LoopObjects don't send any payload
    pub fn to_payload(&self) -> Result<Option<Value>, camera::Error> {
        Ok(match self {
            LoopObject::Register(_) => None,
            LoopObject::Event(e) => Some(e.to_payload()),
            LoopObject::Telemetry(t) => Some(t.to_payload()),
            LoopObject::CameraRegister(c) => Some(c.to_payload()?),
        })
    }

    /// A universal send function. Register needs an extra code in the headers, this adds it.
    pub fn send(
        &self,
        conn: &RetryingSession,
        server: &str,
        mut headers: Vec<(String, String)>,
    ) -> Result<Response, SendError> {
        if let LoopObject::Register(register) = self {
            set_header(&mut headers, "Code", &register.code);
        }
        let name = match self {
            LoopObject::Register(_) => "Register",
            LoopObject::Event(_) => "Event",
            LoopObject::Telemetry(_) => "Telemetry",
            LoopObject::CameraRegister(_) => "CameraRegister",
        };
        log::debug!("Sending {name}: {self:?}");
        let payload = self.to_payload().map_err(SendError::Payload)?;
        let url = format!("{server}{}", self.endpoint());
        let res = conn.request(self.method(), &url, &headers, payload.as_ref())?;
        log::debug!("{name} response: {}", res.text);
        Ok(res)
    }
}

/// `headers[name] = value`: replaces an existing header (case as given), else appends.
pub fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    match headers.iter_mut().find(|(key, _)| key == name) {
        Some(entry) => entry.1 = value.to_owned(),
        None => headers.push((name.to_owned(), value.to_owned())),
    }
}

/// A request to Connect to register the camera
#[derive(Debug, Clone)]
pub struct CameraRegister {
    pub timestamp: f64,
    pub camera: Arc<Camera>,
}

impl PartialEq for CameraRegister {
    /// The same object
    fn eq(&self, other: &CameraRegister) -> bool {
        self.timestamp == other.timestamp && Arc::ptr_eq(&self.camera, &other.camera)
    }
}

impl CameraRegister {
    pub fn new(camera: Arc<Camera>) -> CameraRegister {
        CameraRegister {
            timestamp: get_timestamp(None),
            camera,
        }
    }

    /// Converts the camera to data for registration
    pub fn to_payload(&self) -> Result<Value, camera::Error> {
        let obj_config = self.camera.get_settings()?;
        let mut config = Camera::json_from_settings(&obj_config)?;
        config.set("camera_id", PyValue::Str(self.camera.camera_id()));
        let available_resolutions: Vec<PyValue> = self
            .camera
            .available_resolutions()
            .iter()
            .map(|res| PyValue::Dict(res.to_dict()))
            .collect();
        let mut options = Dict::new();
        options.set(
            "available_resolutions",
            PyValue::List(available_resolutions),
        );
        let capabilities: Vec<PyValue> = self
            .camera
            .capabilities()
            .iter()
            .map(|cap| PyValue::str(cap.value()))
            .collect();

        let mut data = Dict::new();
        data.set("config", PyValue::Dict(config));
        data.set("options", PyValue::Dict(options));
        data.set("capabilities", PyValue::List(capabilities));
        data.set("fingerprint", PyValue::Str(self.camera.fingerprint()));
        Ok(py_to_json(&PyValue::Dict(data))?)
    }
}

/// A Python value as `requests` puts it into JSON: `json.dumps(value, allow_nan=False)`.
fn py_to_json(value: &PyValue) -> Result<Value, PyErr> {
    Ok(match value {
        PyValue::None => Value::Null,
        PyValue::Bool(b) => Value::Bool(*b),
        PyValue::Int(i) => match i.to_i64() {
            Some(i) => Value::from(i),
            None => {
                return Err(camera::not_covered(format!(
                    "the int {} in JSON",
                    i.as_str()
                )));
            }
        },
        PyValue::Float(f) => match serde_json::Number::from_f64(*f) {
            Some(n) => Value::Number(n),
            None => {
                return Err(PyErr::ValueError(format!(
                    "Out of range float values are not JSON compliant: {}",
                    crate::util::py_float_repr(*f)
                )));
            }
        },
        PyValue::Str(s) => Value::String(s.clone()),
        PyValue::List(items) => {
            Value::Array(items.iter().map(py_to_json).collect::<Result<_, _>>()?)
        }
        PyValue::Dict(dict) => {
            let mut map = Map::new();
            for (key, item) in dict.iter() {
                map.insert(key.clone(), py_to_json(item)?);
            }
            Value::Object(map)
        }
    })
}

/// A model for type hinting the sheet settings list
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Sheet {
    pub name: String,
    pub z_offset: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::py_json_dumps;
    use serde_json::json;

    fn kwargs(value: Value) -> Kwargs {
        match value {
            Value::Object(map) => map,
            _ => unreachable!(),
        }
    }

    /// The doctests of `filter_null`.
    #[test]
    fn filter_null_doctests() {
        assert_eq!(
            filter_null(&json!({"one": 1, "none": null})),
            json!({"one": 1})
        );
        assert_eq!(filter_null(&json!([1, null])), json!([1]));
        assert_eq!(
            filter_null(&json!({"list": [1, null], "dict": {"one": 1, "none": null}})),
            json!({"list": [1], "dict": {"one": 1}})
        );
    }

    /// Expected payload printed by Python 3.11 at c0d7d233:
    /// `json.dumps(Event(Event.REJECTED, Source.CONNECT, command_id=7, reason="Another command
    /// is running", actual_command_id=3, state=State.IDLE).to_payload())`
    #[test]
    fn event_payload() {
        let event = Event::new(
            consts::Event::Rejected,
            Source::Connect,
            None,
            Some(7),
            kwargs(json!({
                "reason": "Another command is running",
                "actual_command_id": 3,
                "state": "IDLE",
            })),
        );
        assert_eq!(
            py_json_dumps(&event.to_payload()),
            r#"{"event": "REJECTED", "source": "CONNECT", "data": {"actual_command_id": 3}, "command_id": 7, "reason": "Another command is running", "state": "IDLE"}"#
        );
    }

    #[test]
    fn event_drops_falsy_attributes() {
        let event = Event::new(
            consts::Event::Info,
            Source::Wui,
            Some(5.0),
            Some(0),
            kwargs(
                json!({"job_id": 0, "reason": "", "dialog_id": null, "x": null, "y": [null, 1]}),
            ),
        );
        assert_eq!(event.timestamp, 5.0);
        assert_eq!(
            py_json_dumps(&event.to_payload()),
            r#"{"event": "INFO", "source": "WUI", "data": {"y": [1]}}"#
        );
    }

    #[test]
    fn telemetry_payload() {
        let telemetry = Telemetry::new(
            State::Printing,
            None,
            kwargs(json!({"temp_nozzle": 215.0, "fan": null})),
        );
        assert_eq!(
            py_json_dumps(&telemetry.to_payload()),
            r#"{"temp_nozzle": 215.0, "state": "PRINTING"}"#
        );
        let object = LoopObject::Telemetry(telemetry);
        assert_eq!(
            (object.endpoint(), object.method(), object.needs_token()),
            ("/p/telemetry", "POST", true)
        );
    }

    #[test]
    fn register() {
        let register = Register::new("abc");
        let now = clock::time() as i64;
        assert!((register.timeout - now - CODE_TIMEOUT).abs() <= 1);
        let object = LoopObject::Register(register);
        assert_eq!(
            (object.endpoint(), object.method(), object.needs_token()),
            ("/p/register", "GET", false)
        );
        assert_eq!(object.to_payload(), Ok(None));

        let mut headers = vec![("Code".to_owned(), "old".to_owned())];
        set_header(&mut headers, "Code", "abc");
        set_header(&mut headers, "Token", "t");
        assert_eq!(
            headers,
            vec![("Code".into(), "abc".into()), ("Token".into(), "t".into())]
        );
    }
}
