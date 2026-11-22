//! Constants and enums for Printer (`const.py`).
//!
//! The camera part of `const.py` (from `NotSupported` on) belongs to the camera module, which
//! step A does not port.

use std::fmt;
use std::str::FromStr;

/// 100ms
pub const TIMESTAMP_PRECISION: f64 = 0.1;
/// 10s
pub const CONNECTION_TIMEOUT: u64 = 10;
/// 3s
pub const CAMERA_WAIT_TIMEOUT: u64 = 3;
/// 1s
pub const ONE_SECOND_TIMEOUT: u64 = 1;
pub const GCODE_EXTENSIONS: [&str; 4] = [".gcode", ".gc", ".g", ".gco"];
pub const FIRMWARE_EXTENSION: &str = ".hex";
pub const SL_EXTENSIONS: [&str; 1] = [".sl1"];
/// 20s
pub const CAMERA_BUSY_TIMEOUT: u64 = 20;

/// Maximum length of filename, including .gcode suffix
pub const FILENAME_LENGTH: usize = 248;

/// Maximum length of element name in path
pub const MAX_NAME_LENGTH: usize = 255;

/// Characters, forbidden in file name or path
pub const FORBIDDEN_CHARACTERS: [char; 8] = ['\\', '?', '"', '%', '¯', '°', '#', 'ˇ'];

pub const METADATA_MAX_OFFSET: usize = 2000; // bytes from the start and end of a file
pub const METADATA_CHUNK_SIZE: usize = 200;
pub const COMMENT_BLOCK_MAX_SIZE: usize = 1_000_000; // Max 1MB of comments

/// The `ValueError` of `Enum(value)` for a value that is not a member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotAMember {
    pub value: String,
    pub enum_name: &'static str,
}

impl fmt::Display for NotAMember {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Python: "'X' is not a valid Command"
        write!(f, "'{}' is not a valid {}", self.value, self.enum_name)
    }
}

impl std::error::Error for NotAMember {}

/// A Python `Enum` whose values are strings: `.value`, `Enum(value)`, and serde as the value.
macro_rules! str_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $value:literal,)* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant,)*
        }

        impl $name {
            /// Every member, in definition order.
            pub const ALL: &'static [$name] = &[$($name::$variant,)*];

            /// `.value`
            pub fn value(self) -> &'static str {
                match self {
                    $($name::$variant => $value,)*
                }
            }
        }

        impl FromStr for $name {
            type Err = NotAMember;

            /// `Enum(value)`
            fn from_str(value: &str) -> Result<Self, NotAMember> {
                match value {
                    $($value => Ok($name::$variant),)*
                    _ => Err(NotAMember { value: value.to_owned(), enum_name: stringify!($name) }),
                }
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.value())
            }
        }

        impl From<$name> for serde_json::Value {
            fn from(member: $name) -> serde_json::Value {
                serde_json::Value::String(member.value().to_owned())
            }
        }
    };
}

str_enum! {
    /// Printer could be in one of this state.
    State {
        Idle = "IDLE",
        Busy = "BUSY",
        Printing = "PRINTING",
        Paused = "PAUSED",
        Finished = "FINISHED",
        Stopped = "STOPPED",
        Error = "ERROR",
        Attention = "ATTENTION",
        Ready = "READY",
    }
}

str_enum! {
    /// Job can be in one of this state.
    JobState {
        Printing = "PRINTING",
        Paused = "PAUSED",
        FinStopped = "FIN_STOPPED",
        FinError = "FIN_ERROR",
        FinOk = "FIN_OK",
        FinHarvested = "FIN_HARVESTED",
    }
}

/// Printer Type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrinterType {
    I3Mk25,
    I3Mk25S,
    I3Mk3,
    I3Mk3S,
    Sl1,
    Sl1S,
    M1,
    Ht90,
}

impl PrinterType {
    pub const ALL: &'static [PrinterType] = &[
        PrinterType::I3Mk25,
        PrinterType::I3Mk25S,
        PrinterType::I3Mk3,
        PrinterType::I3Mk3S,
        PrinterType::Sl1,
        PrinterType::Sl1S,
        PrinterType::M1,
        PrinterType::Ht90,
    ];

    /// `.value`
    pub fn value(self) -> (u32, u32, u32) {
        match self {
            PrinterType::I3Mk25 => (1, 2, 5),
            PrinterType::I3Mk25S => (1, 2, 6),
            PrinterType::I3Mk3 => (1, 3, 0),
            PrinterType::I3Mk3S => (1, 3, 1),
            PrinterType::Sl1 => (5, 1, 0),
            PrinterType::Sl1S => (5, 1, 1),
            PrinterType::M1 => (5, 2, 0),
            PrinterType::Ht90 => (6, 1, 0),
        }
    }

    /// `PrinterType(value)`
    pub fn from_value(value: (u32, u32, u32)) -> Option<PrinterType> {
        PrinterType::ALL
            .iter()
            .copied()
            .find(|member| member.value() == value)
    }
}

impl fmt::Display for PrinterType {
    /// `'.'.join(str(i) for i in self.value)`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (a, b, c) = self.value();
        write!(f, "{a}.{b}.{c}")
    }
}

str_enum! {
    /// MMU Type
    MmuType {
        Mmu3 = "MMU3",
    }
}

/// `MMU_SLOT_COUNTS`
pub fn mmu_slot_count(mmu: MmuType) -> u32 {
    match mmu {
        MmuType::Mmu3 => 5,
    }
}

str_enum! {
    /// Storage Type
    StorageType {
        Local = "LOCAL",
        Sdcard = "SDCARD",
        SdCard = "SD_CARD",
        Usb = "USB",
        Link = "LINK",
        Other = "OTHER",
    }
}

