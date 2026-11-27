//! PrusaLink's `config.py` (`reference/Prusa-Link`, `171148ce`, `prusa/link/config.py`), with
//! `extendparser`'s `Get` it stands on (`extendparser/get.py`, 0.3.1, BSD, Ondřej Tůma), ported
//! function by function (#241, section A0 of `docs/archive/plans/mk3-port-plan.md`).
//!
//! The parser underneath is CPython 3.13.12's `configparser`
//! ([`maker_connect_sdk::configparser`]): `Config` is `ConfigParser()`, with `BasicInterpolation`,
//! `Settings` is `ConfigParser(interpolation=None)`. `int(...)` is CPython's
//! ([`maker_connect_gcode_metadata::py::int_from_str`]), the paths `os.path`'s
//! ([`maker_connect_sdk::pyos`]).
//!
//! From section A, the two `Settings` needs: `PRINTER_CONF_TYPES` (`const.py:36-41`, in
//! [`super::consts`]; a `bidict`: its keys and the membership test only) and `sanitize_api_key` (`util.py:37-41`).
//!
//! **Not covered:**
//! - `set_global_log_level` (`config.py:229-240`) and the handler `get_log_handler` makes
//!   (`:242-261`): Python's `logging` set-up, nothing the printer sees. What it reads from the
//!   file is ported ([`Config::log_handler`]): the same `get` calls, so the same errors.
//! - `Settings.instance` (`:270`, `:317`): one `Settings` per process. Here one per printer
//!   module, as the agent hosts several (#218).
//! - `bidict` (`const.py:11`): only `in` of its keys is used here (`config.py:291`).
//! - `get_option`'s `list`, `tuple` and `set` targets (`get.py:53-56`): `config.py` asks for
//!   `str`, `int` and `bool` only.
//! - Logging (`log.warning`, `log.info` in `get_option`), as in the other engine ports: kept as
//!   comments.
//! - `pwd` on Windows (`config.py:114`, `:118`): there is none; an error names the call.

use std::path::{Path, PathBuf};

use maker_connect_gcode_metadata::py::{PyErr, PyInt, int_from_str, repr_str, strip};
use maker_connect_sdk::configparser::{ConfigParser, Error};
use maker_connect_sdk::pyos;

/// `CONNECT` (`config.py:16`)
pub const CONNECT: &str = "connect.prusa3d.com";

/// `LOG_FORMAT_FOREGROUND` (`config.py:18-20`)
pub const LOG_FORMAT_FOREGROUND: &str = "%(asctime)s %(levelname)s {%(module)s.%(funcName)s():%(lineno)d} [%(threadName)s]: %(message)s ";

/// `LOG_FORMAT_SYSLOG` (`config.py:21-23`)
pub const LOG_FORMAT_SYSLOG: &str =
    "%(name)s[%(process)d]: %(levelname)s: %(message)s {%(funcName)s():%(lineno)d}";

pub use super::consts::PRINTER_CONF_TYPES;

/// `sanitize_api_key` (`util.py:37-41`): Strip leading and trailing whitespace from an API key
/// value.
pub fn sanitize_api_key(api_key: &str) -> String {
    if api_key.is_empty() {
        return String::new();
    }
    strip(api_key).to_owned()
}

fn value_error(message: String) -> Error {
    Error {
        class: "ValueError",
        message,
    }
}

fn py_err(err: PyErr) -> Error {
    Error {
        class: err.class_name(),
        message: err.message().to_owned(),
    }
}

/// `get_log_level_dict` (`config.py:28-38`): Parse log level from command line arguments.
pub fn get_log_level_dict(log_levels: &[String]) -> Result<Vec<(String, String)>, Error> {
    let mut log_level_dict: Vec<(String, String)> = Vec::new();
    for log_config in log_levels {
        let parts: Vec<&str> = log_config.split('=').collect();
        if parts.len() != 2 {
            return Err(value_error(
                "Log level settings needs to contain exactly one \"=\"".to_owned(),
            ));
        }
        dict_set(&mut log_level_dict, parts[0], parts[1]);
    }
    Ok(log_level_dict)
}

