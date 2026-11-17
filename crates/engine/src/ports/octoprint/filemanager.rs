//! Port of the parts of OctoPrint 1.11.8 `src/octoprint/filemanager/__init__.py` and
//! `filemanager/destinations.py` the comm layer uses: `valid_file_type` (and the extension tree
//! functions it calls) and `FileDestinations`.
//!
//! Original: Copyright (C) 2014 The OctoPrint Project, AGPL-3.0.
//!
//! The extension tree comes from the default tree plus slicer plugins (`SlicerPlugin`
//! implementations) and the `octoprint.filemanager.extension_tree` hooks. We have no plugins
//! (plugin.rs), so both add nothing; a registered hook would reach `unimplemented!`.
//! OURS: `ContentTypeDetector`'s `detector` callable is not kept (only `get_mime_type` calls it,
//! which is not ported).

use super::files::splitext;
use super::plugin::plugin_manager;

/// `filemanager/destinations.py:5 FileDestinations`
pub struct FileDestinations;

impl FileDestinations {
    pub const SDCARD: &'static str = "sdcard";
    pub const LOCAL: &'static str = "local";
}

/// `filemanager/__init__.py:21 ContentTypeMapping` / `ContentTypeDetector` (a `namedtuple`),
/// a list of extensions, or a nested dict: the values of an extension tree.
#[derive(Debug, Clone, PartialEq)]
pub enum ExtensionTreeValue {
    /// `ContentTypeMapping(extensions, content_type)`
    ContentTypeMapping {
        extensions: Vec<String>,
        content_type: Option<String>,
    },
    /// `ContentTypeDetector(extensions, detector)`, see the module doc
    ContentTypeDetector { extensions: Vec<String> },
    /// a `list`
    List(Vec<String>),
    /// a `dict`
    Dict(ExtensionTree),
}

/// An extension tree: a `dict` (insertion-ordered)
pub type ExtensionTree = Vec<(String, ExtensionTreeValue)>;

/// `filemanager/__init__.py:30 full_extension_tree()`
pub fn full_extension_tree() -> ExtensionTree {
    let result: ExtensionTree = vec![(
        "machinecode".to_string(),
        ExtensionTreeValue::Dict(vec![(
            "gcode".to_string(),
            ExtensionTreeValue::ContentTypeMapping {
                extensions: ["gcode", "gco", "g", "gc~"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                content_type: Some("text/plain".to_string()),
            },
        )]),
    )];

    // slicer_plugins = plugin_manager().get_implementations(SlicerPlugin): none, no plugins

    let extension_tree_hooks = plugin_manager().get_hooks("octoprint.filemanager.extension_tree");
    if !extension_tree_hooks.is_empty() {
        unimplemented!(
            "filemanager/__init__.py:97-110, merging the result of an extension tree hook"
        );
    }

    result
}

/// `filemanager/__init__.py:119 get_extensions(type, subtree=None)`
pub fn get_extensions(r#type: &str, subtree: Option<&ExtensionTree>) -> Option<Vec<String>> {
    let full;
    let subtree = match subtree {
        Some(s) => s,
        None => {
            full = full_extension_tree();
            &full
        }
    };

    for (key, value) in subtree {
        if key == r#type {
            return Some(get_all_extensions(Some(value)));
        } else if let ExtensionTreeValue::Dict(d) = value {
            if let Some(sub_extensions) = get_extensions(r#type, Some(d)) {
                if !sub_extensions.is_empty() {
                    return Some(sub_extensions);
                }
            }
        }
    }

    None
}

/// `filemanager/__init__.py:132 get_all_extensions(subtree=None)`: `subtree` is a dict, a
/// `ContentTypeMapping`/`ContentTypeDetector` or a list; `None` is the full tree.
pub fn get_all_extensions(subtree: Option<&ExtensionTreeValue>) -> Vec<String> {
    let full;
    let subtree = match subtree {
        Some(s) => s,
        None => {
            full = ExtensionTreeValue::Dict(full_extension_tree());
            &full
        }
    };

    let mut result = Vec::new();
    match subtree {
        ExtensionTreeValue::Dict(d) => {
            for (_, value) in d {
                match value {
                    ExtensionTreeValue::Dict(_) => result.extend(get_all_extensions(Some(value))),
                    ExtensionTreeValue::ContentTypeMapping { extensions, .. }
                    | ExtensionTreeValue::ContentTypeDetector { extensions } => {
                        result.extend(extensions.clone())
                    }
                    ExtensionTreeValue::List(l) => result.extend(l.clone()),
                }
            }
        }
        ExtensionTreeValue::ContentTypeMapping { extensions, .. }
        | ExtensionTreeValue::ContentTypeDetector { extensions } => result = extensions.clone(),
        ExtensionTreeValue::List(l) => result = l.clone(),
    }
    result
}