str_enum! {
    /// Events known by Connect.
    Event {
        Accepted = "ACCEPTED",
        Rejected = "REJECTED",
        Failed = "FAILED",
        Finished = "FINISHED",

        Info = "INFO",
        StateChanged = "STATE_CHANGED",

        MediumEjected = "MEDIUM_EJECTED",
        MediumInserted = "MEDIUM_INSERTED",
        FileChanged = "FILE_CHANGED",
        FileInfo = "FILE_INFO",
        JobInfo = "JOB_INFO",
        TransferInfo = "TRANSFER_INFO",
        MeshBedData = "MESH_BED_DATA",

        TransferAborted = "TRANSFER_ABORTED",
        TransferStopped = "TRANSFER_STOPPED",
        TransferFinished = "TRANSFER_FINISHED",

        SlotEvent = "SLOT_EVENT",
    }
}

str_enum! {
    /// Printer event source.
    Source {
        Connect = "CONNECT",
        Gui = "GUI",
        Wui = "WUI",
        Serial = "SERIAL",
        Gcode = "GCODE",
        Marlin = "MARLIN",
        Firmware = "FIRMWARE",
        Hw = "HW",
        User = "USER",
        Slot = "SLOT",
    }
}

str_enum! {
    /// Commands which could be sent by Connect.
    Command {
        SendInfo = "SEND_INFO",
        Gcode = "GCODE",

        StartPrint = "START_PRINT",
        StopPrint = "STOP_PRINT",
        PausePrint = "PAUSE_PRINT",
        ResumePrint = "RESUME_PRINT",

        SendFileInfo = "SEND_FILE_INFO",
        DeleteFile = "DELETE_FILE",
        DeleteFolder = "DELETE_FOLDER",
        DeleteDirectory = "DELETE_DIRECTORY",
        CreateFolder = "CREATE_FOLDER",
        CreateDirectory = "CREATE_DIRECTORY",
        SendJobInfo = "SEND_JOB_INFO",
        SendStateInfo = "SEND_STATE_INFO",

        Upgrade = "UPGRADE",
        ResetPrinter = "RESET_PRINTER",

        StartUrlDownload = "START_URL_DOWNLOAD",
        StartConnectDownload = "START_CONNECT_DOWNLOAD",
        SendTransferInfo = "SEND_TRANSFER_INFO",
        StopTransfer = "STOP_TRANSFER",

        SetPrinterReady = "SET_PRINTER_READY",
        CancelPrinterReady = "CANCEL_PRINTER_READY",
        SetValue = "SET_VALUE",

        LoadFilament = "LOAD_FILAMENT",
        UnloadFilament = "UNLOAD_FILAMENT",
        SlotAction = "SLOT_ACTION",
        DialogAction = "DIALOG_ACTION",

        // Not the SDK's: Maker Connect's extension, Part 4 (`grbl.commands`,
        // docs/design/prusa-sdk-extension.md, "New commands").
        Home = "HOME",
        Unlock = "UNLOCK",
        Jog = "JOG",
        SetOrigin = "SET_ORIGIN",
        GoToOrigin = "GO_TO_ORIGIN",
    }
}

str_enum! {
    /// File transfer types
    TransferType {
        NoTransfer = "NO_TRANSFER",
        FromWeb = "FROM_WEB",         // from URL
        FromConnect = "FROM_CONNECT", // from URL using Connect
        FromPrinter = "FROM_PRINTER", // from URL using another printer
        FromSlicer = "FROM_SLICER",   // from Slicer software
        FromClient = "FROM_CLIENT",   // from computer
        ToConnect = "TO_CONNECT",     // uploading to Connect
        ToClient = "TO_CLIENT",       // downloading to computer
    }
}

str_enum! {
    /// File type
    FileType {
        PrintFile = "PRINT_FILE",
        Firmware = "FIRMWARE",
        File = "FILE",
        Folder = "FOLDER",
    }
}

str_enum! {
    /// Status of registration to Connect
    RegistrationStatus {
        Finished = "FINISHED",            // Finished registration
        InProgress = "IN_PROGRESS",       // Unfinished registration, code exists
        NoRegistration = "NO_REGISTRATION", // No registration, code doesn't exist
    }
}

/// Drop everything and execute these when they come
pub const PRIORITY_COMMANDS: [Command; 1] = [Command::ResetPrinter];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip() {
        for command in Command::ALL {
            assert_eq!(command.value().parse::<Command>(), Ok(*command));
        }
        // The SDK's 27, and Part 4's five.
        assert_eq!(Command::ALL.len(), 32);
        assert_eq!(Event::ALL.len(), 17);
        assert_eq!(Source::ALL.len(), 10);
        assert_eq!(State::ALL.len(), 9);
    }

    #[test]
    fn unknown_value_is_the_python_value_error() {
        let err = "PRINT_LABEL".parse::<Command>().unwrap_err();
        assert_eq!(err.to_string(), "'PRINT_LABEL' is not a valid Command");
    }

    #[test]
    fn printer_type_str() {
        assert_eq!(PrinterType::I3Mk3S.to_string(), "1.3.1");
        assert_eq!(PrinterType::from_value((5, 1, 1)), Some(PrinterType::Sl1S));
        assert_eq!(PrinterType::from_value((9, 9, 9)), None);
    }

    #[test]
    fn serialises_as_the_value() {
        assert_eq!(serde_json::to_string(&State::Idle).unwrap(), "\"IDLE\"");
        assert_eq!(
            serde_json::Value::from(Source::Wui),
            serde_json::json!("WUI")
        );
    }
}