/// `check_log_level` (`config.py:41-44`): Check valid log level.
pub fn check_log_level(value: &str) -> Result<(), Error> {
    if !["CRITICAL", "ERROR", "WARNING", "INFO", "DEBUG"].contains(&value) {
        return Err(value_error(format!("Invalid value {value}")));
    }
    Ok(())
}

/// `check_server_type` (`config.py:47-50`): Check valid server class.
pub fn check_server_type(value: &str) -> Result<(), Error> {
    if !["single", "threading", "forking"].contains(&value) {
        return Err(value_error(format!("Invalid value {value}")));
    }
    Ok(())
}

fn dict_set(dict: &mut Vec<(String, String)>, key: &str, value: &str) {
    match dict.iter_mut().find(|(k, _)| k == key) {
        Some(entry) => entry.1 = value.to_owned(),
        None => dict.push((key.to_owned(), value.to_owned())),
    }
}

/// A value of a [`Model`], as Python holds it: what `get_option` makes of the file's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Int(PyInt),
    Bool(bool),
    None,
}

impl Value {
    /// `type(value).__name__`
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Str(_) => "str",
            Value::Int(_) => "int",
            Value::Bool(_) => "bool",
            Value::None => "NoneType",
        }
    }

    /// Python's truth value (`if value:`)
    pub fn truthy(&self) -> bool {
        match self {
            Value::Str(s) => !s.is_empty(),
            Value::Int(i) => !i.is_zero(),
            Value::Bool(b) => *b,
            Value::None => false,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
}

impl std::fmt::Display for Value {
    /// `str(value)`
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Str(s) => f.write_str(s),
            Value::Int(i) => f.write_str(i.as_str()),
            Value::Bool(true) => f.write_str("True"),
            Value::Bool(false) => f.write_str("False"),
            Value::None => f.write_str("None"),
        }
    }
}

/// The target of a `get_option` (`get.py:41`): `str`, `int` or `bool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Str,
    Int,
    Bool,
}

/// `Model` (`config.py:53-69`): Config model based on dictionary, in insertion order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Model(pub Vec<(String, Value)>);

impl Model {
    /// `model.key` (`__getattr__`): `AttributeError(KeyError(key))` when it is not there.
    pub fn attr(&self, key: &str) -> Result<&Value, Error> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .ok_or_else(|| Error {
                class: "AttributeError",
                message: repr_str(key),
            })
    }

    /// `model.key = val` (`__setattr__`)
    pub fn set(&mut self, key: &str, val: Value) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = val,
            None => self.0.push((key.to_owned(), val)),
        }
    }

    /// `model.key` for a `str` value
    pub fn str(&self, key: &str) -> Result<&str, Error> {
        let value = self.attr(key)?;
        value.as_str().ok_or_else(|| Error {
            class: "TypeError",
            message: format!("{key} is not a str: {value}"),
        })
    }
}

/// `Get.get_option(section, option, target, fallback)` (`get.py:41-66`): Return option in
/// target type. `fallback` `None` is `Nothing` (none given).
pub fn get_option(
    parser: &ConfigParser,
    section: &str,
    option: &str,
    target: Target,
    fallback: Option<Value>,
) -> Result<Value, Error> {
    let result = (|| {
        if target == Target::Bool {
            return Ok(Value::Bool(parser.getboolean(section, option)?));
        }
        let got = parser.get(section, option)?;
        let value = strip(&got);
        match target {
            Target::Str => Ok(Value::Str(value.to_owned())),
            Target::Int => int_from_str(value).map(Value::Int).map_err(py_err),
            Target::Bool => unreachable!(),
        }
    })();
    match result {
        Err(err) if err.class == "NoSectionError" || err.class == "NoOptionError" => {
            match fallback {
                // log.warning("[%s]::%s not defined and no fallback value specified", ...)
                None => Err(err),
                // log.info("Using fallback value `%s' for [%s]::%s", ...)
                Some(fallback) => Ok(fallback),
            }
        }
        other => other,
    }
}

/// `Get.get_section(section, options, skip=True)` (`get.py:67-92`): Get full options from
/// section; options without fallback values are not returned if they are not found.
pub fn get_section(
    parser: &ConfigParser,
    section: &str,
    options: &[(&str, Target, Option<Value>)],
) -> Result<Model, Error> {
    let mut values = Model::default();
    for (option, target, fallback) in options {
        match get_option(parser, section, option, *target, fallback.clone()) {
            Ok(value) => values.set(option, value),
            Err(err) if err.class == "NoSectionError" || err.class == "NoOptionError" => {}
            Err(err) => return Err(err),
        }
    }
    Ok(values)
}

