//! CPython 3.13 `Lib/fnmatch.py` and `Lib/glob.py` (`reference/cpython-3.13`), the parts
//! `comm.py`'s `serialList` uses: `fnmatch.fnmatch(name, pattern)` (with `os.path.normcase`, a no-op
//! on POSIX) and `glob.glob(pathname)` with its defaults (`root_dir=None`, `dir_fd=None`,
//! `recursive=False`, `include_hidden=False`), on a POSIX path. Plus `posixpath.split`/`join`
//! (`posixpath.py:72`, `:100`) which `glob` uses. Python Software Foundation licence.
//!
//! Python's `re` is `fancy_regex` here (owner's approval M5); `\Z` of Python is `\z`.

use fancy_regex::Regex;

/// `re.escape(c)` for one character (CPython 3.13 `re/__init__.py`: `_special_chars_map`)
fn re_escape(c: char) -> String {
    if "()[]{}?*+-|^$\\.&~# \t\n\r\x0b\x0c".contains(c) {
        format!("\\{c}")
    } else {
        c.to_string()
    }
}

/// `fnmatch.py:75 translate(pat)`: a shell pattern as a regular expression.
pub fn translate(pat: &str) -> String {
    // `STAR` is a marker object in Python; here `None`
    let parts = _translate(pat);
    _join_translated_parts(&parts)
}

fn _translate(pat: &str) -> Vec<Option<String>> {
    let pat: Vec<char> = pat.chars().collect();
    let mut res: Vec<Option<String>> = Vec::new();
    let (mut i, n) = (0usize, pat.len());
    while i < n {
        let c = pat[i];
        i += 1;
        if c == '*' {
            // compress consecutive `*` into one
            if res.last().is_none_or(|l| l.is_some()) {
                res.push(None);
            }
        } else if c == '?' {
            res.push(Some(".".to_string()));
        } else if c == '[' {
            let mut j = i;
            if j < n && pat[j] == '!' {
                j += 1;
            }
            if j < n && pat[j] == ']' {
                j += 1;
            }
            while j < n && pat[j] != ']' {
                j += 1;
            }
            if j >= n {
                res.push(Some("\\[".to_string()));
            } else {
                let mut stuff: String;
                let slice = |a: usize, b: usize| -> String { pat[a..b].iter().collect() };
                if !pat[i..j].contains(&'-') {
                    stuff = slice(i, j).replace('\\', "\\\\");
                } else {
                    let mut chunks: Vec<String> = Vec::new();
                    let mut k = if pat[i] == '!' { i + 2 } else { i + 1 };
                    loop {
                        // k = pat.find('-', k, j)
                        let found = (k..j).find(|x| pat[*x] == '-');
                        let Some(kk) = found else { break };
                        k = kk;
                        chunks.push(slice(i, k));
                        i = k + 1;
                        k += 3;
                    }
                    let chunk = slice(i, j);
                    if !chunk.is_empty() {
                        chunks.push(chunk);
                    } else {
                        chunks
                            .last_mut()
                            .expect("a chunk before the last hyphen")
                            .push('-');
                    }
                    // Remove empty ranges -- invalid in RE.
                    let mut k = chunks.len().saturating_sub(1);
                    while k >= 1 {
                        let prev_last = chunks[k - 1].chars().last().expect("non-empty chunk");
                        let next_first = chunks[k].chars().next().expect("non-empty chunk");
                        if prev_last > next_first {
                            let mut p: Vec<char> = chunks[k - 1].chars().collect();
                            p.pop();
                            let merged: String =
                                p.into_iter().chain(chunks[k].chars().skip(1)).collect();
                            chunks[k - 1] = merged;
                            chunks.remove(k);
                        }
                        k -= 1;
                    }
                    // Escape backslashes and hyphens for set difference (--).
                    // Hyphens that create ranges shouldn't be escaped.
                    stuff = chunks
                        .iter()
                        .map(|s| s.replace('\\', "\\\\").replace('-', "\\-"))
                        .collect::<Vec<_>>()
                        .join("-");
                }
                // Escape set operations (&&, ~~ and ||).
                stuff = stuff
                    .chars()
                    .map(|c| {
                        if "&~|".contains(c) {
                            format!("\\{c}")
                        } else {
                            c.to_string()
                        }
                    })
                    .collect();
                i = j + 1;
                if stuff.is_empty() {
                    // Empty range: never match.
                    res.push(Some("(?!)".to_string()));
                } else if stuff == "!" {
                    // Negated empty range: match any character.
                    res.push(Some(".".to_string()));
                } else {
                    if stuff.starts_with('!') {
                        stuff = format!("^{}", &stuff[1..]);
                    } else if stuff.starts_with('^') || stuff.starts_with('[') {
                        stuff = format!("\\{stuff}");
                    }
                    res.push(Some(format!("[{stuff}]")));
                }
            }
        } else {
            res.push(Some(re_escape(c)));
        }
    }
    assert_eq!(i, n);
    res
}

