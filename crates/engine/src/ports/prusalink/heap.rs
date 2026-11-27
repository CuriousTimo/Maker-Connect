//! PrusaLink's `printer_adapter/structures/heap.py` (`reference/Prusa-Link`, `171148ce`): Contains
//! implementation of the HeapItem, MinHeap and MaxHeap classes. "I HAVE COPIED THIS FROM THE
//! INTERNET! It turns out that popping the last item in the queue was broken" (#242, section A).
//!
//! An item is shared: the heap holds it and so does its owner (`is_planner_fed.py` keeps its
//! `TimeValue`s in a queue and pops them by their `heap_index`). So the heap holds
//! [`Shared`] items, and a subclass of `HeapItem` (`TimeValue`) is a type that holds one
//! ([`AsHeapItem`]).
//!
//! **Not covered:** comparing a `HeapItem` with something else (`TypeError`, `:17-40`): the types
//! allow only items; `__hash__` (`:42-43`): unused.

use std::sync::{Arc, Mutex};

use maker_connect_gcode_metadata::py::PyErr;

/// An item the heap and its owner both hold.
pub type Shared<T> = Arc<Mutex<T>>;

/// `HeapItem` (`:10-43`): An item in the heap. Needs to be comparable
#[derive(Debug, Clone, PartialEq)]
pub struct HeapItem {
    pub value: f64,
    pub heap_value: Option<f64>,
    pub heap_index: Option<usize>,
}

impl HeapItem {
    /// `__init__(value)` (`:12-15`)
    pub fn new(value: f64) -> HeapItem {
        HeapItem {
            value,
            heap_value: None,
            heap_index: None,
        }
    }
}

/// A `HeapItem` or a subclass of it.
pub trait AsHeapItem {
    fn heap_item(&self) -> &HeapItem;
    fn heap_item_mut(&mut self) -> &mut HeapItem;
}

impl AsHeapItem for HeapItem {
    fn heap_item(&self) -> &HeapItem {
        self
    }
    fn heap_item_mut(&mut self) -> &mut HeapItem {
        self
    }
}

/// `heap_value` of an item; `None` compared is Python's `TypeError`.
fn heap_value<T: AsHeapItem>(item: &Shared<T>) -> f64 {
    item.lock()
        .unwrap()
        .heap_item()
        .heap_value
        .expect("TypeError: a HeapItem compared before it was pushed")
}

fn set_heap_index<T: AsHeapItem>(item: &Shared<T>, index: usize) {
    item.lock().unwrap().heap_item_mut().heap_index = Some(index);
}

/// `MinHeap` (`:46-170`) and `MaxHeap(MinHeap)` (`:173-180`): the same but for `push`.
pub struct MinHeap<T> {
    pub heap: Vec<Shared<T>>,
    /// `MaxHeap`: Lazily implemented max heap by using the min heap, just inverting the heap value
    max: bool,
}

/// `MaxHeap` (`:173-180`)
pub type MaxHeap<T> = MinHeap<T>;

impl<T: AsHeapItem> MinHeap<T> {
    /// `MinHeap()` (`:48-49`)
    pub fn new() -> MinHeap<T> {
        MinHeap {
            heap: vec![],
            max: false,
        }
    }

    /// `MaxHeap()`
    pub fn new_max() -> MaxHeap<T> {
        MinHeap {
            heap: vec![],
            max: true,
        }
    }

    /// `__len__` (`:51-52`)
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    /// `__bool__` (`:54-55`)
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    /// `__getitem__` (`:57-58`)
    pub fn get(&self, key: usize) -> Option<&Shared<T>> {
        self.heap.get(key)
    }

    /// `push` (`:63-66`): Ads an element to the heap. `MaxHeap.push` (`:177-180`):
    /// `item.heap_value = -item.value`.
    pub fn push(&mut self, item: Shared<T>) {
        {
            let mut guard = item.lock().unwrap();
            let it = guard.heap_item_mut();
            it.heap_value = Some(if self.max { -it.value } else { it.value });
        }
        self._push(item);
    }