fn s(text: &str) -> Option<Value> {
    Some(Value::Str(text.to_owned()))
}

fn int(value: i64) -> Option<Value> {
    Some(Value::Int(PyInt::from_i64(value)))
}

fn b(value: bool) -> Option<Value> {
    Some(Value::Bool(value))
}

/// `FakeArgs` (`config.py:72-87`): the command line's arguments `Config` reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub config: PathBuf,
    pub debug: bool,
    pub foreground: bool,
    pub pidfile: Option<String>,
    pub module_log_level: Option<Vec<String>>,
    pub address: Option<String>,
    pub tcp_port: Option<i64>,
    pub link_info: Option<bool>,
    pub serial_port: Option<String>,
    pub info: bool,
    pub printer_number: Option<i64>,
}

impl Args {
    /// `FakeArgs(path)`
    pub fn fake(path: &Path) -> Args {
        Args {
            config: path.to_path_buf(),
            debug: false,
            foreground: true,
            pidfile: None,
            module_log_level: None,
            address: None,
            tcp_port: None,
            link_info: None,
            serial_port: None,
            info: false,
            printer_number: None,
        }
    }
}

/// A user's name and home (`pwd.getpwuid(...)`, `pwd.getpwnam(...)`): `pw_name`, `pw_dir`.
#[cfg(unix)]
fn passwd(entry: *mut libc::passwd) -> Option<(String, String)> {
    if entry.is_null() {
        return None;
    }
    // SAFETY: a non-null entry from getpwuid/getpwnam points at its static struct, whose
    // strings are NUL-terminated.
    unsafe {
        let name = std::ffi::CStr::from_ptr((*entry).pw_name);
        let dir = std::ffi::CStr::from_ptr((*entry).pw_dir);
        Some((
            name.to_string_lossy().into_owned(),
            dir.to_string_lossy().into_owned(),
        ))
    }
}

/// `pwd.getpwuid(getuid())` (`config.py:114`)
#[cfg(unix)]
fn getpwuid_self() -> Result<(String, String), Error> {
    // SAFETY: getuid cannot fail; getpwuid returns null or a pointer to a static entry.
    let uid = unsafe { libc::getuid() };
    passwd(unsafe { libc::getpwuid(uid) }).ok_or_else(|| Error {
        class: "KeyError",
        message: format!("'getpwuid(): uid not found: {uid}'"),
    })
}

/// `pwd.getpwnam(name)` (`config.py:118`)
#[cfg(unix)]
fn getpwnam(name: &str) -> Result<(String, String), Error> {
    let not_found = || Error {
        class: "KeyError",
        message: format!("'getpwnam(): name not found: {}'", repr_str(name)),
    };
    let cname = std::ffi::CString::new(name).map_err(|_| not_found())?;
    // SAFETY: cname is NUL-terminated; getpwnam returns null or a static entry.
    passwd(unsafe { libc::getpwnam(cname.as_ptr()) }).ok_or_else(not_found)
}

/// `getuid() != 0`
#[cfg(unix)]
fn not_root() -> bool {
    // SAFETY: getuid cannot fail.
    unsafe { libc::getuid() != 0 }
}

#[cfg(not(unix))]
fn no_pwd(call: &str) -> Error {
    Error {
        class: "ModuleNotFoundError",
        message: format!("No module named 'pwd' ({call}, config.py:114, :118: not covered)"),
    }
}

#[cfg(not(unix))]
fn getpwuid_self() -> Result<(String, String), Error> {
    Err(no_pwd("getpwuid"))
}

#[cfg(not(unix))]
fn getpwnam(_name: &str) -> Result<(String, String), Error> {
    Err(no_pwd("getpwnam"))
}

#[cfg(not(unix))]
fn not_root() -> bool {
    true
}

/// An error of `os.path` (`abspath` fails only in `getcwd`, or on a NUL): its class and text
/// (an `OSError`'s `repr`: its `str` is not ported).
fn os_err(err: pyos::Error) -> Error {
    match err {
        pyos::Error::Os(err) => Error {
            class: "OSError",
            message: err.repr(),
        },
        pyos::Error::Value(message) => value_error(message),
    }
}

