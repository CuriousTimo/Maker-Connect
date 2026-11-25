//! A Marlin printer's SD card as the SDK's `SD Card` storage (#238, `docs/design/prusa-sdk-extension.md`
//! part 9, Storages).
//!
//! Two references, each on its own side. What goes to the printer is OctoPrint's only: the port's
//! comm lists the card itself and its list comes in the telemetry (`sd_files`, port K19); a print is
//! `Order::SdJob` (port K7, K20). What goes to the hub is PrusaLink's (Prusa-Link at
//! `reference/Prusa-Link`): the storage attached when the card is there (`sd_attach`,
//! `prusa_link.py:942-945`), its tree replaced by each listing (`replace_sd_tree`,
//! `sdk_augmentation/printer.py:110-130`), detached when the card is gone (`sd_detach`,
//! `prusa_link.py:951-953`), and `SEND_FILE_INFO` of a card file (`sd_file_info`, `from_path`,
//! `sdk_augmentation/printer.py:94-150`). Nothing of PrusaLink's own card scanning (`sd_card.py`)
//! is used.

use super::Agent;
use maker_connect_gcode_metadata::metadata::MetaData;
use maker_connect_protocol::SdFile;
use maker_connect_sdk::consts::{Event, StorageType};
use maker_connect_sdk::files::{File, Filesystem, Storage};
use serde_json::{Map, Value, json};
use std::sync::Arc;

/// `SD_STORAGE_NAME` (Prusa-Link `prusa/link/const.py:128`).
pub(crate) const SD_STORAGE_NAME: &str = "SD Card";

/// `get_root` (`sd_card.py:60-62`): `SDFile(name=SD_STORAGE_NAME, is_dir=True, read_only=True)`.
fn get_root() -> File {
    File::new(SD_STORAGE_NAME, true, None, read_only())
}

fn read_only() -> Map<String, Value> {
    let mut attrs = Map::new();
    attrs.insert("read_only".into(), json!(true));
    attrs
}

/// The parts of a card path, without the leading `/`.
fn parts(path: &str) -> Vec<&str> {
    path.trim_start_matches('/')
        .split('/')
        .filter(|p| !p.is_empty())
        .collect()
}

/// The tree of OctoPrint's list (design, part 9: the long path's parts, or the short path's where
/// the printer gave no long name). Ours: a long path with another number of parts than the short one
/// is taken as no long name (the short parts, so each node keeps its own `sfn`). Each node as
/// `SDFile.add_node` adds it (`sdk_augmentation/file.py`): read only, its short name as `sfn`; a file
/// with `size` and `m_timestamp` as `FileTreeParser.parse_file` passes them (`sd_card.py:149-167`),
/// here OctoPrint's.
pub(crate) fn tree(files: &[SdFile]) -> File {
    let root = get_root();
    for f in files {
        let short = parts(&f.name);
        let long = parts(&f.longname);
        let names = if long.len() == short.len() {
            &long
        } else {
            &short
        };
        let Some((file_name, dirs)) = names.split_last() else {
            continue;
        };
        let mut node = root.clone();
        for (i, dir) in dirs.iter().enumerate() {
            node = match node.child(dir) {
                Some(child) => child,
                None => {
                    let mut attrs = read_only();
                    attrs.insert("sfn".into(), json!(short[i]));
                    let Ok(child) = node.add(dir, true) else {
                        break;
                    };
                    child.set_attrs(attrs);
                    child
                }
            };
        }
        let mut attrs = read_only();
        attrs.insert("sfn".into(), json!(short[short.len() - 1]));
        if let Some(size) = f.size {
            attrs.insert("size".into(), json!(size));
        }
        if let Some(t) = f.timestamp {
            attrs.insert("m_timestamp".into(), json!(t));
        }
        if let Ok(child) = node.add(file_name, false) {
            child.set_attrs(attrs);
        }
    }
    root
}

/// `file_to_dict_legacy` (Prusa-Link `sdk_augmentation/file_tree.py:11-26`), to tell a repeated
/// listing.
fn file_to_dict_legacy(file: &File) -> Value {
    let mut result = Map::new();
    result.insert(
        "type".into(),
        json!(if file.is_dir() { "DIR" } else { "FILE" }),
    );
    result.insert("name".into(), json!(file.name()));
    let attrs = file.attrs();
    for attr in ["read_only", "m_timestamp"] {
        if let Some(v) = attrs.get(attr) {
            result.insert(attr.into(), v.clone());
        }
    }
    result.insert("size".into(), json!(file.size()));
    if file.is_dir() {
        let children: Vec<Value> = file.children().iter().map(file_to_dict_legacy).collect();
        result.insert("children".into(), Value::Array(children));
    }
    Value::Object(result)
}

