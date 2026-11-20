//! `tests/test_metadata.py` of the reference (prusa3d/gcode-metadata at 7d8f25d, LGPLv2+),
//! test by test, on our port.
//!
//! The reference's test files are in `reference/gcode-metadata/tests/gcodes/` (not in the
//! repository, nor in CI): without them the tests that read them say so and pass.
//! `pytest.raises(E, match=...)`: the error's class, and its message containing the text.
//!
//! Left out with SL1 (owner, 2026-11-20): `TestSLMetaData` (4 tests) and `TestSLMetaDataBudget`
//! (7 tests); they are on the local branch `sl1-parked`.

use maker_connect_gcode_metadata::metadata::{
    MMU_ATTRS, MetaData, MetaError, VERSION, get_meta_class, get_metadata,
};
use maker_connect_gcode_metadata::py::{self, Dict, PyValue};
use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

// --- the module's helpers ------------------------------------------------------------------

fn gcodes_dir() -> Option<String> {
    let dir = format!(
        "{}/../../reference/gcode-metadata/tests/gcodes",
        env!("CARGO_MANIFEST_DIR")
    );
    if Path::new(&dir).is_dir() {
        Some(dir)
    } else {
        eprintln!("skipped: the reference's test files are not here ({dir})");
        None
    }
}

/// `tempfile.TemporaryDirectory()`, removed when dropped.
struct TmpDir(String);

