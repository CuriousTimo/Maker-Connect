use super::*;
use crate::ports::prusalink::heap::Shared;
use crate::ports::prusalink::python;
use serde_json::{Value, json};

fn read<T>(field: &Option<T>, name: &str) -> Value {
    match Model::get(field, name) {
        Ok(_) => json!({"ok": true}),
        Err(error) => json!({"error": [error.class_name(), error.message()]}),
    }
}

fn fields(m: &Model) -> Vec<Value> {
    vec![
        read(&m.serial_adapter, "serial_adapter"),
        read(&m.file_printer, "file_printer"),
        read(&m.print_stats, "print_stats"),
        read(&m.state_manager, "state_manager"),
        read(&m.job, "job"),
        read(&m.ip_updater, "ip_updater"),
        read(&m.sd_card, "sd_card"),
        read(&m.folder_storage, "folder_storage"),
        read(&m.filesystem_storage, "filesystem_storage"),
        read(&m.mmu_observer, "mmu_observer"),
    ]
}

/// PrusaLink's `Model()` and ours: a default telemetry, every field `AttributeError` until set,
/// a set one read back, the others still unset.
#[test]
fn identical_to_prusalink() {
    let Some(theirs) = python::run(include_str!("../driver_model.py"), &json!({})) else {
        return;
    };
    let mut model = Model::new();
    let unset = fields(&model);
    let telemetry_is_default = model.latest_telemetry == Telemetry::default();
    model.serial_adapter = Some(Shared::new(std::sync::Mutex::new(SerialAdapterData {
        reset_disabling: false,
        ..SerialAdapterData::default()
    })));
    let reset_disabling = Model::get(&model.serial_adapter, "serial_adapter")
        .unwrap()
        .lock()
        .unwrap()
        .reset_disabling;
    // `self.data = model.serial_adapter` (`serial_adapter.py:92`), then changed through the data.
    let data = Model::get(&model.serial_adapter, "serial_adapter")
        .unwrap()
        .clone();
    data.lock().unwrap().reset_disabling = true;
    let shared = Model::get(&model.serial_adapter, "serial_adapter")
        .unwrap()
        .lock()
        .unwrap()
        .reset_disabling;
    let ours = json!({
        "telemetry_is_default": telemetry_is_default,
        "unset": unset,
        "set": read(&model.serial_adapter, "serial_adapter"),
        "reset_disabling": reset_disabling,
        "shared": shared,
        "others_still_unset": fields(&model)[1..],
    });
    assert_eq!(ours, theirs);
}
