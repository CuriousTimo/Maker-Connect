from prusa.link.printer_adapter.structures.heap import HeapItem, MaxHeap, MinHeap

# The operations on a MinHeap and a MaxHeap: push a new item, or pop one by its `heap_index` (as
# is_planner_fed.py does) or by a plain index; after each: the result, and each item's id,
# heap_value and heap_index in the heap's order.
request = json.load(sys.stdin)
heaps = {"min": MinHeap(), "max": MaxHeap()}
items = {}
steps = []
for kind, op, ident, number in request["ops"]:
    heap = heaps[kind]
    try:
        if op == "push":
            item = HeapItem(number)
            item.ident = ident
            items[ident] = item
            heap.push(item)
            result = None
        elif op == "pop_item":
            result = heap.pop(items[ident].heap_index).ident
        else:
            result = heap.pop(number).ident
    except (IndexError, RuntimeError) as error:
        result = [type(error).__name__, str(error)]
    steps.append([result, [[i.ident, i.heap_value, i.heap_index] for i in heap.heap]])
print(json.dumps(steps))
