from pydantic.utils import deep_update

# Each case: the mapping and the updating mappings; `deep_update`'s result as pairs in its order.
request = json.load(sys.stdin)


def pairs(value):
    if isinstance(value, dict):
        return [[k, pairs(v)] for k, v in value.items()]
    return value


print(json.dumps([pairs(deep_update(mapping, *updating))
                  for mapping, updating in request["cases"]]))
