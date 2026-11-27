//! `sortedcontainers.SortedKeyList` 2.4.0 (`sortedcontainers/sortedlist.py`, Apache-2.0, Copyright
//! Grant Jenks), the part PrusaLink uses: `SerialParser.pattern_list`
//! (`serial/serial_parser.py:68`, `:112-138`; `add`, `remove`, `in`, iteration). A ported second
//! source, as the reference uses it (owner, 2026-11-16; `docs/archive/plans/mk3-port-plan.md` §5).
//!
//! The structure is the library's own: sublists of at most twice the load factor (1000), each with
//! its keys and its maximum, split and joined as it does. That decides the order of equal keys,
//! which a caller sees: a value goes after the values of an equal key (`bisect_right`), and
//! `remove` takes the first equal value among those of its key.
//!
//! **Not covered:** the positional index (`_index`, `_offset`): it is built only by positional
//! access (`skl[i]`, `_loc`, `_pos`), which PrusaLink does not use, so it stays empty and the
//! updates of it in `_expand` and `_delete` are kept as the source has them.

use std::fmt::Debug;

/// `bisect.bisect_right(a, x)`
fn bisect_right<K: PartialOrd>(a: &[K], x: &K) -> usize {
    let (mut lo, mut hi) = (0, a.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if x < &a[mid] {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

/// `bisect.bisect_left(a, x)`
fn bisect_left<K: PartialOrd>(a: &[K], x: &K) -> usize {
    let (mut lo, mut hi) = (0, a.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if &a[mid] < x {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

/// `remove`'s `ValueError('{0!r} not in list'.format(value))`, with the value's `repr`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotInList(pub String);

/// `SortedKeyList` (`sortedlist.py:1691`)
pub struct SortedKeyList<T, K> {
    _key: fn(&T) -> K,
    _len: usize,
    _load: usize,
    _lists: Vec<Vec<T>>,
    _keys: Vec<Vec<K>>,
    _maxes: Vec<K>,
    _index: Vec<usize>,
    _offset: usize,
}

impl<T: PartialEq + Debug, K: PartialOrd + Clone> SortedKeyList<T, K> {
    /// `SortedList.DEFAULT_LOAD_FACTOR` (`:142`)
    pub const DEFAULT_LOAD_FACTOR: usize = 1000;

    /// `__init__(iterable=None, key=key)` (`:1716-1751`)
    pub fn new(key: fn(&T) -> K) -> SortedKeyList<T, K> {
        SortedKeyList {
            _key: key,
            _len: 0,
            _load: Self::DEFAULT_LOAD_FACTOR,
            _lists: vec![],
            _keys: vec![],
            _maxes: vec![],
            _index: vec![],
            _offset: 0,
        }
    }

    /// `_reset(load)` (`:214-236`) of an empty list: the load factor (the test's small sublists).
    pub fn _reset(&mut self, load: usize) {
        assert_eq!(self._len, 0, "only of an empty list here");
        self._load = load;
    }

    /// `len(skl)`
    pub fn len(&self) -> usize {
        self._len
    }

    pub fn is_empty(&self) -> bool {
        self._len == 0
    }

    /// `add(value)` (`:1778-1820`): Add `value` to sorted-key list.
    pub fn add(&mut self, value: T) {
        let key = (self._key)(&value);

        if !self._maxes.is_empty() {
            let mut pos = bisect_right(&self._maxes, &key);

            if pos == self._maxes.len() {
                pos -= 1;
                self._lists[pos].push(value);
                self._keys[pos].push(key.clone());
                self._maxes[pos] = key;
            } else {
                let idx = bisect_right(&self._keys[pos], &key);
                self._lists[pos].insert(idx, value);
                self._keys[pos].insert(idx, key);
            }

            self._expand(pos);
        } else {
            self._lists.push(vec![value]);
            self._keys.push(vec![key.clone()]);
            self._maxes.push(key);
        }

        self._len += 1;
    }

    /// `_expand(pos)` (`:1822-1859`): Split sublists with length greater than double the
    /// load-factor.
    fn _expand(&mut self, pos: usize) {
        if self._keys[pos].len() > (self._load << 1) {
            let _load = self._load;

            let half: Vec<T> = self._lists[pos].split_off(_load);
            let half_keys: Vec<K> = self._keys[pos].split_off(_load);
            self._maxes[pos] = self._keys[pos].last().unwrap().clone();

            let half_max = half_keys.last().unwrap().clone();
            self._lists.insert(pos + 1, half);
            self._keys.insert(pos + 1, half_keys);
            self._maxes.insert(pos + 1, half_max);

            self._index.clear();
        } else if !self._index.is_empty() {
            let mut child = self._offset + pos;
            while child != 0 {
                self._index[child] += 1;
                child = (child - 1) >> 1;
            }
            self._index[0] += 1;
        }
    }

    /// `__contains__(value)` (`:1903-1950`): Return true if `value` is an element of the
    /// sorted-key list.
    pub fn contains(&self, value: &T) -> bool {
        if self._maxes.is_empty() {
            return false;
        }

        let key = (self._key)(value);
        let mut pos = bisect_left(&self._maxes, &key);

        if pos == self._maxes.len() {
            return false;
        }

        let mut idx = bisect_left(&self._keys[pos], &key);

        let len_keys = self._keys.len();
        let mut len_sublist = self._keys[pos].len();

        loop {
            if self._keys[pos][idx] != key {
                return false;
            }
            if self._lists[pos][idx] == *value {
                return true;
            }
            idx += 1;
            if idx == len_sublist {
                pos += 1;
                if pos == len_keys {
                    return false;
                }
                len_sublist = self._keys[pos].len();
                idx = 0;
            }
        }
    }

    /// `remove(value)` (`:2001-2052`): Remove `value` from sorted-key list; `value` must be a
    /// member. If `value` is not a member, raise ValueError.
    pub fn remove(&mut self, value: &T) -> Result<(), NotInList> {
        let not_in_list = || NotInList(format!("{value:?} not in list"));
        if self._maxes.is_empty() {
            return Err(not_in_list());
        }

        let key = (self._key)(value);
        let mut pos = bisect_left(&self._maxes, &key);

        if pos == self._maxes.len() {
            return Err(not_in_list());
        }

        let mut idx = bisect_left(&self._keys[pos], &key);
        let len_keys = self._keys.len();
        let mut len_sublist = self._keys[pos].len();

        loop {
            if self._keys[pos][idx] != key {
                return Err(not_in_list());
            }
            if self._lists[pos][idx] == *value {
                self._delete(pos, idx);
                return Ok(());
            }
            idx += 1;
            if idx == len_sublist {
                pos += 1;
                if pos == len_keys {
                    return Err(not_in_list());
                }
                len_sublist = self._keys[pos].len();
                idx = 0;
            }
        }
    }

    /// `_delete(pos, idx)` (`:2054-2104`): Delete value at the given `(pos, idx)`. Combines lists
    /// that are less than half the load level.
    fn _delete(&mut self, mut pos: usize, idx: usize) {
        self._keys[pos].remove(idx);
        self._lists[pos].remove(idx);
        self._len -= 1;

        let len_keys_pos = self._keys[pos].len();

        if len_keys_pos > (self._load >> 1) {
            self._maxes[pos] = self._keys[pos].last().unwrap().clone();

            if !self._index.is_empty() {
                let mut child = self._offset + pos;
                while child > 0 {
                    self._index[child] -= 1;
                    child = (child - 1) >> 1;
                }
                self._index[0] -= 1;
            }
        } else if self._keys.len() > 1 {
            if pos == 0 {
                pos += 1;
            }

            let prev = pos - 1;
            let keys = std::mem::take(&mut self._keys[pos]);
            self._keys[prev].extend(keys);
            let lists = std::mem::take(&mut self._lists[pos]);
            self._lists[prev].extend(lists);
            self._maxes[prev] = self._keys[prev].last().unwrap().clone();

            self._lists.remove(pos);
            self._keys.remove(pos);
            self._maxes.remove(pos);
            self._index.clear();

            self._expand(prev);
        } else if len_keys_pos != 0 {
            self._maxes[pos] = self._keys[pos].last().unwrap().clone();
        } else {
            self._lists.remove(pos);
            self._keys.remove(pos);
            self._maxes.remove(pos);
        }
    }

    /// `__iter__()` (`:920-928`): `chain.from_iterable(self._lists)`
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self._lists.iter().flatten()
    }

    /// The sublists (`_lists`), for the test against the library's own structure.
    pub fn sublists(&self) -> &[Vec<T>] {
        &self._lists
    }
}

#[cfg(test)]
mod tests;