/// `replace_sd_tree` (`sdk_augmentation/printer.py:110-130`). Ours: our SDK's `Storage` keeps its
/// tree for good, so `storage.tree = tree` is a new `Storage` of the same name, path, type and
/// space in its place (its `last_updated` is new; only PrusaLink's web pages read it).
fn replace_sd_tree(fs: &Filesystem, name: &str, tree: File) {
    let unchanged = {
        let mut storage_dict = fs.storage_dict();
        let Some(storage) = storage_dict.get(name).cloned() else {
            return;
        };
        if storage.tree.ptr_eq(&tree) {
            return;
        }
        let unchanged = file_to_dict_legacy(&storage.tree) == file_to_dict_legacy(&tree);
        let Ok(new) = Storage::new(
            tree,
            &storage.storage,
            storage.path_storage.as_deref(),
            storage.use_inotify,
            storage.storage_type,
            storage.system_space,
        ) else {
            return;
        };
        storage_dict.insert(name.to_owned(), Arc::new(new));
        unchanged
    };
    if unchanged {
        // a repeated listing, Connect knows it all already
        return;
    }
    let mut payload = Map::new();
    payload.insert("old_path".into(), Value::Null);
    payload.insert("new_path".into(), json!(format!("{}{name}", fs.sep)));
    payload.insert("rescan".into(), json!(true));
    fs.connect_event(Event::FileChanged, payload);
}

/// Once a second, from `marlin::tick`: the storage as the port's report has the card. Attached
/// when OctoPrint says the card is ready (`sd_attach`), its tree replaced when the list is new
/// (`sd_tree_updated`), detached when it is gone (`sd_detach`).
pub(crate) fn update(agent: &Agent, sd_card: Option<bool>, sd_files: Option<&[SdFile]>) {
    let Some(fs) = agent.fs.lock().unwrap().clone() else {
        return;
    };
    let mut attached = agent.sd_files.lock().unwrap();
    let files = sd_files.unwrap_or_default();
    match (sd_card == Some(true), attached.as_ref()) {
        (true, None) => {
            // `self.printer.fs.attach(SD_STORAGE_NAME, files, "", use_inotify=False,
            // storage_type=StorageType.SD_CARD)`
            match fs.attach(
                SD_STORAGE_NAME,
                tree(files),
                "",
                false,
                StorageType::SdCard,
                None,
            ) {
                Ok(()) => {
                    log::debug!("SD Card inserted");
                    *attached = Some(files.to_vec());
                }
                Err(err) => log::error!("SD Card: {}", err.repr()),
            }
        }
        (true, Some(last)) if last.as_slice() != files => {
            replace_sd_tree(&fs, SD_STORAGE_NAME, tree(files));
            *attached = Some(files.to_vec());
        }
        (false, Some(_)) => {
            log::debug!("SD Card removed");
            if let Err(err) = fs.detach(SD_STORAGE_NAME) {
                log::error!("SD Card: {}", err.repr());
            }
            *attached = None;
        }
        _ => {}
    }
}

/// `file_is_on_sd` (Prusa-Link `util.py:146-150`): the path's first part is the card's storage.
pub(crate) fn file_is_on_sd(path: &str) -> bool {
    parts(path).first() == Some(&SD_STORAGE_NAME)
}

/// `sd_file_info` (`sdk_augmentation/printer.py:94-108`): a folder's info as the SDK gives it, a
/// file's with the metadata of its name (`from_path`, `:133-150`).
pub(crate) fn sd_file_info(
    fs: &Filesystem,
    path: &str,
) -> Result<Map<String, Value>, maker_connect_sdk::fileinfo::Error> {
    let info = fs.file_info(path)?;
    if info.contains_key("children") {
        return Ok(info);
    }
    let mut meta = MetaData::fdm(path);
    let _ = meta.load_from_path(path);
    let mut data = Map::new();
    data.insert("path".into(), json!(path));
    for (k, v) in info {
        data.insert(k, v);
    }
    for (k, v) in meta.data.iter() {
        data.insert(k.clone(), maker_connect_sdk::fileinfo::py_to_json(v));
    }
    Ok(data)
}

