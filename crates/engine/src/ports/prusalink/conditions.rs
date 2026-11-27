//! PrusaLink's `conditions.py` (`reference/Prusa-Link`, `171148ce`): PrusaLink error states
//! (#244, section B2). For more information see prusalink_states.txt.
//!
//! On top of the SDK's conditions ([`maker_connect_sdk::conditions`], the SDK's `conditions.py`):
//! PrusaLink's tree of conditions under `ROOT_COND` (`:26-79`), its two trackers (`:81-85`), the
//! status functions (`:88-122`) and the `LinkError`s (`:124-537`).
//!
//! **Differences:**
//! 1. The tree and the trackers are module globals in PrusaLink; here one [`Conditions`] per
//!    printer module, as the agent hosts several (#218; as `MCSingleton`, section A; proxy
//!    2026-11-27: approved). The SDK's chain (`INTERNET`, `HTTP`, `TOKEN`, `COND_TRACKER`) stays
//!    one per process, as in the SDK port ([`CHAIN`]): `INTERNET.set_parent(LAN)` (`:44`) joins
//!    it to the first module's `LAN`, and a second module's [`Conditions::new`] fails there with
//!    the SDK's own `ValueError` ("Can't replace a parent with another").
//! 2. `status()`'s order: PrusaLink lists `reversed(list(ROOT_COND))`, and a condition's
//!    children are a set (the SDK's `conditions.py:49`, `__iter__` `:163-166`), whose order
//!    follows memory addresses and is undefined; ours the order of making them, as the SDK port's
//!    [`Condition::iter`]. The same names, states and messages.
//! 3. A `LinkError` class is an [`ErrorClass`] (its class attributes as Python resolves them
//!    through the bases); an instance a [`LinkError`].
//! 4. `connect_status` reads `Settings.instance.use_connect()` (`:117`); here the module's
//!    [`Settings`] is its argument, as `Settings.instance` is one per printer module
//!    (`config.rs`).
//!
//! **Not covered:** `set_url`, `json_response` and `text_response` (`:147-149`, `:155-177`): poorwsgi's web
//! layer (`construct_url`, `JSONResponse`, `TextResponse`), which the agent does not have (plan
//! `docs/archive/plans/mk3-port-plan.md:54`); `from poorwsgi import state`'s codes are numbers
//! here.

use maker_connect_gcode_metadata::py::PyErr;
use maker_connect_sdk::conditions::{CHAIN, Condition, ConditionError, ConditionTracker};

use super::config::Settings;

/// The dicts `printer_status` and `connect_status` return: `{"ok": ..., "message": ...}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub ok: bool,
    pub message: String,
}

impl Status {
    /// `OK_MSG = {"ok": True, "message": "OK"}` (`:24`)
    pub fn ok_msg() -> Status {
        Status {
            ok: true,
            message: "OK".to_owned(),
        }
    }
}

/// PrusaLink's conditions of one printer module (`:26-85`).
pub struct Conditions {
    pub root_cond: Condition,
    pub device: Condition,
    pub phy: Condition,
    pub lan: Condition,
    pub serial: Condition,
    pub rpi_enabled: Condition,
    pub id: Condition,
    pub upgraded: Condition,
    pub fw: Condition,
    pub sn: Condition,
    pub job_id: Condition,
    pub hw: Condition,
    pub net_tracker: ConditionTracker,
    pub printer_tracker: ConditionTracker,
}

