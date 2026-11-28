//! PrusaLink's `printer_adapter/model.py` (`reference/Prusa-Link`, `171148ce`): Contains
//! implementation of the Model class (#246, section D; moved in from section E, owner
//! 2026-11-27).
//!
//! The ten module data fields are annotations only in the source (`:29-38`): an instance has none
//! of them until its module assigns it, and reading one before that is `AttributeError`. Here each
//! is an `Option`, `None` until set; [`Model::get`] reads one as the source does.
//!
//! **Differs:** `metaclass=MCSingleton` (`:18`): one model per printer module (section A,
//! `mc_singleton.rs`), not per process.
//!
//! **Not covered:** the class attribute `latest_telemetry: Telemetry = Telemetry()` (`:23`), read
//! through the class (`Model.latest_telemetry`): nothing in PrusaLink reads it there; every read is
//! of an instance's (`:41`).

use maker_connect_gcode_metadata::py::PyErr;

use super::model_classes::Telemetry;
use super::module_data_classes::{
    FilePrinterData, IPUpdaterData, JobData, MMUObserverData, PrintStatsData, SDCardData,
    SerialAdapterData, StateManagerData, StorageData,
};

/// `Model` (`:18-41`): This class should collect every bit of info from all the informer classes.
/// Some values are reset upon reading, other, more state oriented should stay
#[derive(Default)]
pub struct Model {
    pub latest_telemetry: Telemetry,
    // Let's try and share inner module states for cooperation
    // The idea is, every module will get the model.
    // Every component HAS TO write its OWN INFO ONLY but can read
    // everything
    pub serial_adapter: Option<SerialAdapterData>,
    pub file_printer: Option<FilePrinterData>,
    pub print_stats: Option<PrintStatsData>,
    pub state_manager: Option<StateManagerData>,
    pub job: Option<JobData>,
    pub ip_updater: Option<IPUpdaterData>,
    pub sd_card: Option<SDCardData>,
    pub folder_storage: Option<StorageData>,
    pub filesystem_storage: Option<StorageData>,
    pub mmu_observer: Option<MMUObserverData>,
}

impl Model {
    /// `__init__` (`:40-41`): `self.latest_telemetry: Telemetry = Telemetry()`
    pub fn new() -> Model {
        Model::default()
    }

    /// `self.<name>` of a field not yet assigned (`:29-38`, annotations only): CPython's
    /// `AttributeError` `"'Model' object has no attribute '<name>'"`.
    pub fn get<'a, T>(field: &'a Option<T>, name: &str) -> Result<&'a T, PyErr> {
        field.as_ref().ok_or_else(|| {
            PyErr::AttributeError(format!("'Model' object has no attribute '{name}'"))
        })
    }
}

#[cfg(test)]
mod tests;
