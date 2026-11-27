from sortedcontainers import SortedKeyList

# The operations on a SortedKeyList keyed as SerialParser's (`-item.priority`), its load factor
# set small so sublists split and join; after each one: the result, the order and the sublists.
request = json.load(sys.stdin)


class P:
    """A pairing: equal only to itself, as RegexPairing"""
    def __init__(self, ident, priority):
        self.ident, self.priority = ident, priority

    def __repr__(self):
        return f"P({self.ident})"


skl = SortedKeyList(key=lambda item: -item.priority)
skl._reset(request["load"])
made = {}
steps = []
for op, ident, priority in request["ops"]:
    if ident not in made:
        made[ident] = P(ident, priority)
    item = made[ident]
    if op == "add":
        result = skl.add(item)
    elif op == "contains":
        result = item in skl
    else:
        try:
            result = skl.remove(item)
        except ValueError as error:
            result = ["ValueError", str(error)]
    steps.append([result, [i.ident for i in skl],
                  [[i.ident for i in sub] for sub in skl._lists]])
print(json.dumps(steps))