impl Conditions {
    /// What `conditions.py` does when imported (`:21-85`).
    pub fn new() -> Result<Conditions, ConditionError> {
        // assert HTTP is not None
        // assert TOKEN is not None
        let root_cond = Condition::new(
            "Root",
            "The root of everything, it's almost always OK",
            None,
            None,
            0,
        )?;

        let device = Condition::new(
            "Device",
            "Eth|WLAN device does not exist",
            Some(root_cond),
            Some("No WLAN device"),
            1020,
        )?;
        let phy = Condition::new(
            "Phy",
            "Eth|WLAN device is not connected",
            Some(device),
            Some("No WLAN conn"),
            1010,
        )?;
        let lan = Condition::new(
            "Lan",
            "Eth|WLAN has no IP address",
            Some(phy),
            Some("No WLAN IP addr"),
            1000,
        )?;

        CHAIN.internet.set_parent(lan)?;

        let serial = Condition::new(
            "Port",
            "Serial device does not exist",
            Some(root_cond),
            None,
            570,
        )?;
        let rpi_enabled = Condition::new(
            "RPIenabled",
            "RPi port is not enabled",
            Some(serial),
            None,
            560,
        )?;
        let id = Condition::new(
            "ID",
            "Device is not supported",
            Some(rpi_enabled),
            None,
            550,
        )?;
        let upgraded = Condition::new(
            "Upgraded",
            "Printer upgraded, re-register it",
            Some(id),
            None,
            500,
        )?;
        let fw = Condition::new(
            "Firmware",
            "Firmware is not up-to-date",
            Some(rpi_enabled),
            None,
            540,
        )?;
        let sn = Condition::new(
            "SN",
            "Serial number cannot be obtained",
            Some(rpi_enabled),
            None,
            530,
        )?;
        let job_id = Condition::new(
            "JobID",
            "Job ID cannot be obtained",
            Some(rpi_enabled),
            None,
            520,
        )?;
        let hw = Condition::new(
            "HW",
            "Firmware detected a hardware issue",
            Some(rpi_enabled),
            None,
            510,
        )?;

        CHAIN.tracker.add_tracked_condition_tree(root_cond);

        let net_tracker = ConditionTracker::new();
        net_tracker.add_tracked_condition_tree(device);

        let printer_tracker = ConditionTracker::new();
        printer_tracker.add_tracked_condition_tree(serial);

        Ok(Conditions {
            root_cond,
            device,
            phy,
            lan,
            serial,
            rpi_enabled,
            id,
            upgraded,
            fw,
            sn,
            job_id,
            hw,
            net_tracker,
            printer_tracker,
        })
    }

    /// `use_connect_errors(use_connect)` (`:88-95`): Set whether to use Connect related errors or
    /// not
    pub fn use_connect_errors(&self, use_connect: bool) {
        if use_connect {
            CHAIN.tracker.add_tracked_condition_tree(CHAIN.internet);
            self.net_tracker.add_tracked_condition_tree(CHAIN.internet);
        } else {
            CHAIN.tracker.remove_tracked_condition_tree(CHAIN.internet);
            self.net_tracker
                .remove_tracked_condition_tree(CHAIN.internet);
        }
    }

