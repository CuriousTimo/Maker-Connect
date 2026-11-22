import hashlib
import unidecode

h = hashlib.md5()
for cp in range(0x110000):
    if 0xd800 <= cp <= 0xdfff:
        continue
    h.update(unidecode.unidecode(chr(cp)).encode('ascii'))
    h.update(b'\0')
strings = json.load(sys.stdin)
print(json.dumps({"md5": h.hexdigest(), "strings": [unidecode.unidecode(s) for s in strings]}))