/// OctoPrint's name of a card file for `Order::SdJob`: the short names of its path in the tree,
/// joined, as the printer's `get_sd_files` names it (`standard.py:942`, `x[0][1:]`: comm's name
/// without its leading `/`), which the files API selects it by. `None` when the path is not a file
/// of the card.
pub(crate) fn octoprint_name(fs: &Filesystem, path: &str) -> Option<(String, File)> {
    let p = parts(path);
    let (storage, rest) = p.split_first()?;
    if *storage != SD_STORAGE_NAME || rest.is_empty() {
        return None;
    }
    let tree = fs.storage_dict().get(SD_STORAGE_NAME)?.tree.clone();
    let mut node = tree;
    let mut short = Vec::new();
    for name in rest {
        node = node.child(name)?;
        short.push(node.attrs().get("sfn")?.as_str()?.to_owned());
    }
    if node.is_dir() {
        return None;
    }
    Some((short.join("/"), node))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sd(name: &str, size: Option<i64>, longname: &str, timestamp: Option<i64>) -> SdFile {
        SdFile {
            name: name.into(),
            size,
            longname: longname.into(),
            timestamp,
        }
    }

    /// Marlin's `M20 L T` with `LONG_FILENAME_HOST_SUPPORT` (`cardreader.cpp:300-365`), as comm
    /// keeps it: the long path's parts are the tree, the short names kept as `sfn`.
    #[test]
    fn the_tree_of_long_names() {
        let root = tree(&[
            sd(
                "/PRINTS~1/BENCHY~1.GCO",
                Some(1234),
                "Prints/Benchy.gcode",
                Some(1_700_000_000),
            ),
            sd("/CUBE.GCO", Some(10), "/CUBE.GCO", None),
        ]);
        assert_eq!(root.name(), "SD Card");
        assert_eq!(root.attrs()["read_only"], true);
        let dir = root.child("Prints").expect("the long folder name");
        assert!(dir.is_dir());
        assert_eq!(dir.attrs()["sfn"], "PRINTS~1");
        assert_eq!(dir.attrs()["read_only"], true);
        let file = dir.child("Benchy.gcode").expect("the long file name");
        assert_eq!(file.attrs()["sfn"], "BENCHY~1.GCO");
        assert_eq!(file.size(), 1234);
        assert_eq!(file.attrs()["m_timestamp"], 1_700_000_000);
        assert_eq!(file.attrs()["read_only"], true);
        let cube = root.child("CUBE.GCO").expect("no long name: the short one");
        assert!(cube.attrs().get("m_timestamp").is_none());
    }

    #[test]
    fn a_print_by_octoprints_name() {
        let fs = Filesystem::new("/", None);
        let files = [sd(
            "/PRINTS~1/BENCHY~1.GCO",
            Some(1234),
            "Prints/Benchy.gcode",
            None,
        )];
        fs.attach(
            SD_STORAGE_NAME,
            tree(&files),
            "",
            false,
            StorageType::SdCard,
            None,
        )
        .unwrap();
        let (name, node) = octoprint_name(&fs, "/SD Card/Prints/Benchy.gcode").unwrap();
        assert_eq!(name, "PRINTS~1/BENCHY~1.GCO");
        assert_eq!(node.size(), 1234);
        assert!(octoprint_name(&fs, "/SD Card/Prints").is_none(), "a folder");
        assert!(octoprint_name(&fs, "/SD Card/none.gcode").is_none());
        assert!(octoprint_name(&fs, "/gcodes/a.gcode").is_none());
        assert!(file_is_on_sd("/SD Card/Prints/Benchy.gcode"));
        assert!(!file_is_on_sd("/gcodes/a.gcode"));
    }

    /// `replace_sd_tree`: `FILE_CHANGED` with `rescan` only when the tree changed.
    #[test]
    fn a_new_listing_replaces_the_tree() {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = events.clone();
        let cb: maker_connect_sdk::models::EventCallback =
            Arc::new(move |event, _source, _a, _b, kwargs| {
                seen.lock().unwrap().push((event, kwargs));
            });
        let fs = Filesystem::new("/", Some(cb));
        let one = [sd("/A.GCO", Some(1), "/A.GCO", None)];
        fs.attach(
            SD_STORAGE_NAME,
            tree(&one),
            "",
            false,
            StorageType::SdCard,
            None,
        )
        .unwrap();
        events.lock().unwrap().clear();
        replace_sd_tree(&fs, SD_STORAGE_NAME, tree(&one));
        assert!(events.lock().unwrap().is_empty(), "a repeated listing");
        let two = [sd("/B.GCO", Some(2), "/B.GCO", None)];
        replace_sd_tree(&fs, SD_STORAGE_NAME, tree(&two));
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, Event::FileChanged);
        assert_eq!(
            Value::Object(events[0].1.clone()),
            json!({"old_path": null, "new_path": "/SD Card", "rescan": true})
        );
        assert!(fs.get("/SD Card/B.GCO").is_some());
        assert!(fs.get("/SD Card/A.GCO").is_none());
    }
}
