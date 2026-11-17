//! Python containers the ported classes use, written for the behaviour OctoPrint relies on
//! (CPython 3.13: `Lib/collections/__init__.py` documents `deque`; the C implementation is
//! `Modules/_collectionsmodule.c`; `dict` keeps insertion order).

use std::collections::VecDeque;

/// `collections.deque(iterable, maxlen)`: when full, `append` discards from the other end
/// (`maxlen=None` is unbounded; `maxlen=0` keeps nothing). Only the operations the port uses.
#[derive(Debug, Clone, PartialEq)]
pub struct PyDeque<T> {
    items: VecDeque<T>,
    maxlen: Option<usize>,
}

impl<T> PyDeque<T> {
    /// `deque([], maxlen)`. (Python raises `ValueError` for a negative `maxlen`; callers pass
    /// an `Option<usize>` after that check.)
    pub fn new(maxlen: Option<usize>) -> Self {
        PyDeque {
            items: VecDeque::new(),
            maxlen,
        }
    }

    pub fn maxlen(&self) -> Option<usize> {
        self.maxlen
    }

    /// `append(x)`
    pub fn append(&mut self, x: T) {
        if self.maxlen == Some(0) {
            return;
        }
        self.items.push_back(x);
        if let Some(m) = self.maxlen {
            while self.items.len() > m {
                self.items.pop_front();
            }
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// `clear()`
    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> std::collections::vec_deque::Iter<'_, T> {
        self.items.iter()
    }

    /// `d[i]` for a non-negative or negative index (`None` is `IndexError`).
    pub fn get(&self, i: isize) -> Option<&T> {
        let idx = if i < 0 {
            self.items.len().checked_sub(i.unsigned_abs())?
        } else {
            i as usize
        };
        self.items.get(idx)
    }
}

/// `dict`: insertion-ordered. Only the operations the port uses.
#[derive(Debug, Clone)]
pub struct PyDict<V>(Vec<(String, V)>);

/// `dict == dict`: the same keys with equal values, in any order.
impl<V: PartialEq> PartialEq for PyDict<V> {
    fn eq(&self, other: &Self) -> bool {
        self.0.len() == other.0.len() && self.0.iter().all(|(k, v)| other.get(k) == Some(v))
    }
}

impl<V> Default for PyDict<V> {
    fn default() -> Self {
        PyDict(Vec::new())
    }
}

impl<V> PyDict<V> {
    pub fn new() -> Self {
        PyDict(Vec::new())
    }

    /// `d[k] = v` (an existing key keeps its place)
    pub fn insert(&mut self, k: &str, v: V) {
        match self.0.iter_mut().find(|(x, _)| x == k) {
            Some(e) => e.1 = v,
            None => self.0.push((k.to_string(), v)),
        }
    }

    /// `d.get(k)`
    pub fn get(&self, k: &str) -> Option<&V> {
        self.0.iter().find(|(x, _)| x == k).map(|(_, v)| v)
    }

    pub fn contains_key(&self, k: &str) -> bool {
        self.get(k).is_some()
    }

    /// `d.pop(k)`
    pub fn pop(&mut self, k: &str) -> Option<V> {
        let i = self.0.iter().position(|(x, _)| x == k)?;
        Some(self.0.remove(i).1)
    }

    /// `d.update(other)`
    pub fn update(&mut self, other: impl IntoIterator<Item = (String, V)>) {
        for (k, v) in other {
            self.insert(&k, v);
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &V)> {
        self.0.iter().map(|(k, v)| (k, v))
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// `d.keys()`
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.0.iter().map(|(k, _)| k)
    }
}

/// `dict(iterable_of_pairs)`: a repeated key keeps its first place and takes the last value.
impl<V> FromIterator<(String, V)> for PyDict<V> {
    fn from_iter<I: IntoIterator<Item = (String, V)>>(iter: I) -> Self {
        let mut d = PyDict::new();
        d.update(iter);
        d
    }
}

#[cfg(test)]
mod tests {
    //! OURS.
    use super::*;

    #[test]
    fn deque_maxlen() {
        let mut d = PyDeque::new(Some(2));
        d.append(1);
        d.append(2);
        d.append(3);
        assert_eq!(vec![&2, &3], d.iter().collect::<Vec<_>>());
        assert_eq!(Some(&3), d.get(-1));
        assert_eq!(None, d.get(-3));
        let mut z = PyDeque::new(Some(0));
        z.append(1);
        assert!(z.is_empty());
        let mut u = PyDeque::new(None);
        for i in 0..100 {
            u.append(i);
        }
        assert_eq!(100, u.len());
    }

    #[test]
    fn dict_order() {
        let mut d = PyDict::new();
        d.insert("b", 1);
        d.insert("a", 2);
        d.insert("b", 3);
        assert_eq!(
            vec![("b", 3), ("a", 2)],
            d.iter().map(|(k, v)| (k.as_str(), *v)).collect::<Vec<_>>()
        );
        assert_eq!(Some(2), d.pop("a"));
        assert_eq!(None, d.get("a"));
    }

    #[test]
    fn dict_eq_ignores_order() {
        let a: PyDict<i32> = [("a".to_string(), 1), ("b".to_string(), 2)]
            .into_iter()
            .collect();
        let b: PyDict<i32> = [("b".to_string(), 2), ("a".to_string(), 1)]
            .into_iter()
            .collect();
        assert_eq!(a, b);
        assert_eq!(
            vec!["a", "b"],
            a.keys().map(|k| k.as_str()).collect::<Vec<_>>()
        );
        assert_eq!(
            vec!["b", "a"],
            b.keys().map(|k| k.as_str()).collect::<Vec<_>>()
        );
    }
}