    /// `status()` (`:98-103`): Return a dict with representation of all current conditions
    pub fn status(&self) -> Vec<(String, (&'static str, String))> {
        let mut result: Vec<(String, (&'static str, String))> = vec![];
        for condition in self.root_cond.iter().into_iter().rev() {
            let value = (condition.state().name(), condition.long_msg());
            // `result[condition.name] = ...`: a name already there keeps its place
            match result
                .iter_mut()
                .find(|(name, _)| *name == condition.name())
            {
                Some((_, old)) => *old = value,
                None => result.push((condition.name(), value)),
            }
        }
        result
    }

    /// `printer_status()` (`:106-111`): Returns a representation of the currently broken printer
    /// condition
    pub fn printer_status(&self) -> Status {
        let Some(worst) = self.printer_tracker.get_worst() else {
            return Status::ok_msg();
        };
        Status {
            ok: false,
            message: worst.long_msg(),
        }
    }

    /// `connect_status()` (`:114-122`): Returns a representation of the currently broken Connect
    /// condition
    pub fn connect_status(&self, settings: &Settings) -> Status {
        let Some(worst) = self.net_tracker.get_worst() else {
            if !settings.use_connect() {
                return Status {
                    ok: true,
                    message: "Connect isn't configured".to_owned(),
                };
            }
            return Status::ok_msg();
        };
        Status {
            ok: false,
            message: worst.long_msg(),
        }
    }
}

/// A `LinkError` class (`:124-133`): its class attributes as Python resolves them through the
/// bases; `None` where neither the class nor a base sets one (`title`, `text`, `status_code` are
/// only annotated on `LinkError`).
#[derive(Debug)]
pub struct ErrorClass {
    pub name: &'static str,
    /// The base class; `None` for `LinkError`.
    pub parent: Option<&'static ErrorClass>,
    pub title: Option<&'static str>,
    pub text: Option<&'static str>,
    pub id: Option<&'static str>,
    pub status_code: Option<u16>,
    pub use_basic_template: bool,
}

impl ErrorClass {
    /// `issubclass(self, other)`
    pub fn is_subclass(&'static self, other: &ErrorClass) -> bool {
        let mut class = Some(self);
        while let Some(c) = class {
            if std::ptr::eq(c, other) {
                return true;
            }
            class = c.parent;
        }
        false
    }
}

/// An instance of a `LinkError` class.
#[derive(Debug, Clone)]
pub struct LinkError {
    pub class: &'static ErrorClass,
    pub details: Option<String>,
    pub path: Option<String>,
    pub url: String,
    pub template: String,
    /// `RuntimeError.args`: `(self.text,)`
    pub args: (String,),
}

impl LinkError {
    /// `LinkError.__init__(details="")` (`:135-145`), of `class`
    pub fn new(class: &'static ErrorClass, details: &str) -> Result<LinkError, PyErr> {
        let missing = |attribute: &str| {
            PyErr::Other(
                "AttributeError",
                format!("'{}' object has no attribute '{attribute}'", class.name),
            )
        };
        let details = if !details.is_empty() {
            Some(details.to_owned())
        } else {
            None
        };
        let path = class
            .id
            .filter(|id| !id.is_empty())
            .map(|id| format!("/error/{id}"));
        // pylint: disable=consider-using-f-string
        let template = if class.use_basic_template {
            "error.html".to_owned()
        } else {
            format!("error-{}.html", class.id.unwrap_or("None"))
        };
        let text = class.text.ok_or_else(|| missing("text"))?;
        Ok(LinkError {
            class,
            details,
            path,
            url: String::new(),
            template,
            args: (text.to_owned(),),
        })
    }

    /// `gen_headers()` (`:151-153`): Return headers with Content-Location if id was set.
    pub fn gen_headers(&self) -> Vec<(&'static str, String)> {
        if self.url.is_empty() {
            return vec![];
        }
        vec![("Content-Location", self.url.clone())]
    }
}

/// `LinkError(RuntimeError)` (`:124`): Link error structure.
pub static LINK_ERROR: ErrorClass = ErrorClass {
    name: "LinkError",
    parent: None,
    title: None,
    text: None,
    id: None,
    status_code: None,
    use_basic_template: true,
};

/// `BadRequestError(LinkError)` (`:179`): 400 Bad Request error
pub static BAD_REQUEST_ERROR: ErrorClass = ErrorClass {
    name: "BadRequestError",
    parent: Some(&LINK_ERROR),
    title: None,
    text: None,
    id: None,
    status_code: Some(400),
    use_basic_template: true,
};

/// `TemperatureTooLow(BadRequestError)` (`:184`): 400 Temperature is too low
pub static TEMPERATURE_TOO_LOW: ErrorClass = ErrorClass {
    name: "TemperatureTooLow",
    parent: Some(&BAD_REQUEST_ERROR),
    title: Some("Temperature too low"),
    text: Some("Desired temperature is too low"),
    id: Some("temperature-too-low"),
    status_code: Some(400),
    use_basic_template: true,
};

/// `TemperatureTooHigh(BadRequestError)` (`:191`): 400 Temperature is too high
pub static TEMPERATURE_TOO_HIGH: ErrorClass = ErrorClass {
    name: "TemperatureTooHigh",
    parent: Some(&BAD_REQUEST_ERROR),
    title: Some("Temperature too high"),
    text: Some("Desired temperature is too high"),
    id: Some("temperature-too-high"),
    status_code: Some(400),
    use_basic_template: true,
};

/// `ValueTooLow(BadRequestError)` (`:198`): 400 Generic value is too low
pub static VALUE_TOO_LOW: ErrorClass = ErrorClass {
    name: "ValueTooLow",
    parent: Some(&BAD_REQUEST_ERROR),
    title: Some("Value too low"),
    text: Some("Desired value is too low"),
    id: Some("value-too-low"),
    status_code: Some(400),
    use_basic_template: true,
};

/// `ValueTooHigh(BadRequestError)` (`:205`): 400 Generic value is too high
pub static VALUE_TOO_HIGH: ErrorClass = ErrorClass {
    name: "ValueTooHigh",
    parent: Some(&BAD_REQUEST_ERROR),
    title: Some("Value too high"),
    text: Some("Desired value is too high"),
    id: Some("value-too-high"),
    status_code: Some(400),
    use_basic_template: true,
};

/// `CantMoveAxis(BadRequestError)` (`:212`): 400 Can't Move Axis
pub static CANT_MOVE_AXIS: ErrorClass = ErrorClass {
    name: "CantMoveAxis",
    parent: Some(&BAD_REQUEST_ERROR),
    title: Some("Can't move axis"),
    text: Some("Can't move axis in current state"),
    id: Some("cant-move-axis"),
    status_code: Some(400),
    use_basic_template: true,
};

/// `CantMoveAxisZ(BadRequestError)` (`:219`): 400 Can't move axis in current state
pub static CANT_MOVE_AXIS_Z: ErrorClass = ErrorClass {
    name: "CantMoveAxisZ",
    parent: Some(&BAD_REQUEST_ERROR),
    title: Some("Can't Move Axis Z in current state"),
    text: Some("Axis Z can't be moved in current state"),
    id: Some("cant-move-axis-z"),
    status_code: Some(400),
    use_basic_template: true,
};

/// `DestinationSameAsSource(BadRequestError)` (`:226`): 400 Destination is same as source
pub static DESTINATION_SAME_AS_SOURCE: ErrorClass = ErrorClass {
    name: "DestinationSameAsSource",
    parent: Some(&BAD_REQUEST_ERROR),
    title: Some("Destination same as source"),
    text: Some("Destination to move file is same as the source of the file"),
    id: Some("destination-same-as-source"),
    status_code: Some(400),
    use_basic_template: true,
};

/// `NoFileInRequest(BadRequestError)` (`:233`): 400 File not found in request payload.
pub static NO_FILE_IN_REQUEST: ErrorClass = ErrorClass {
    name: "NoFileInRequest",
    parent: Some(&BAD_REQUEST_ERROR),
    title: Some("Missing file in payload."),
    text: Some("File is not send in request payload or it hasn't right name."),
    id: Some("no-file-in-request"),
    status_code: Some(400),
    use_basic_template: true,
};

/// `FileSizeMismatch(BadRequestError)` (`:240`): 400 File size mismatch.
pub static FILE_SIZE_MISMATCH: ErrorClass = ErrorClass {
    name: "FileSizeMismatch",
    parent: Some(&BAD_REQUEST_ERROR),
    title: Some("File Size Mismatch"),
    text: Some("You sent more or less data than is in Content-Length header."),
    id: Some("file-size-mismatch"),
    status_code: Some(400),
    use_basic_template: true,
};

/// `InvalidIniFileFormat(BadRequestError)` (`:247`): 400 Invalid ini file format.
pub static INVALID_INI_FILE_FORMAT: ErrorClass = ErrorClass {
    name: "InvalidIniFileFormat",
    parent: Some(&BAD_REQUEST_ERROR),
    title: Some("Invalid ini File Format"),
    text: Some("Format or the structure of your ini file is invalid."),
    id: Some("invalid-ini-file-format"),
    status_code: Some(400),
    use_basic_template: true,
};

/// `InvalidBooleanHeader(BadRequestError)` (`:254`): 400 Invalid Boolean Header
pub static INVALID_BOOLEAN_HEADER: ErrorClass = ErrorClass {
    name: "InvalidBooleanHeader",
    parent: Some(&BAD_REQUEST_ERROR),
    title: Some("Invalid Boolean Header"),
    text: Some("Invalid Boolean Header according to RFC8941 / 3.3.6"),
    id: Some("invalid-boolean-header"),
    status_code: Some(400),
    use_basic_template: true,
};

/// `ForbiddenCharacters(BadRequestError)` (`:261`): 400 Forbidden Characters.
pub static FORBIDDEN_CHARACTERS: ErrorClass = ErrorClass {
    name: "ForbiddenCharacters",
    parent: Some(&BAD_REQUEST_ERROR),
    title: Some("Forbidden Characters"),
    text: Some("Forbidden characters in file or folder name."),
    id: Some("forbidden-characters"),
    status_code: Some(400),
    use_basic_template: true,
};

/// `FilenameTooLong(BadRequestError)` (`:268`): 400 Filename Too Long
pub static FILENAME_TOO_LONG: ErrorClass = ErrorClass {
    name: "FilenameTooLong",
    parent: Some(&BAD_REQUEST_ERROR),
    title: Some("Filename Too Long"),
    text: Some("File name length is too long"),
    id: Some("filename-too-long"),
    status_code: Some(400),
    use_basic_template: true,
};

/// `FoldernameTooLong(BadRequestError)` (`:275`): 400 Foldername Too Long
pub static FOLDERNAME_TOO_LONG: ErrorClass = ErrorClass {
    name: "FoldernameTooLong",
    parent: Some(&BAD_REQUEST_ERROR),
    title: Some("Foldername Too Long"),
    text: Some("Folder name length is too long"),
    id: Some("foldername-too-long"),
    status_code: Some(400),
    use_basic_template: true,
};

/// `FileUploadFailed(BadRequestError)` (`:282`): 400 File Upload Failed
pub static FILE_UPLOAD_FAILED: ErrorClass = ErrorClass {
    name: "FileUploadFailed",
    parent: Some(&BAD_REQUEST_ERROR),
    title: Some("File Upload Failed"),
    text: Some("File upload has failed"),
    id: Some("file-upload-failed"),
    status_code: Some(400),
    use_basic_template: true,
};

/// `CantConnect(BadRequestError)` (`:289`): 400 Can't connect to PrusaConnect
pub static CANT_CONNECT: ErrorClass = ErrorClass {
    name: "CantConnect",
    parent: Some(&BAD_REQUEST_ERROR),
    title: Some("Can't Connect"),
    text: Some("Can't connect to PrusaConnect"),
    id: Some("cant-connect"),
    status_code: Some(400),
    use_basic_template: true,
};

/// `CantResolveHostname(BadRequestError)` (`:296`): 400 Can't resolve PrusaConnect hostname
pub static CANT_RESOLVE_HOSTNAME: ErrorClass = ErrorClass {
    name: "CantResolveHostname",
    parent: Some(&BAD_REQUEST_ERROR),
    title: Some("Can't resolve hostname"),
    text: Some("Can't resolve PrusaConnect hostname"),
    id: Some("cant-resolve-hostname"),
    status_code: Some(400),
    use_basic_template: true,
};

/// `NotSupportedFileType(LinkError)` (`:303`): 415 Not supported file
pub static NOT_SUPPORTED_FILE_TYPE: ErrorClass = ErrorClass {
    name: "NotSupportedFileType",
    parent: Some(&LINK_ERROR),
    title: Some("Not Supported File Type"),
    text: Some("Uploaded file type is not supported."),
    id: Some("not-supported-file-type"),
    status_code: Some(415),
    use_basic_template: true,
};

/// `ForbiddenError(LinkError)` (`:311`): 403 Forbidden
pub static FORBIDDEN_ERROR: ErrorClass = ErrorClass {
    name: "ForbiddenError",
    parent: Some(&LINK_ERROR),
    title: Some("Forbidden"),
    text: Some("You don not have permission to access this."),
    id: Some("forbidden"),
    status_code: Some(403),
    use_basic_template: true,
};

/// `NotFoundError(LinkError)` (`:319`): 404 Not Found error
pub static NOT_FOUND_ERROR: ErrorClass = ErrorClass {
    name: "NotFoundError",
    parent: Some(&LINK_ERROR),
    title: Some("Not Found"),
    text: Some("Resource you want not found."),
    id: Some("not-found"),
    status_code: Some(404),
    use_basic_template: true,
};

/// `NotCurrentJob(NotFoundError)` (`:327`): 404 Not current job
pub static NOT_CURRENT_JOB: ErrorClass = ErrorClass {
    name: "NotCurrentJob",
    parent: Some(&NOT_FOUND_ERROR),
    title: Some("Not Current Job"),
    text: Some("Given job id does not belong to current job"),
    id: Some("not-found"),
    status_code: Some(404),
    use_basic_template: true,
};

/// `GoneError(LinkError)` (`:333`): 410 Gone
pub static GONE_ERROR: ErrorClass = ErrorClass {
    name: "GoneError",
    parent: Some(&LINK_ERROR),
    title: Some("Target Resource Unavailable"),
    text: Some("Target resource is unavailable."),
    id: Some("file-gone"),
    status_code: Some(410),
    use_basic_template: true,
};

/// `ThumbnailUnavailable(GoneError)` (`:341`): 410 Thumbnail Unavailable
pub static THUMBNAIL_UNAVAILABLE: ErrorClass = ErrorClass {
    name: "ThumbnailUnavailable",
    parent: Some(&GONE_ERROR),
    title: Some("Thumbnail Unavailable"),
    text: Some("Thumbnail is unavailable."),
    id: Some("file-gone"),
    status_code: Some(410),
    use_basic_template: true,
};

/// `FileNotFound(NotFoundError)` (`:347`): 404 File Not Found
pub static FILE_NOT_FOUND: ErrorClass = ErrorClass {
    name: "FileNotFound",
    parent: Some(&NOT_FOUND_ERROR),
    title: Some("File Not Found"),
    text: Some("File you want was not found."),
    id: Some("not-found"),
    status_code: Some(404),
    use_basic_template: true,
};

/// `FolderNotFound(NotFoundError)` (`:353`): 404 Folder Not Found
pub static FOLDER_NOT_FOUND: ErrorClass = ErrorClass {
    name: "FolderNotFound",
    parent: Some(&NOT_FOUND_ERROR),
    title: Some("Folder Not Found"),
    text: Some("Folder you want was not found."),
    id: Some("not-found"),
    status_code: Some(404),
    use_basic_template: true,
};

/// `LocationNotFound(NotFoundError)` (`:359`): 404 Location from url not found.
pub static LOCATION_NOT_FOUND: ErrorClass = ErrorClass {
    name: "LocationNotFound",
    parent: Some(&NOT_FOUND_ERROR),
    title: Some("Location Not Found"),
    text: Some("Location not found, use local."),
    id: Some("location-not-found"),
    status_code: Some(404),
    use_basic_template: true,
};

/// `ConflictError(LinkError)` (`:367`): 409 Conflict error.
pub static CONFLICT_ERROR: ErrorClass = ErrorClass {
    name: "ConflictError",
    parent: Some(&LINK_ERROR),
    title: None,
    text: None,
    id: None,
    status_code: Some(409),
    use_basic_template: true,
};

/// `DirectoryNotEmpty(ConflictError)` (`:372`): 409 Directory is not empty
pub static DIRECTORY_NOT_EMPTY: ErrorClass = ErrorClass {
    name: "DirectoryNotEmpty",
    parent: Some(&CONFLICT_ERROR),
    title: Some("Directory is not empty"),
    text: Some("Directory can't be deleted, because it's not empty."),
    id: Some("directory-not-empty"),
    status_code: Some(409),
    use_basic_template: true,
};

/// `CurrentlyPrinting(ConflictError)` (`:379`): 409 Printer is currently printing
pub static CURRENTLY_PRINTING: ErrorClass = ErrorClass {
    name: "CurrentlyPrinting",
    parent: Some(&CONFLICT_ERROR),
    title: Some("Printer is currently printing"),
    text: Some("Printer is currently printing."),
    id: None,
    status_code: Some(409),
    use_basic_template: true,
};

/// `NotStateToPrint(ConflictError)` (`:385`): 409 Printer is not in state to print
pub static NOT_STATE_TO_PRINT: ErrorClass = ErrorClass {
    name: "NotStateToPrint",
    parent: Some(&CONFLICT_ERROR),
    title: Some("Not in state to print"),
    text: Some("Printer is not in state to print."),
    id: Some("not-state-to-print"),
    status_code: Some(409),
    use_basic_template: true,
};

/// `NotPrinting(ConflictError)` (`:392`): 409 Printer is not printing
pub static NOT_PRINTING: ErrorClass = ErrorClass {
    name: "NotPrinting",
    parent: Some(&CONFLICT_ERROR),
    title: Some("Printer Is Not Printing"),
    text: Some("Operation you want can only be done when printer is printing."),
    id: None,
    status_code: Some(409),
    use_basic_template: true,
};

/// `NotPaused(ConflictError)` (`:398`): 409 Printer is not paused
pub static NOT_PAUSED: ErrorClass = ErrorClass {
    name: "NotPaused",
    parent: Some(&CONFLICT_ERROR),
    title: Some("Printer Is Not Paused"),
    text: Some("Operation you want can only be done when printer is paused."),
    id: None,
    status_code: Some(409),
    use_basic_template: true,
};

/// `FileCurrentlyPrinted(ConflictError)` (`:404`): 409 File is currently printed
pub static FILE_CURRENTLY_PRINTED: ErrorClass = ErrorClass {
    name: "FileCurrentlyPrinted",
    parent: Some(&CONFLICT_ERROR),
    title: Some("File is currently printed"),
    text: Some("You try to do an operation with the file, which is currently printed."),
    id: Some("file-currently-printed"),
    status_code: Some(409),
    use_basic_template: true,
};

/// `TransferConflict(ConflictError)` (`:412`): 409 Already in transfer process.
pub static TRANSFER_CONFLICT: ErrorClass = ErrorClass {
    name: "TransferConflict",
    parent: Some(&CONFLICT_ERROR),
    title: Some("Already in transfer process"),
    text: Some("Only one file at time can be transferred."),
    id: Some("transfer-conflict"),
    status_code: Some(409),
    use_basic_template: true,
};

/// `TransferStopped(ConflictError)` (`:420`): 409 Transfer process was stopped by user.
pub static TRANSFER_STOPPED: ErrorClass = ErrorClass {
    name: "TransferStopped",
    parent: Some(&CONFLICT_ERROR),
    title: Some("Transfer stopped"),
    text: Some("Transfer process was stopped by user."),
    id: Some("transfer-stopped"),
    status_code: Some(409),
    use_basic_template: true,
};

/// `UnavailableUpdate(ConflictError)` (`:427`): 409 Update is unavailable to install
pub static UNAVAILABLE_UPDATE: ErrorClass = ErrorClass {
    name: "UnavailableUpdate",
    parent: Some(&CONFLICT_ERROR),
    title: Some("Unavailable update"),
    text: Some("Update is unavailable to install"),
    id: Some("unavailable-update"),
    status_code: Some(409),
    use_basic_template: true,
};

/// `UnableToUpdate(ConflictError)` (`:435`): 409 Unable to install update
pub static UNABLE_TO_UPDATE: ErrorClass = ErrorClass {
    name: "UnableToUpdate",
    parent: Some(&CONFLICT_ERROR),
    title: Some("Unable to update"),
    text: Some("Unable to install update"),
    id: Some("unable-to-update"),
    status_code: Some(409),
    use_basic_template: true,
};

/// `LengthRequired(LinkError)` (`:443`): 411 Length Required.
pub static LENGTH_REQUIRED: ErrorClass = ErrorClass {
    name: "LengthRequired",
    parent: Some(&LINK_ERROR),
    title: Some("Length Required"),
    text: Some("Missing Content-Length header or no content in request."),
    id: Some("length-required"),
    status_code: Some(411),
    use_basic_template: true,
};

/// `EntityTooLarge(LinkError)` (`:451`): 413 Payload Too Large
pub static ENTITY_TOO_LARGE: ErrorClass = ErrorClass {
    name: "EntityTooLarge",
    parent: Some(&LINK_ERROR),
    title: Some("Request Entity Too Large"),
    text: Some("Not enough space in storage."),
    id: Some("entity-too-large"),
    status_code: Some(413),
    use_basic_template: true,
};

/// `FileAlreadyExists(LinkError)` (`:459`): 409 File Already Exists
pub static FILE_ALREADY_EXISTS: ErrorClass = ErrorClass {
    name: "FileAlreadyExists",
    parent: Some(&LINK_ERROR),
    title: Some("File Already Exists"),
    text: Some("File already exists."),
    id: Some("file-already-exists"),
    status_code: Some(409),
    use_basic_template: true,
};

/// `FolderAlreadyExists(LinkError)` (`:467`): 409 Folder Already Exists
pub static FOLDER_ALREADY_EXISTS: ErrorClass = ErrorClass {
    name: "FolderAlreadyExists",
    parent: Some(&LINK_ERROR),
    title: Some("Folder Already Exists"),
    text: Some("Folder already exists."),
    id: Some("folder-already-exists"),
    status_code: Some(409),
    use_basic_template: true,
};

/// `StorageNotExist(LinkError)` (`:475`): 409 Storage Does Not Exist
pub static STORAGE_NOT_EXIST: ErrorClass = ErrorClass {
    name: "StorageNotExist",
    parent: Some(&LINK_ERROR),
    title: Some("Storage Does Not Exist"),
    text: Some("Storage doest not exist."),
    id: Some("storage-not-exist"),
    status_code: Some(409),
    use_basic_template: true,
};

/// `SDCardReadOnly(LinkError)` (`:483`): 409 SD Card Read Only
pub static SD_CARD_READ_ONLY: ErrorClass = ErrorClass {
    name: "SDCardReadOnly",
    parent: Some(&LINK_ERROR),
    title: Some("SD Card Read Only"),
    text: Some("SD Card storage is read only."),
    id: Some("sdcard-read-only"),
    status_code: Some(409),
    use_basic_template: true,
};

/// `SDCardNotSupported(LinkError)` (`:491`): 409 Some operations are not possible on SDCard.
pub static SD_CARD_NOT_SUPPORTED: ErrorClass = ErrorClass {
    name: "SDCardNotSupported",
    parent: Some(&LINK_ERROR),
    title: Some("SDCard is not Suppported"),
    text: Some("Location `sdcard` is not supported, use local."),
    id: Some("sdcard-not-supported"),
    status_code: Some(409),
    use_basic_template: true,
};

/// `UnsupportedMediaError(LinkError)` (`:499`): 415 Unsupported Media Type
pub static UNSUPPORTED_MEDIA_ERROR: ErrorClass = ErrorClass {
    name: "UnsupportedMediaError",
    parent: Some(&LINK_ERROR),
    title: Some("Unsupported Media Type"),
    text: Some("Only G-Code for FDM printer can be uploaded."),
    id: Some("unsupported-media-type"),
    status_code: Some(415),
    use_basic_template: true,
};

/// `InternalServerError(LinkError)` (`:507`): 500 Internal Server Error.
pub static INTERNAL_SERVER_ERROR: ErrorClass = ErrorClass {
    name: "InternalServerError",
    parent: Some(&LINK_ERROR),
    title: Some("Internal Server Error"),
    text: Some("We're sorry, but there is a error in service. Please try again later."),
    id: Some("internal-server-error"),
    status_code: Some(500),
    use_basic_template: true,
};

/// `ResponseTimeout(InternalServerError)` (`:516`): 500 Response Timeout
pub static RESPONSE_TIMEOUT: ErrorClass = ErrorClass {
    name: "ResponseTimeout",
    parent: Some(&INTERNAL_SERVER_ERROR),
    title: Some("Response Timeout"),
    text: Some("There is some problem on PrusaLink."),
    id: Some("response-timeout"),
    status_code: Some(500),
    use_basic_template: true,
};

/// `PrinterUnavailable(LinkError)` (`:523`): 503 Printer Unavailable.
pub static PRINTER_UNAVAILABLE: ErrorClass = ErrorClass {
    name: "PrinterUnavailable",
    parent: Some(&LINK_ERROR),
    title: Some("Printer Unavailable."),
    text: Some("PrusaLink not finished initializing or Printer not connected."),
    id: Some("printer-unavailable"),
    status_code: Some(503),
    use_basic_template: true,
};

/// `RequestTimeout(LinkError)` (`:531`): 408 Request timeout.
pub static REQUEST_TIMEOUT: ErrorClass = ErrorClass {
    name: "RequestTimeout",
    parent: Some(&LINK_ERROR),
    title: Some("Request timeout."),
    text: Some("PrusaLink got tired of waiting for your request. cancelled upload?"),
    id: Some("request-timeout"),
    status_code: Some(408),
    use_basic_template: true,
};
#[cfg(test)]
mod tests;