    /// `_push` (`:68-79`): Adds an element to the heap. In min heaps this is done by adding the
    /// element at the end, then switching places with parents larger than it
    fn _push(&mut self, item: Shared<T>) {
        self.heap.push(item);

        let initial_index = self.heap.len() - 1;
        self.sift_down(0, initial_index);
    }

    /// `pop(index=0)` (`:81-109`): Removes an element from the heap. In min heaps this is done by
    /// swapping the element with the last element in the heap, then removing the last element,
    /// (which is now the thing we wanted to delete). After that depending on the value of the
    /// element that replaced the deleted one sifting it up or down
    pub fn pop(&mut self, index: usize) -> Result<Shared<T>, PyErr> {
        let Some(old_item) = self.heap.get(index).cloned() else {
            return Err(PyErr::Other(
                "IndexError",
                "list index out of range".to_owned(),
            ));
        };
        let old_value = heap_value(&old_item);

        if old_item.lock().unwrap().heap_item().heap_index != Some(index) {
            return Err(PyErr::Other(
                "RuntimeError",
                "Item index and actual index differ. NOOOOO!".to_owned(),
            ));
        }

        let new_item = self.heap.pop().unwrap();

        // The first one checks if we didn't remove the last item from the
        // heap, if we did, there is nothing else that needs to be done
        if index != self.len() && !self.is_empty() {
            let new_value = heap_value(&new_item);

            self.heap[index] = new_item;
            if new_value > old_value {
                self.sift_up(index);
            } else {
                self.sift_down(0, index);
            }
        }

        Ok(old_item)
    }

    /// `sift_up` (`:111-137`): Compares an element with its children, if the element is larger,
    /// its position gets swapped with the smaller child. Continues until there are no children
    /// smaller than the element
    pub fn sift_up(&mut self, mut pos: usize) {
        let endpos = self.heap.len();
        let startpos = pos;
        let newitem = self.heap[pos].clone();
        // Bubble up the smaller child until hitting a leaf.
        let mut childpos = 2 * pos + 1; // leftmost child position
        while childpos < endpos {
            // Set childpos to index of smaller child.
            let rightpos = childpos + 1;
            if rightpos < endpos
                && !(heap_value(&self.heap[childpos]) < heap_value(&self.heap[rightpos]))
            {
                childpos = rightpos;
            }
            // Move the smaller child up.
            self.heap[pos] = self.heap[childpos].clone();
            set_heap_index(&self.heap[pos], pos);
            pos = childpos;
            childpos = 2 * pos + 1;
        }
        // The leaf at pos is empty now. Put newitem there, and bubble it up
        // to its final resting place (by sifting its parents down).
        self.heap[pos] = newitem;
        set_heap_index(&self.heap[pos], pos);
        self.sift_down(startpos, pos);
    }

    /// `sift_down` (`:139-163`): The element gets compared with its parent, if it's smaller, they
    /// get swapped. Continues until it finds a parent smaller than itself, or the element becomes
    /// the root of the heap
    pub fn sift_down(&mut self, startpos: usize, mut pos: usize) {
        let newitem = self.heap[pos].clone();
        // Follow the path to the root, moving parents down until finding
        // a place newitem fits.
        while pos > startpos {
            let parentpos = (pos - 1) >> 1;
            let parent = self.heap[parentpos].clone();
            if heap_value(&newitem) < heap_value(&parent) {
                self.heap[pos] = parent.clone();
                set_heap_index(&parent, pos);
                pos = parentpos;
                continue;
            }
            break;
        }
        self.heap[pos] = newitem.clone();
        set_heap_index(&newitem, pos);
    }
}

impl<T: AsHeapItem> Default for MinHeap<T> {
    fn default() -> Self {
        MinHeap::new()
    }
}

#[cfg(test)]
mod tests;