fn _join_translated_parts(inp: &[Option<String>]) -> String {
    let mut res = String::new();
    let (mut i, n) = (0usize, inp.len());
    // Fixed pieces at the start?
    while i < n && inp[i].is_some() {
        res.push_str(inp[i].as_ref().expect("checked"));
        i += 1;
    }
    // Now deal with STAR fixed STAR fixed ...
    while i < n {
        assert!(inp[i].is_none());
        i += 1;
        if i == n {
            res.push_str(".*");
            break;
        }
        assert!(inp[i].is_some());
        let mut fixed = String::new();
        while i < n && inp[i].is_some() {
            fixed.push_str(inp[i].as_ref().expect("checked"));
            i += 1;
        }
        if i == n {
            res.push_str(".*");
            res.push_str(&fixed);
        } else {
            res.push_str(&format!("(?>.*?{fixed})"));
        }
    }
    assert_eq!(i, n);
    // Python's `\Z` (end of the string) is `\z`
    format!("(?s:{res})\\z")
}

/// `fnmatch.py:50 filter(names, pat)` on POSIX (`os.path.normcase` is a no-op there)
pub fn filter(names: &[String], pat: &str) -> Vec<String> {
    let re = Regex::new(&translate(pat)).unwrap_or_else(|e| panic!("re.error: {e}"));
    names
        .iter()
        .filter(|name| {
            re.find(name)
                .expect("regex runtime error")
                .is_some_and(|m| m.start() == 0)
        })
        .cloned()
        .collect()
}

/// `fnmatch.py:23 fnmatch(name, pat)` on POSIX
pub fn fnmatch(name: &str, pat: &str) -> bool {
    !filter(&[name.to_string()], pat).is_empty()
}

/// `posixpath.py:100 split(p)`
pub fn path_split(p: &str) -> (String, String) {
    let i = p.rfind('/').map_or(0, |i| i + 1);
    let (head, tail) = (&p[..i], &p[i..]);
    let head = if !head.is_empty() && head.chars().any(|c| c != '/') {
        head.trim_end_matches('/')
    } else {
        head
    };
    (head.to_string(), tail.to_string())
}