impl TmpDir {
    fn new(test: &str) -> TmpDir {
        let dir =
            std::env::temp_dir().join(format!("gcode-metadata-test-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TmpDir(dir.to_str().unwrap().to_owned())
    }

    /// `shutil.copy(path, tmp_dir)`: the copy's path.
    fn copy(&self, path: &str) -> String {
        let to = format!(
            "{}/{}",
            self.0,
            Path::new(path).file_name().unwrap().to_str().unwrap()
        );
        std::fs::copy(path, &to).unwrap();
        to
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `tempfile.mkstemp()[1]`
fn mkstemp(test: &str) -> (TmpDir, String) {
    let dir = TmpDir::new(test);
    let path = format!("{}/tmpfile", dir.0);
    std::fs::write(&path, b"").unwrap();
    (dir, path)
}

/// Util method which reads the file in chunk and call load_from_chunk on the blocks of data
/// from given meta_class.
fn chunk_read_file(meta_class: &mut MetaData, filepath: &str, chunk_size: Option<usize>) {
    let chunk_size = chunk_size.unwrap_or(10 * 1024); // 10 KiB test on small chunks
    let mut file = std::fs::File::open(filepath).unwrap();
    // find out file size
    let file_size = file.seek(SeekFrom::End(0)).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut chunk = vec![0u8; chunk_size];
    loop {
        let mut got = 0;
        while got < chunk_size {
            let n = file.read(&mut chunk[got..]).unwrap();
            if n == 0 {
                break;
            }
            got += n;
        }
        if got == 0 {
            break;
        }
        meta_class
            .load_from_chunk(&chunk[..got], file_size)
            .unwrap();
    }
}

/// Modifies the cache file and adds a valid version number
fn give_cache_version(path: &str, version_to_give: &str) {
    let cache = py::json_loads(&std::fs::read_to_string(path).unwrap()).unwrap();
    let PyValue::Dict(cache) = cache else {
        panic!("not a dict")
    };
    let mut my_cache = Dict::new();
    my_cache.set("version", PyValue::str(version_to_give));
    my_cache.update(&cache);
    std::fs::write(path, py::json_dumps(&PyValue::Dict(my_cache), None)).unwrap();
}

/// A Python value written as JSON.
fn v(json: &str) -> PyValue {
    py::json_loads(json).unwrap()
}

fn data(meta: &MetaData, key: &str) -> PyValue {
    meta.data
        .get(key)
        .unwrap_or_else(|| panic!("{key} not in data"))
        .clone()
}

/// `pytest.raises(ValueError, match=text)`
fn value_error<T: std::fmt::Debug>(result: Result<T, MetaError>, text: &str) {
    match result {
        Err(MetaError::Py(e)) if e.is_value_error() => {
            assert!(
                e.message().contains(text),
                "{:?} has no {text:?}",
                e.message()
            )
        }
        other => panic!("not a ValueError: {other:?}"),
    }
}

fn unknown_type<T: std::fmt::Debug>(result: Result<T, MetaError>) {
    assert!(
        matches!(result, Err(MetaError::UnknownGcodeFileType(_))),
        "{result:?}"
    );
}

// --- module tests --------------------------------------------------------------------------

/// Test get_metadata() with a non-existing file
#[test]
fn test_get_metadata_file_does_not_exist() {
    let fname = "/somehwere/in/the/rainbow/my.gcode";
    let metadata = get_metadata(fname, true, None).unwrap();
    assert!(metadata.data.is_empty());
}

/// Test save-cache() with empty file
#[test]
fn test_save_cache_empty_file() {
    let Some(gcodes_dir) = gcodes_dir() else {
        return;
    };
    let fname = format!("{gcodes_dir}/fdn_all_empty.gcode");
    let fn_cache = format!("{gcodes_dir}/.fdn_all_empty.gcode");
    let meta = get_metadata(&fname, true, None).unwrap();
    meta.save_cache().unwrap();
    let err = std::fs::File::open(fn_cache).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

/// Test load_cache() with a non-existing cache file
#[test]
fn test_load_cache_file_does_not_exist() {
    let Some(gcodes_dir) = gcodes_dir() else {
        return;
    };
    let tmp_dir = TmpDir::new("load_cache_file_does_not_exist");
    let fname = format!("{gcodes_dir}/fdn_all_empty.gcode");
    let temp_gcode = tmp_dir.copy(&fname);
    value_error(MetaData::new(&temp_gcode).load_cache(), "");
}

/// test load_cache() with incorrect, or missing key
#[test]
fn test_load_cache_key_error() {
    let Some(gcodes_dir) = gcodes_dir() else {
        return;
    };
    let fname = format!("{gcodes_dir}/fdn_filename_empty.gcode");
    value_error(MetaData::new(&fname).load_cache(), "");
}

/// is_cache_recent, when cache file is fresher, than original file
#[test]
fn test_is_cache_recent_fresher() {
    let Some(gcodes_dir) = gcodes_dir() else {
        return;
    };
    let tmp_dir = TmpDir::new("is_cache_recent_fresher");
    let temp_gcode = tmp_dir.copy(&format!("{gcodes_dir}/fdn_filename.gcode"));
    // Create the time difference
    std::thread::sleep(std::time::Duration::from_millis(10));
    tmp_dir.copy(&format!("{gcodes_dir}/.fdn_filename.gcode.cache"));
    assert!(MetaData::new(&temp_gcode).is_cache_recent().unwrap());
}

/// is_cache_recent, when cache file is older, than original file
#[test]
fn test_is_cache_recent_older() {
    let Some(gcodes_dir) = gcodes_dir() else {
        return;
    };
    let tmp_dir = TmpDir::new("is_cache_recent_older");
    tmp_dir.copy(&format!("{gcodes_dir}/.fdn_filename.gcode.cache"));
    // Create the time difference
    std::thread::sleep(std::time::Duration::from_millis(10));
    let temp_gcode = tmp_dir.copy(&format!("{gcodes_dir}/fdn_filename.gcode"));
    assert!(!MetaData::new(&temp_gcode).is_cache_recent().unwrap());
}

/// is_cache_correct_version, when cache file has no version
#[test]
fn test_is_cache_correct_version_none() {
    let Some(gcodes_dir) = gcodes_dir() else {
        return;
    };
    let fn_gcode = format!("{gcodes_dir}/fdn_filename.gcode");
    assert!(!MetaData::new(&fn_gcode).is_cache_correct_version().unwrap());
}

/// is_cache_correct_version, when cache file has different version
#[test]
fn test_is_cache_correct_version_mismatch() {
    let Some(gcodes_dir) = gcodes_dir() else {
        return;
    };
    let tmp_dir = TmpDir::new("is_cache_correct_version_mismatch");
    let temp_gcode = tmp_dir.copy(&format!("{gcodes_dir}/fdn_filename.gcode"));
    let cache_path = tmp_dir.copy(&format!("{gcodes_dir}/.fdn_filename.gcode.cache"));
    give_cache_version(&cache_path, "0.0.0");
    assert!(
        !MetaData::new(&temp_gcode)
            .is_cache_correct_version()
            .unwrap()
    );
}

/// is_cache_correct_version, when cache file has the same version
#[test]
fn test_is_cache_correct_version_match() {
    let Some(gcodes_dir) = gcodes_dir() else {
        return;
    };
    let tmp_dir = TmpDir::new("is_cache_correct_version_match");
    let temp_gcode = tmp_dir.copy(&format!("{gcodes_dir}/fdn_filename.gcode"));
    let cache_path = tmp_dir.copy(&format!("{gcodes_dir}/.fdn_filename.gcode.cache"));
    give_cache_version(&cache_path, VERSION);
    assert!(
        MetaData::new(&temp_gcode)
            .is_cache_correct_version()
            .unwrap()
    );
}

/// Test get_metadata() with a file that has a wrong ending
#[test]
fn test_get_metadata_invalid_file() {
    let (_dir, fname) = mkstemp("get_metadata_invalid_file");
    unknown_type(get_metadata(&fname, true, None));
}

// --- TestFDNMetaData: tests for standard gcode metadata ------------------------------------

/// Test MMU metadata (mmu_attribute_test.gcode).
#[test]
fn fdn_test_mmu() {
    let Some(gcodes_dir) = gcodes_dir() else {
        return;
    };
    let fname = format!("{gcodes_dir}/mmu_attribute_test.gcode");
    let meta = get_metadata(&fname, false, None).unwrap();
    // check mmu data
    assert_eq!(data(&meta, "filament used [mm]"), v("6"));
    let grams = data(&meta, "filament used [g]").to_py_float().unwrap();
    assert_eq!(py::round_f64(grams, 2), 42.69);
    assert_eq!(
        data(&meta, "filament used [mm] per tool"),
        v("[1.0, 2.0, 3.0, 0.0, 0.0]")
    );
    assert_eq!(
        data(&meta, "filament used [g] per tool"),
        v("[27.0, 4.0, 5.0, 5.23, 1.46]")
    );
    assert_eq!(
        data(&meta, "filament cost per tool"),
        v("[3.0, -3.0, 0.0, 0.0, 0.0]")
    );
    assert_eq!(
        data(&meta, "bed_temperature per tool"),
        v("[90, 60, 105, 105, 105]")
    );
    assert_eq!(
        data(&meta, "temperature per tool"),
        v("[280, -280, 280, 280, 280]")
    );
    assert_eq!(
        data(&meta, "extruder_colour per tool"),
        v(r##"["#FF8000", "#DB5182", "#3EC0FF", "#FF4F4F", "#FBEB7D"]"##)
    );
    assert_eq!(
        data(&meta, "filament_abrasive per tool"),
        v("[1, 1, 1, 1, 1]")
    );
    assert_eq!(data(&meta, "filament_abrasive"), v("1"));
    assert_eq!(
        data(&meta, "nozzle_high_flow per tool"),
        v("[1, 0, 0, 1, 0]")
    );
    // This might be wrong, we might want to not allow negative values, but it's fun, so
    // whatever
    assert_eq!(data(&meta, "filament cost"), v("0"));
    // if tool specific value are not the same, the value is not added
    for key in [
        "filament used [cm3]",
        "filament_type",
        "nozzle_diameter",
        "temperature",
        "extruder_colour",
    ] {
        assert!(!meta.data.contains(key), "{key}");
    }
}

/// Both the file and filename contain metadata. There are thumbnails.
#[test]
fn fdn_test_full() {
    let Some(gcodes_dir) = gcodes_dir() else {
        return;
    };
    let fname = format!("{gcodes_dir}/fdn_full_0.15mm_PETG_MK3S_2h6m.gcode");
    let meta = get_metadata(&fname, false, None).unwrap();
    let want = v(r#"{
        "bed_temperature": 90,
        "brim_width": 0,
        "estimated printing time (normal mode)": "2h 6m 5s",
        "filament_abrasive": 1,
        "filament cost": 0.41,
        "filament used [cm3]": 10.65,
        "filament used [g]": 13.52,
        "filament used [mm]": 4427.38,
        "filament_type": "PETG",
        "fill_density": "20%",
        "nozzle_diameter": 0.4,
        "nozzle_high_flow": 0,
        "printer_model": "MK3S",
        "layer_height": 0.15,
        "support_material": 0,
        "temperature": 250,
        "ironing": 0,
        "layer_info_present": true,
        "normal_left_present": true,
        "normal_percent_present": true,
        "quiet_left_present": true,
        "quiet_percent_present": true,
        "objects_info": {
            "objects": [
                {"name": "Shape-Box id:0 copy 0",
                 "polygon": [[111.000, 111.000], [93.000, 111.000], [93.000, 93.000],
                             [111.000, 93.000]]},
                {"name": "Shape-Box id:0 copy 1",
                 "polygon": [[87.000, 111.000], [69.000, 111.000], [69.000, 93.000],
                             [87.000, 93.000]]},
                {"name": "Shape-Box id:0 copy 2",
                 "polygon": [[111.000, 87.000], [93.000, 87.000], [93.000, 69.000],
                             [111.000, 69.000]]},
                {"name": "Shape-Box id:0 copy 3",
                 "polygon": [[87.000, 87.000], [69.000, 87.000], [69.000, 69.000],
                             [87.000, 69.000]]}
            ]
        }
    }"#);
    assert_eq!(PyValue::Dict(meta.data.clone()), want);
    assert_eq!(meta.thumbnails.get("640x480_PNG").unwrap().len(), 158644);
}

/// Tests an updated file with additional suppported info
#[test]
fn fdn_test_m73_and_layer_info() {
    let Some(gcodes_dir) = gcodes_dir() else {
        return;
    };
    let fname = format!("{gcodes_dir}/full_m73_layers_0.2mm_PLA_MK3SMMU2S_11m.gcode");
    let meta = get_metadata(&fname, false, None).unwrap();
    for (key, want) in [
        ("estimated printing time (normal mode)", r#""10m 32s""#),
        ("filament_type", r#""PLA""#),
        (
            "filament_type per tool",
            r#"["PLA", "PLA", "PLA", "PLA", "PLA"]"#,
        ),
        ("printer_model", r#""MK3SMMU2S""#),
        ("layer_height", "0.2"),
        ("brim_width", "0"),
        ("fill_density", r#""15%""#),
        ("ironing", "0"),
        ("support_material", "0"),
    ] {
        assert_eq!(data(&meta, key), v(want), "{key}");
    }
    // `is True`
    for key in [
        "normal_percent_present",
        "normal_left_present",
        "quiet_percent_present",
        "quiet_left_present",
        "layer_info_present",
    ] {
        assert!(matches!(data(&meta, key), PyValue::Bool(true)), "{key}");
    }
    assert_eq!(meta.thumbnails.get("160x120_PNG").unwrap().len(), 5616);
}

/// 3.0 names the support mode, 2.x states 0/1; both read as 0/1.
#[test]
fn fdn_test_support_material_reads_as_flag() {
    for (id, value, flag) in [
        ("2.x off", "0", 0),
        ("2.x on", "1", 1),
        ("3.0 off", "none", 0),
        ("3.0 manual only", "enforcers_only", 1),
        ("3.0 on", "everywhere", 1),
    ] {
        let data_ = format!("; support_material = {value}\n").into_bytes();
        let mut meta = MetaData::fdm("support.gcode");
        meta.load_from_chunk(&data_, data_.len() as u64).unwrap();
        assert_eq!(data(&meta, "support_material"), PyValue::int(flag), "{id}");
    }
}

/// An unreadable support value is left out, not guessed.
#[test]
fn fdn_test_support_material_unknown_value_is_dropped() {
    let data_ = b"; support_material = sometimes\n";
    let mut meta = MetaData::fdm("support.gcode");
    meta.load_from_chunk(data_, data_.len() as u64).unwrap();
    assert!(!meta.data.contains("support_material"));
}

/// A real PrusaSlicer 3.0 gcode: support is read from the named value in its INI block, the
/// JSON config block adds no stray keys, and max_layer_z, which the JSON block pushes ~90 kB
/// from the end of the file, is still found.
#[test]
fn fdn_test_prusaslicer_3_gcode() {
    let Some(gcodes_dir) = gcodes_dir() else {
        return;
    };
    let fname = format!("{gcodes_dir}/ps3_support_Sphere_0.2mm_PLA_COREONE_INDX4T_18m.gcode");
    let meta = get_metadata(&fname, false, None).unwrap();
    assert_eq!(data(&meta, "support_material"), v("1"));
    assert_eq!(data(&meta, "printer_model"), v(r#""COREONE_INDX4T""#));
    assert_eq!(data(&meta, "filament used [g]"), v("3.68"));
    assert_eq!(data(&meta, "max_layer_z"), v("25.0"));
    let mut known: BTreeSet<String> = MetaData::fdm("").attrs_keys().into_iter().collect();
    known.extend(MMU_ATTRS.iter().map(|(name, _)| format!("{name} per tool")));
    let keys: BTreeSet<String> = meta.data.keys().cloned().collect();
    assert!(keys.is_subset(&known), "{:?}", keys.difference(&known));
}

const ONLY_PATH: &str = r#"{
    "estimated printing time (normal mode)": "2h9m",
    "filament_type": "PETG",
    "printer_model": "MK3S",
    "layer_height": 0.25,
    "nozzle_diameter": 0.4
}"#;

/// Only the filename contains metadata. There are no thumbnails.
#[test]
fn fdn_test_only_path() {
    let Some(gcodes_dir) = gcodes_dir() else {
        return;
    };
    let fname = format!("{gcodes_dir}/fdn_only_filename_0.4n_0.25mm_PETG_MK3S_2h9m.gcode");
    let meta = get_metadata(&fname, false, None).unwrap();
    assert_eq!(PyValue::Dict(meta.data.clone()), v(ONLY_PATH));
    assert!(meta.thumbnails.is_empty());
}

/// Only the file contains metadata. There are thumbnails.
#[test]
fn fdn_test_fdn_all_empty() {
    let Some(gcodes_dir) = gcodes_dir() else {
        return;
    };
    let fname = format!("{gcodes_dir}/fdn_all_empty.gcode");
    let meta = get_metadata(&fname, false, None).unwrap();
    assert!(meta.data.is_empty());
    assert!(meta.thumbnails.is_empty());
    assert_eq!(meta.path, fname);
}

/// Test chunks from file read ok. Test on several chunk size.
#[test]
fn fdn_test_from_chunks() {
    let Some(gcodes_dir) = gcodes_dir() else {
        return;
    };
    for chunk_size in [10 * 1024, 20 * 1024 * 1024, 2 * 1024 * 1024] {
        let fname = format!("{gcodes_dir}/fdn_full_0.15mm_PETG_MK3S_2h6m.gcode");
        let mut chunk_meta = get_meta_class(&fname, None).unwrap();
        chunk_read_file(&mut chunk_meta, &fname, Some(chunk_size));
        // check that same result came from parsing metadata as a whole file
        let meta = get_metadata(&fname, false, None).unwrap();
        assert!(meta.thumbnails == chunk_meta.thumbnails, "{chunk_size}");
        assert_eq!(meta.data, chunk_meta.data, "{chunk_size}");
    }
}

/// Test chunks fake file without metadata with string
#[test]
fn fdn_test_from_chunks_fake_data() {
    let data_ = b"*\n".repeat(1024 * 1024 * 200); // 400 MB file
    let mut chunk_meta = get_meta_class("fake_file.gcode", None).unwrap();
    let chunk_size = 10 * 1024; // 10 KiB test on small chunks
    let data_size = data_.len();
    for i in (0..data_size).step_by(chunk_size) {
        let end = (i + chunk_size).min(data_size);
        chunk_meta
            .load_from_chunk(&data_[i..end], data_size as u64)
            .unwrap();
    }
    assert!(chunk_meta.data.is_empty());
}

/// Test chunks from file with no metadata.
#[test]
fn fdn_test_from_chunks_meta_only_path() {
    let Some(gcodes_dir) = gcodes_dir() else {
        return;
    };
    let fname = format!("{gcodes_dir}/fdn_only_filename_0.4n_0.25mm_PETG_MK3S_2h9m.gcode");
    let mut chunk_meta = get_meta_class(&fname, None).unwrap();
    chunk_read_file(&mut chunk_meta, &fname, None);
    assert!(chunk_meta.data.is_empty());
    chunk_meta.load_from_path(&fname).unwrap();
    assert_eq!(PyValue::Dict(chunk_meta.data.clone()), v(ONLY_PATH));
    assert!(chunk_meta.thumbnails.is_empty());
}

/// Test chunks from file with no metadata.
#[test]
fn fdn_test_from_chunks_empty_file() {
    let Some(gcodes_dir) = gcodes_dir() else {
        return;
    };
    let fname = format!("{gcodes_dir}/fdn_all_empty.gcode");
    let mut chunk_meta = get_meta_class(&fname, None).unwrap();
    chunk_read_file(&mut chunk_meta, &fname, None);
    assert!(chunk_meta.data.is_empty());
}

/// Test reading metadata as chunks invalid file.
#[test]
fn fdn_test_from_chunks_invalid_file() {
    let (_dir, fname) = mkstemp("from_chunks_invalid_file");
    unknown_type(get_meta_class(&fname, None));
}