fn path_str(path: &[u8]) -> String {
    String::from_utf8_lossy(path).into_owned()
}

/// `abspath(join(a, b))`
fn abspath_join(a: &str, b: &str) -> Result<String, Error> {
    let joined = pyos::join(a.as_bytes(), &[b.as_bytes()]);
    pyos::abspath(&joined).map(|p| path_str(&p)).map_err(os_err)
}

/// The log handler `get_log_handler` makes (`config.py:242-261`): its kind (`StreamHandler`, or
/// `SysLogHandler` with its address) and format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogHandler {
    pub kind: &'static str,
    pub address: Option<String>,
    pub format: String,
}

/// `Config` (`config.py:90-261`): This class handles prusalink.ini configuration file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub parser: ConfigParser,
    pub debug: bool,
    pub daemon: Model,
    pub log_settings: Vec<(String, String)>,
    pub configured_handler: LogHandler,
    pub http: Model,
    pub printer: Model,
    pub cameras: Model,
}

impl Config {
    /// `Config(args)` (`config.py:94-203`)
    pub fn new(args: &Args) -> Result<Config, Error> {
        let mut parser = ConfigParser::with_basic_interpolation();

        parser.read(&args.config)?;
        let debug = args.debug;

        // [daemon]
        let mut daemon = get_section(
            &parser,
            "daemon",
            &[
                ("data_dir", Target::Str, s("")), // user home by default
                ("pid_file", Target::Str, s("./prusalink.pid")),
                ("power_panic_file", Target::Str, s("./power_panic")),
                ("threshold_file", Target::Str, s("./threshold.data")),
                ("user", Target::Str, s("pi")),
                ("group", Target::Str, s("pi")),
                ("printer_number", Target::Int, Some(Value::None)),
            ],
        )?;
        if args.foreground || not_root() {
            let (pw_name, pw_dir) = getpwuid_self()?;
            daemon.set("user", Value::Str(pw_name));
            daemon.set("home", Value::Str(pw_dir));
        } else {
            let (_, pw_dir) = getpwnam(daemon.str("user")?)?;
            daemon.set("home", Value::Str(pw_dir));
        }

        if !daemon.attr("data_dir")?.truthy() {
            let home = daemon.attr("home")?.clone();
            daemon.set("data_dir", home);
        }

        if let Some(pidfile) = &args.pidfile
            && !pidfile.is_empty()
        {
            let abs = pyos::abspath(pidfile.as_bytes()).map_err(os_err)?;
            daemon.set("pid_file", Value::Str(path_str(&abs)));
        }
        if let Some(printer_number) = args.printer_number {
            daemon.set(
                "printer_number",
                Value::Int(PyInt::from_i64(printer_number)),
            );
        }

        for file_ in ["pid_file", "power_panic_file", "threshold_file"] {
            let data_dir = daemon.attr("data_dir")?.to_string();
            let value = daemon.attr(file_)?.to_string();
            daemon.set(file_, Value::Str(abspath_join(&data_dir, &value)?));
        }

        // [logging]: set_global_log_level(args), not covered

        // Let's combine the config log setting and cmd args with cmd args overriding config
        // values
        let mut log_settings: Vec<(String, String)> = Vec::new();
        if parser.contains("log") {
            for (module_name, log_level) in parser.section_items("log")? {
                check_log_level(&log_level)?;
                dict_set(&mut log_settings, &module_name, &log_level);
            }
        }

        if let Some(module_log_level) = &args.module_log_level {
            let override_log_settings = get_log_level_dict(module_log_level)?;
            for (name, level) in override_log_settings {
                dict_set(&mut log_settings, &name, &level);
            }
        }

        // Let's save the handler we've configured for later use
        let configured_handler = Config::log_handler(&parser, args)?;

        // [http]
        let mut http = get_section(
            &parser,
            "http",
            &[
                ("address", Target::Str, s("0.0.0.0")),
                ("port", Target::Int, int(8080)),
                ("link_info", Target::Bool, b(false)),
            ],
        )?;

        if let Some(address) = &args.address
            && !address.is_empty()
        {
            http.set("address", Value::Str(address.clone()));
        }
        if let Some(tcp_port) = args.tcp_port
            && tcp_port != 0
        {
            http.set("port", Value::Int(PyInt::from_i64(tcp_port)));
        }
        if let Some(true) = args.link_info {
            http.set("link_info", Value::Bool(true));
        }

        // [printer]
        let mut printer = get_section(
            &parser,
            "printer",
            &[
                ("port", Target::Str, s("auto")),
                ("baudrate", Target::Int, int(115200)),
                // Dangerous, it writes to the EEPROM on the little 32u2/8u2
                ("reset_disabling", Target::Bool, b(false)),
                ("settings", Target::Str, s("./prusa_printer_settings.ini")),
                // relative to HOME
                ("directory", Target::Str, s("./PrusaLink gcodes")),
            ],
        )?;
        if let Some(serial_port) = &args.serial_port
            && !serial_port.is_empty()
        {
            printer.set("port", Value::Str(serial_port.clone()));
        }

        let data_dir = daemon.attr("data_dir")?.to_string();
        let settings = printer.attr("settings")?.to_string();
        printer.set("settings", Value::Str(abspath_join(&data_dir, &settings)?));
        let directory = printer.attr("directory")?.to_string();
        let directory = abspath_join(&data_dir, &directory)?;
        // `Path(directory).name`: the last part of an absolute, normalised path
        let directory_name = pyos::basename(&directory).to_owned();
        printer.set("directory", Value::Str(directory));
        printer.set("directory_name", Value::Str(directory_name));

        // [cameras]
        let cameras = get_section(
            &parser,
            "cameras",
            &[("auto_detect", Target::Bool, b(true))],
        )?;

        Ok(Config {
            parser,
            debug,
            daemon,
            log_settings,
            configured_handler,
            http,
            printer,
            cameras,
        })
    }