/// `posixpath.py:72 join(a, b)` (two parts)
pub fn path_join(a: &str, b: &str) -> String {
    if b.starts_with('/') || a.is_empty() {
        b.to_string()
    } else if a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// `glob.py:259 has_magic(s)`
fn has_magic(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

/// `glob.py:267 _ishidden(path)`
fn _ishidden(path: &str) -> bool {
    path.starts_with('.')
}

/// `glob.py:259 _join(dirname, basename)`
fn _join(dirname: &str, basename: &str) -> String {
    // It is common if dirname or basename is empty
    if dirname.is_empty() || basename.is_empty() {
        return if dirname.is_empty() {
            basename.to_string()
        } else {
            dirname.to_string()
        };
    }
    path_join(dirname, basename)
}

/// `genericpath.py:26 lexists`
fn _lexists(pathname: &str) -> bool {
    std::fs::symlink_metadata(pathname).is_ok()
}

/// `genericpath.py:48 isdir`
fn _isdir(pathname: &str) -> bool {
    std::fs::metadata(pathname).is_ok_and(|m| m.is_dir())
}

/// `glob.py:154 _iterdir` + `_listdir` (`dir_fd=None`): the names in a directory; an `OSError` is an
/// empty list.
fn _listdir(dirname: &str, dironly: bool) -> Vec<String> {
    let arg = if dirname.is_empty() { "." } else { dirname };
    let Ok(it) = std::fs::read_dir(arg) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in it {
        let Ok(entry) = entry else {
            // an error while iterating ends the iteration (`except OSError: return`)
            break;
        };
        if !dironly
            || entry.file_type().is_ok_and(|t| {
                t.is_dir() || (t.is_symlink() && _isdir(&entry.path().to_string_lossy()))
            })
        {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names
}

/// `glob.py:121 _glob1(dirname, pattern, dir_fd, dironly, include_hidden=False)`
fn _glob1(dirname: &str, pattern: &str, dironly: bool) -> Vec<String> {
    let mut names = _listdir(dirname, dironly);
    if !_ishidden(pattern) {
        names.retain(|x| !_ishidden(x));
    }
    filter(&names, pattern)
}

/// `glob.py:127 _glob0(dirname, basename, dir_fd, dironly, include_hidden=False)`
fn _glob0(dirname: &str, basename: &str) -> Vec<String> {
    if !basename.is_empty() {
        if _lexists(&_join(dirname, basename)) {
            return vec![basename.to_string()];
        }
    } else if _isdir(dirname) {
        // `os.path.split()` returns an empty basename for paths ending with a directory separator.
        return vec![basename.to_string()];
    }
    Vec::new()
}

/// `glob.py:76 _iglob(pathname, root_dir, dir_fd, recursive, dironly, include_hidden=False)` with
/// `root_dir=""`, `dir_fd=None`, `recursive=False`.
fn _iglob(pathname: &str, dironly: bool) -> Vec<String> {
    let (dirname, basename) = path_split(pathname);
    if !has_magic(pathname) {
        assert!(!dironly);
        if !basename.is_empty() {
            if _lexists(pathname) {
                return vec![pathname.to_string()];
            }
        } else if _isdir(&dirname) {
            // Patterns ending with a slash should match only directories
            return vec![pathname.to_string()];
        }
        return Vec::new();
    }
    if dirname.is_empty() {
        return _glob1("", &basename, dironly);
    }
    // `os.path.split()` returns the argument itself as a dirname if it is a drive or UNC path.
    let dirs: Vec<String> = if dirname != pathname && has_magic(&dirname) {
        _iglob(&dirname, true)
    } else {
        vec![dirname.clone()]
    };
    let mut out = Vec::new();
    for dirname in dirs {
        let names = if has_magic(&basename) {
            _glob1(&_join("", &dirname), &basename, dironly)
        } else {
            _glob0(&_join("", &dirname), &basename)
        };
        for name in names {
            out.push(path_join(&dirname, &name));
        }
    }
    out
}

/// `glob.py:20 glob(pathname)` with the defaults: "The order of the returned list is undefined."
pub fn glob(pathname: &str) -> Vec<String> {
    _iglob(pathname, false)
}

#[cfg(test)]
mod tests {
    //! OURS, from CPython 3.13 `Lib/test/test_fnmatch.py` (`test_fnmatch`, `test_fnmatchcase`) and
    //! `test_glob.py` (the cases that need no recursion), on a temporary directory.
    use super::*;

    #[test]
    fn test_fnmatch() {
        // Lib/test/test_fnmatch.py FnmatchTestCase.test_fnmatch
        for (name, pat, expect) in [
            ("abc", "abc", true),
            ("abc", "?*?", true),
            ("abc", "???*", true),
            ("abc", "*???", true),
            ("abc", "???", true),
            ("abc", "*", true),
            ("abc", "ab[cd]", true),
            ("abc", "ab[!de]", true),
            ("abc", "ab[de]", false),
            ("a", "??", false),
            ("a", "b", false),
            // these test that '\' is handled correctly in character sets;
            ("\\", "[\\]", true),
            ("a", "[!\\]", true),
            ("\\", "[!\\]", false),
            // test that filenames with newlines in them are handled correctly.
            ("foo\nbar", "foo*", true),
            ("foo\nbar\n", "foo*", true),
            ("\nfoo", "foo*", false),
            ("\n", "*", true),
            // slow path / interior stars
            ("abc", "a*c", true),
            ("abbbc", "a*b*c", true),
            ("abc", "a*d*c", false),
            ("/dev/ttyUSB0", "/dev/ttyUSB*", true),
            ("/dev/ttyACM0", "/dev/ttyUSB*", false),
            ("a-c", "a[-]c", true),
            ("b", "[a-c]", true),
            ("d", "[a-c]", false),
            ("a", "[]", false),
        ] {
            assert_eq!(expect, fnmatch(name, pat), "{name:?} {pat:?}");
        }
    }

    #[test]
    fn test_translate() {
        // Lib/test/test_fnmatch.py TranslateTestCase.test_translate
        assert_eq!("(?s:a.*b)\\z", translate("a*b"));
        assert_eq!("(?s:.*a)\\z", translate("*a"));
        assert_eq!("(?s:a.*)\\z", translate("a*"));
        assert_eq!("(?s:a(?>.*?b).*c)\\z", translate("a*b*c"));
        assert_eq!("(?s:a.c)\\z", translate("a?c"));
        assert_eq!("(?s:\\[)\\z", translate("["));
        assert_eq!("(?s:[a-z])\\z", translate("[a-z]"));
        assert_eq!("(?s:[^a-z])\\z", translate("[!a-z]"));
        assert_eq!("(?s:\\.txt)\\z", translate(".txt"));
    }

    #[test]
    fn test_path_split_join() {
        // posixpath.py behaviour (Lib/test/test_posixpath.py test_split, test_join)
        assert_eq!(
            ("/foo".to_string(), "bar".to_string()),
            path_split("/foo/bar")
        );
        assert_eq!(("/".to_string(), "foo".to_string()), path_split("/foo"));
        assert_eq!(
            ("/foo/bar".to_string(), "".to_string()),
            path_split("/foo/bar/")
        );
        assert_eq!(("//".to_string(), "foo".to_string()), path_split("//foo"));
        assert_eq!(("".to_string(), "foo".to_string()), path_split("foo"));
        assert_eq!("/bar/baz", path_join("/foo", "/bar/baz"));
        assert_eq!("/foo/bar/baz", path_join("/foo/bar", "baz"));
        assert_eq!("/foo/bar/baz", path_join("/foo/bar/", "baz"));
        assert_eq!("baz", path_join("", "baz"));
    }

    #[test]
    fn test_glob() {
        let dir = std::env::temp_dir().join(format!("maker-connect-glob-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for f in [
            "ttyUSB0", "ttyUSB1", "ttyACM0", ".hidden", "sub/x1", "sub/x2",
        ] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let d = dir.to_str().unwrap();
        let sorted = |mut v: Vec<String>| {
            v.sort();
            v
        };
        let g = |p: &str| sorted(glob(&format!("{d}/{p}")));
        let full = |names: &[&str]| names.iter().map(|n| format!("{d}/{n}")).collect::<Vec<_>>();
        assert_eq!(full(&["ttyUSB0", "ttyUSB1"]), g("ttyUSB*"));
        assert_eq!(full(&["ttyACM0", "ttyUSB0", "ttyUSB1"]), g("tty*"));
        assert_eq!(full(&["ttyACM0"]), g("ttyACM0"));
        assert!(g("nothere").is_empty());
        assert!(g("*hidden").is_empty());
        assert_eq!(full(&[".hidden"]), g(".h*"));
        assert_eq!(full(&["sub/x1", "sub/x2"]), g("s*/x*"));
        assert_eq!(full(&["sub/"]), g("s*/"));
        assert_eq!(full(&["sub/"]), g("sub/"));
        assert_eq!(full(&["sub/x2"]), g("sub/x[2]"));
        assert!(glob("").is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