/// `filemanager/__init__.py:180 valid_extension(extension, type=None, tree=None)`. `None` is
/// Python's `None` (a `type` without extensions).
pub fn valid_extension(
    extension: &str,
    r#type: Option<&str>,
    tree: Option<&ExtensionTree>,
) -> Option<bool> {
    match r#type {
        None | Some("") => {
            let all = match tree {
                Some(t) => get_all_extensions(Some(&ExtensionTreeValue::Dict(t.clone()))),
                None => get_all_extensions(None),
            };
            Some(all.iter().any(|e| e == extension))
        }
        Some(t) => {
            let extensions = get_extensions(t, tree);
            match extensions {
                Some(e) if !e.is_empty() => Some(e.iter().any(|x| x == extension)),
                _ => None,
            }
        }
    }
}

/// `filemanager/__init__.py:194 valid_file_type(filename, type=None, tree=None)`
pub fn valid_file_type(
    filename: &str,
    r#type: Option<&str>,
    tree: Option<&ExtensionTree>,
) -> Option<bool> {
    let (_, extension) = splitext(filename);
    let extension = extension.get(1..).unwrap_or("").to_lowercase();
    valid_extension(&extension, r#type, tree)
}

#[cfg(test)]
mod tests {
    //! Ports of `tests/filemanager/test_filemanager.py` `test_valid_file_type*` and
    //! `test_full_extension_tree`, without the plugin hooks the original mocks in (we have no
    //! plugins): the cases that depend on a hook are not ported; the others are ported with the
    //! default tree.
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn test_full_extension_tree_default() {
        let full = full_extension_tree();
        assert_eq!(1, full.len());
        let ExtensionTreeValue::Dict(machinecode) = &full[0].1 else {
            panic!()
        };
        let ExtensionTreeValue::ContentTypeMapping {
            extensions,
            content_type,
        } = &machinecode[0].1
        else {
            panic!()
        };
        assert_eq!(s(&["gcode", "gco", "g", "gc~"]), *extensions);
        assert_eq!(Some("text/plain".to_string()), *content_type);
    }

    #[test]
    fn test_valid_file_type() {
        assert_eq!(Some(true), valid_file_type("foo.gcode", None, None));
        assert_eq!(Some(false), valid_file_type("foo.unknown", None, None));
        assert_eq!(Some(true), valid_file_type("FOO.GCO", None, None));
    }

    #[test]
    fn test_valid_file_type_with_type() {
        assert_eq!(
            Some(false),
            valid_file_type("foo.stl", Some("machinecode"), None)
        );
        assert_eq!(
            Some(true),
            valid_file_type("foo.gcode", Some("machinecode"), None)
        );
        assert_eq!(
            Some(true),
            valid_file_type("foo.gcode", Some("gcode"), None)
        );
        assert_eq!(None, valid_file_type("foo.gcode", Some("nothing"), None));
    }

    #[test]
    fn test_valid_file_type_with_extension_tree() {
        let tree: ExtensionTree = vec![(
            "machinecode".to_string(),
            ExtensionTreeValue::Dict(vec![
                (
                    "gcode".to_string(),
                    ExtensionTreeValue::ContentTypeMapping {
                        extensions: s(&["gcode", "gco", "g"]),
                        content_type: Some("text/plain".to_string()),
                    },
                ),
                (
                    "foo".to_string(),
                    ExtensionTreeValue::List(s(&["foo", "f"])),
                ),
            ]),
        )];
        assert_eq!(Some(true), valid_file_type("foo.foo", None, Some(&tree)));
        assert_eq!(Some(false), valid_file_type("foo.amf", None, Some(&tree)));
        assert_eq!(
            Some(true),
            valid_file_type("foo.foo", Some("machinecode"), Some(&tree))
        );
        assert_eq!(
            Some(true),
            valid_file_type("foo.foo", Some("foo"), Some(&tree))
        );
    }
}
