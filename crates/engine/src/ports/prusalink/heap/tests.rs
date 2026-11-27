use super::*;
use crate::ports::prusalink::python;
use serde_json::{Value, json};

struct Item {
    ident: u64,
    item: HeapItem,
}

impl AsHeapItem for Item {
    fn heap_item(&self) -> &HeapItem {
        &self.item
    }
    fn heap_item_mut(&mut self) -> &mut HeapItem {
        &mut self.item
    }
}

fn ops() -> Vec<Value> {
    let mut seed: u64 = 99;
    let mut next = |n: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % n
    };
    let mut ops = vec![];
    let mut inside: [Vec<u64>; 2] = [vec![], vec![]];
    let numbers = [0.5, 0.13, 1.0, 0.13, 0.25, 2.0, 0.0, 0.75];
    for ident in 0..800_u64 {
        let k = next(2) as usize;
        let kind = ["min", "max"][k];
        match next(10) {
            0..=4 => {
                ops.push(json!([kind, "push", ident, numbers[next(8) as usize]]));
                inside[k].push(ident);
            }
            5..=7 if !inside[k].is_empty() => {
                let i = next(inside[k].len() as u64) as usize;
                let id = inside[k].remove(i);
                ops.push(json!([kind, "pop_item", id, 0]));
            }
            8 if !inside[k].is_empty() => {
                // a plain index: mostly the root, sometimes past the end
                let index = if next(3) == 0 {
                    inside[k].len() + next(3) as usize
                } else {
                    0
                };
                ops.push(json!([kind, "pop_index", ident, index]));
                // which one went is read back from the result below
                inside[k].clear();
            }
            _ => ops.push(json!([kind, "pop_index", ident, 0])),
        }
    }
    ops
}

/// PrusaLink's own heaps and ours through the same 800 operations (pushes with equal values, pops
/// by `heap_index` anywhere in the heap, pops by plain index, past the end): after each, the same
/// result and the same items with the same `heap_value` and `heap_index` in the same places.
#[test]
fn identical_to_prusalink() {
    let ops = ops();
    let Some(theirs) = python::run(include_str!("../driver_heap.py"), &json!({ "ops": ops }))
    else {
        return;
    };
    let theirs = theirs.as_array().unwrap();
    let mut min: MinHeap<Item> = MinHeap::new();
    let mut max: MaxHeap<Item> = MinHeap::new_max();
    let mut items: std::collections::HashMap<u64, Shared<Item>> = Default::default();
    for (op, their) in ops.iter().zip(theirs) {
        let heap = if op[0] == "min" { &mut min } else { &mut max };
        let ident = op[2].as_u64().unwrap();
        let shown = |r: Result<Shared<Item>, PyErr>| match r {
            Ok(item) => json!(item.lock().unwrap().ident),
            Err(error) => json!([error.class_name(), error.message()]),
        };
        let result = match op[1].as_str().unwrap() {
            "push" => {
                let item = Arc::new(Mutex::new(Item {
                    ident,
                    item: HeapItem::new(op[3].as_f64().unwrap()),
                }));
                items.insert(ident, item.clone());
                heap.push(item);
                Value::Null
            }
            "pop_item" => {
                let index = items[&ident].lock().unwrap().item.heap_index.unwrap();
                shown(heap.pop(index))
            }
            _ => shown(heap.pop(op[3].as_u64().unwrap() as usize)),
        };
        let state: Vec<Value> = heap
            .heap
            .iter()
            .map(|i| {
                let i = i.lock().unwrap();
                json!([i.ident, i.item.heap_value, i.item.heap_index])
            })
            .collect();
        assert_eq!(json!([result, state]), *their, "{op}");
    }
}