    /// `get(section, option, fallback=fallback)`: the fallback for a missing section or option.
    fn get_or(
        parser: &ConfigParser,
        section: &str,
        option: &str,
        fallback: &str,
    ) -> Result<String, Error> {
        match parser.get(section, option) {
            Err(err) if err.class == "NoSectionError" || err.class == "NoOptionError" => {
                Ok(fallback.to_owned())
            }
            other => other,
        }
    }

    /// What `get_log_handler` (`config.py:242-261`) reads: the handler's kind, the syslog
    /// address and the format. Making the handler is not covered.
    pub fn log_handler(parser: &ConfigParser, args: &Args) -> Result<LogHandler, Error> {
        let (kind, address, log_format) = if args.foreground {
            ("StreamHandler", None, LOG_FORMAT_FOREGROUND.to_owned())
        } else {
            let log_syslog = Config::get_or(parser, "logging", "syslog", "/dev/log")?;
            (
                "SysLogHandler",
                Some(log_syslog),
                LOG_FORMAT_SYSLOG.to_owned(),
            )
        };
        let log_format = Config::get_or(parser, "logging", "format", &log_format)?;
        Ok(LogHandler {
            kind,
            address,
            format: log_format,
        })
    }

    /// `set_section` (`config.py:205-219`): Set section from model.
    fn set_section(&mut self, name: &str, model: &[(String, String)]) -> Result<(), Error> {
        if !self.parser.contains(name) {
            self.parser.add_section(name)?;
        }
        for (key, val) in model {
            // the `printer.storage` hack (`:211-213`): its option is commented out (`:184`)
            if name == "daemon" && key == "home" {
                continue;
            } else if name == "printer" && key == "directory_name" {
                continue;
            } else {
                self.parser.set(name, key, val)?;
            }
        }
        Ok(())
    }

    fn strs(model: &Model) -> Vec<(String, String)> {
        model
            .0
            .iter()
            .map(|(k, v)| (k.clone(), v.to_string()))
            .collect()
    }

    /// `update_sections` (`config.py:221-227`): Update config from attributes.
    pub fn update_sections(&mut self) -> Result<(), Error> {
        let daemon = Config::strs(&self.daemon);
        self.set_section("daemon", &daemon)?;
        let log = self.log_settings.clone();
        self.set_section("log", &log)?;
        let http = Config::strs(&self.http);
        self.set_section("http", &http)?;
        let printer = Config::strs(&self.printer);
        self.set_section("printer", &printer)?;
        let cameras = Config::strs(&self.cameras);
        self.set_section("cameras", &cameras)
    }
}

/// `Settings` (`config.py:264-354`): This class handles prusa_printer_settings.ini configuration
/// file, the official Prusa settings file, shared by all printers, which PrusaConnect can
/// generate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub parser: ConfigParser,
    pub printer: Model,
    pub network: Model,
    pub service_connect: Model,
    pub service_local: Model,
}

impl Settings {
    /// `Settings(settings_file)` (`config.py:272-322`)
    pub fn new(settings_file: &Path) -> Result<Settings, Error> {
        let mut parser = ConfigParser::new();

        parser.read(settings_file)?;

        // [printer]
        let mut printer = get_section(
            &parser,
            "printer",
            &[
                ("type", Target::Str, s("")),
                ("name", Target::Str, s("")),
                ("location", Target::Str, s("")),
                ("farm_mode", Target::Bool, b(false)),
                ("network_error_chime", Target::Bool, b(false)),
            ],
        )?;

        let name = strip(printer.str("name")?).to_owned();
        printer.set("name", Value::Str(name));
        let location = strip(printer.str("location")?).to_owned();
        printer.set("location", Value::Str(location));

        let type_ = printer.str("type")?;
        if !type_.is_empty() && !PRINTER_CONF_TYPES.iter().any(|(key, _)| *key == type_) {
            return Err(value_error(
                "Settings file for an unsupported printer".to_owned(),
            ));
        }

        // [network]
        let network = get_section(&parser, "network", &[("hostname", Target::Str, s(""))])?;

        // [service::connect]
        let service_connect = get_section(
            &parser,
            "service::connect",
            &[
                ("hostname", Target::Str, s(CONNECT)),
                ("tls", Target::Bool, b(true)),
                ("port", Target::Int, int(0)), // 0 means 443 with tls, or 80 without tls
                ("token", Target::Str, s("")),
            ],
        )?;

        // [service::local]
        let mut service_local = get_section(
            &parser,
            "service::local",
            &[
                ("enable", Target::Int, int(1)),
                ("username", Target::Str, s("")),
                ("digest", Target::Str, s("")),
                ("api_key", Target::Str, s("")),
            ],
        )?;
        let api_key = sanitize_api_key(service_local.str("api_key")?);
        service_local.set("api_key", Value::Str(api_key));

        let mut settings = Settings {
            parser,
            printer,
            network,
            service_connect,
            service_local,
        };

        // Reflect possible changes back to prusa_printer_settings.ini file
        settings.update_sections(false)?;
        settings.parser.write_file(settings_file)?;
        Ok(settings)
    }

    /// `set_section` (`config.py:324-329`): Set section from model.
    fn set_section(&mut self, name: &str, model: &Model) -> Result<(), Error> {
        if !self.parser.contains(name) {
            self.parser.add_section(name)?;
        }
        for (key, val) in &model.0 {
            self.parser.set(name, key, &val.to_string())?;
        }
        Ok(())
    }

    /// `update_sections(connect_skip)` (`config.py:331-337`): Update config from attributes.
    pub fn update_sections(&mut self, connect_skip: bool) -> Result<(), Error> {
        let printer = self.printer.clone();
        self.set_section("printer", &printer)?;
        let network = self.network.clone();
        self.set_section("network", &network)?;
        if !connect_skip {
            let service_connect = self.service_connect.clone();
            self.set_section("service::connect", &service_connect)?;
        }
        let service_local = self.service_local.clone();
        self.set_section("service::local", &service_local)
    }

    /// `is_wizard_needed` (`config.py:339-347`): Is there a reason for the wizard to be shown?
    pub fn is_wizard_needed(&self) -> bool {
        let interested_in = [
            self.printer.attr("type"),
            self.service_local.attr("username"),
            self.service_local.attr("digest"),
        ];
        !interested_in
            .iter()
            .all(|v| v.as_ref().is_ok_and(|v| v.truthy()))
    }

    /// `use_connect` (`config.py:349-354`): the user's wish to use Connect, now given by the
    /// token.
    pub fn use_connect(&self) -> bool {
        self.service_connect.attr("token").is_ok_and(|v| v.truthy())
    }
}

#[cfg(test)]
mod tests;
