"""Spoolman's OpenPrintTag decoder, run on a corpus of tags: the reference for the hub's port.

Run by scripts/spoolman-audit-check.py with the Python of reference/.venv-spoolman (`-I`), so the
decoder is Spoolman's own `spoolman.tags.decode.decode` at the audited commit, with the cbor2 of
its uv.lock (its C decoder, as Spoolman runs it). Writes
crates/hub/src/spoolman/openprinttag-fixture.json; the Rust test `spoolman::openprinttag::tests`
decodes the same cases and compares every field.

The corpus, every case decoded by Spoolman:
- the two real tags in Spoolman's tests (Prusa's sample tag and test vector 01), with UIDs in
  both byte orders, a UID of another length and none, and the format spelled several ways;
- every truncation of both, and every byte of their NDEF message replaced by each of BYTES;
- every main field of Spoolman's decoder with each CBOR value of VALUES, built with Spoolman's
  own test builder (tests_integration/tests/_openprinttag_fixtures.py);
- hand-made payloads for what that builder cannot make (RAW below): duplicate and mixed-type
  keys, offsets, nesting depth, semantic tags, simple values, indefinite lengths, bad UTF-8.
"""

import base64
import importlib.metadata
import json
import math
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REF = ROOT / "reference" / "Spoolman"
sys.path.insert(0, str(REF))
sys.path.insert(0, str(REF / "tests_integration" / "tests"))

import cbor2  # noqa: E402
import logging  # noqa: E402

logging.disable(logging.CRITICAL)  # Spoolman logs every refused tag; the result is what counts

from _openprinttag_fixtures import (  # noqa: E402
    REAL_PRUSA_SAMPLE_TAG_B64,
    REAL_PRUSA_TEST01_B64,
    REAL_PRUSA_TEST01_TAG_UID_HEX,
    build_openprinttag,
    build_openprinttag_from_payload,
)
from spoolman.tags import decode as tag_decode  # noqa: E402

OUT = ROOT / "crates" / "hub" / "src" / "spoolman" / "openprinttag-fixture.json"

BASES = {
    "sample": base64.b64decode(REAL_PRUSA_SAMPLE_TAG_B64),
    "test01": base64.b64decode(REAL_PRUSA_TEST01_B64),
}
# Head bytes of every CBOR major type and the argument sizes, the TLV/NDEF markers, and plain values.
BYTES = [0x00, 0x01, 0x03, 0x10, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1F, 0x20, 0x3F, 0x40, 0x5F, 0x60,
         0x7F, 0x80, 0x9F, 0xA0, 0xBF, 0xC0, 0xD8, 0xE0, 0xE1, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA,
         0xFB, 0xFE, 0xFF]
MAIN_KEYS = [0, 4, 8, 9, 10, 11, 16, 17, 18, 19, 29, 30, 34, 35, 37, 38]
VALUES = [0, 1, 2, 42, -1, 255, 2**31 - 1, 2**31, -(2**31), -(2**31) - 1, 2**53, 2**53 + 1, 2**63 - 1, 2**63,
          2**64 - 1, -(2**64), 0.0, -0.0, 0.5, 1.75, 1.24, 1000.0, 5e-324, 1e308, math.nan, math.inf, -math.inf,
          True, False, None, cbor2.undefined, "", "PLA", "x" * 64, "x" * 65, "x" * 1024, "x" * 1025, "é" * 600,
          b"", b"\x01\x02", b"\x3d\x3e\x3d", b"\x3d\x3e\x3d\xff", bytes(range(16)), bytes(range(17)), [1], [],
          {1: 2}, {}]
BASE_MAIN = {8: 0, 9: 0, 10: "PLA", 11: "Vendor", 29: 1.24, 30: 1.75, 16: 1000.0, 18: 140.0, 19: b"\x3d\x3e\x3d"}


def nfcv(payload: bytes) -> bytes:
    """A CBOR payload in an NDEF record in NFC-V memory; long payloads in a standard (4-byte length) record."""
    if len(payload) < 256:
        return build_openprinttag_from_payload(payload)
    mime = b"application/vnd.openprinttag"
    record = bytes([0b11000010, len(mime)]) + len(payload).to_bytes(4, "big") + mime + payload
    tlv = bytes([0x03, 0xFF]) + len(record).to_bytes(2, "big") + record + bytes([0xFE])
    return bytes([0xE1, 0x40, 0x00, 0x01]) + tlv


def h(*parts: str) -> bytes:
    return bytes.fromhex("".join(parts))


MAIN = cbor2.dumps(BASE_MAIN).hex()
# Payloads by hand: meta, then main (and aux), as CBOR hex.
RAW = {
    "duplicate key, last value wins": h("a0", "a2", "0a63414141", "0a63424242"),
    "key 10 as float 10.0": h("a0", "a1", "f94900", "63414141"),
    "key 10 as half 10.0 then int": h("a0", "a2", "f94900", "63414141", "0a63424242"),
    "key 8 as false": h("a0", "a2", "f4", "00", "09", "00"),
    "key 9 as true is key 1": h("a0", "a1", "f5", "00"),
    "key as simple 10": h("a0", "a1", "ea", "63414141"),
    "key as text": h("a0", "a1", "6131", "63414141"),
    "key as array": h("a0", "a2", "8100", "00", "0a6141"),
    "key as map": h("a0", "a2", "a10000", "00", "0a6141"),
    "key nan": h("a0", "a2", "f97e00", "00", "0a6141"),
    "class simple 0": h("a0", "a1", "08", "e0"),
    "class 1.0": h("a0", "a1", "08", "f93c00"),
    "class true": h("a0", "a1", "08", "f5"),
    "class nan": h("a0", "a1", "08", "f97e00"),
    "class text": h("a0", "a1", "08", "6130"),
    "class array": h("a0", "a1", "08", "8100"),
    "class map": h("a0", "a1", "08", "a0"),
    "type -0.0": h("a0", "a1", "09", "f98000"),
    "type 42.0": h("a0", "a1", "09", "f95140"),
    "type 43": h("a0", "a1", "09", "182b"),
    "type undefined": h("a0", "a1", "09", "f7"),
    "meta main offset true": h("a1", "00", "f5", "a1", "0a", "6141"),
    "meta main offset 1.0": h("a1", "00", "f93c00", "a1", "0a", "6141"),
    "meta main offset negative": h("a1", "00", "20", "a1", "0a", "6141"),
    "meta main offset huge": h("a1", "00", "1bffffffffffffffff", "a1", "0a", "6141"),
    "meta main offset past end": h("a1", "00", "1864", "a1", "0a", "6141"),
    "meta main offset 0 is meta": h("a1", "00", "00"),
    "meta main offset as key 0.0": h("a1", "f90000", "03", "a1", "0a", "6141"),
    "meta main offset as key false": h("a1", "f4", "03", "a1", "0a", "6141"),
    "meta main offset into main": h("a1", "00", "04", "a1", "0a", "6141"),
    "meta aux offset text": h("a1", "02", "6131", MAIN),
    "meta aux offset negative": h("a1", "02", "20", MAIN),
    "meta aux offset true": h("a1", "02", "f5", MAIN),
    "meta not a map": h("80", MAIN),
    "main not a map": h("a0", "80"),
    "no main": h("a0"),
    "meta truncated": h("a1", "00"),
    "aux consumed int": h("a1", "02", "18" + format(4 + len(MAIN) // 2, "02x"), MAIN, "a10019012c"),
    "aux consumed text": h("a1", "02", "18" + format(4 + len(MAIN) // 2, "02x"), MAIN, "a1006131"),
    "aux consumed bignum int": h("a1", "02", "18" + format(4 + len(MAIN) // 2, "02x"), MAIN, "a1001b0020000000000001"),
    "aux consumed nan": h("a1", "02", "18" + format(4 + len(MAIN) // 2, "02x"), MAIN, "a100f97e00"),
    "aux not a map": h("a1", "02", "18" + format(4 + len(MAIN) // 2, "02x"), MAIN, "80"),
    "aux truncated": h("a1", "02", "18" + format(4 + len(MAIN) // 2, "02x"), MAIN, "a100"),
    "aux semantic tag": h("a1", "02", "18" + format(4 + len(MAIN) // 2, "02x"), MAIN, "a100c100"),
    "aux at 0 is meta": h("a1", "02", "00", MAIN),
    "semantic tag in main": h("a0", "a1", "1d", "c11a00000000"),
    "semantic tag 55799 around main": h("a0", "d9d9f7", "a1", "0a", "6141"),
    "semantic tag in meta": h("a1", "05", "c100", MAIN),
    "shared reference bomb": h("9f" + "d81c8100" * 2 + "d9010481d81d01" + "ff"),
    "simple 24 value 24": h("a0", "a1", "0a", "f818"),
    "simple 24 value 31": h("a0", "a1", "0a", "f81f"),
    "simple 24 value 32": h("a0", "a1", "0a", "f820"),
    "simple 28 reserved": h("a0", "a1", "0a", "fc"),
    "stray break": h("a0", "a1", "0a", "ff"),
    "break in definite array": h("a0", "a1", "0a", "81ff"),
    "indefinite map": h("a0", "bf", "0a", "6141", "ff"),
    "indefinite map odd": h("a0", "bf", "0a", "ff"),
    "indefinite text": h("a0", "a1", "0a", "7f", "6150", "614c", "6141", "ff"),
    "indefinite text bad chunk": h("a0", "a1", "0a", "7f", "4150", "ff"),
    "indefinite text nested indefinite": h("a0", "a1", "0a", "7f", "7fff", "ff"),
    "indefinite bytes colour": h("a0", "a1", "13", "5f", "423d3e", "413d", "ff"),
    "indefinite uint": h("a0", "a1", "0a", "1f"),
    "reserved info 28": h("a0", "a1", "0a", "1c"),
    "text bad utf-8": h("a0", "a1", "0a", "62c328"),
    "text surrogate": h("a0", "a1", "0a", "63eda080"),
    "text in indefinite bad utf-8": h("a0", "a1", "0a", "7f", "61c3", "6128", "ff"),
    "text 1024 code points of 2 bytes": h("a0", "a1", "0a", "7a00000800", "c3a9" * 1024),
    "half float density": h("a0", "a1", "181d", "f93cf6"),
    "float32 density": h("a0", "a1", "181d", "fa3f9eb852"),
    "half subnormal density": h("a0", "a1", "181d", "f90001"),
    "half inf weight": h("a0", "a1", "10", "f97c00"),
    "gtin 0": h("a0", "a1", "04", "00"),
    "gtin float": h("a0", "a1", "04", "f93c00"),
    "temps max 0 min 200": h("a0", "a2", "1822", "18c8", "1823", "00"),
    "temps max 0 min none": h("a0", "a1", "1823", "00"),
    "temps min only": h("a0", "a1", "1822", "18c8"),
    "bed max 0 min 0": h("a0", "a2", "1825", "00", "1826", "00"),
    "instance uuid 15 bytes": h("a0", "a1", "00", "4f" + "11" * 15),
    "instance uuid zero": h("a0", "a1", "00", "50" + "00" * 16),
}
for n in range(4, 12):
    RAW[f"nested arrays {n}"] = h("a0", "a1", "0a", "81" * n + "00")
    RAW[f"nested indefinite {n}"] = h("a0", "a1", "0a", "81" * n + "9fff")
    RAW[f"nested maps {n}"] = h("a0", "a1", "0a", "a100" * n + "00")
    RAW[f"nested in key {n}"] = h("a0", "a1", "81" * n + "00", "00")
RAW["nested 70 arrays"] = h("a0", "a1", "0a", "81" * 70 + "00")

UIDS = [None, "E0040108662F6FBC", "BC6F2F66080104E0", "E004010866", "04A2B3C4D5E6F7", "E0E0E0E0E0E0E0E0",
        "00000000000000E0"]
FORMATS = ["openprinttag", "OpenPrintTag", " openprinttag ", "ntag", "", None]


class Corpus:
    def __init__(self):
        self.results: list[dict | None] = []
        self.index: dict[str, int] = {}
        self.cases: list[dict] = []

    def add(self, case: dict, payload: bytes, uid: str | None = None, fmt: str | None = "openprinttag"):
        decoded = tag_decode.decode(fmt, payload, uid_bytes=bytes.fromhex(uid) if uid else None)
        value = None if decoded is None else {k: getattr(decoded, k) for k in decoded.__dataclass_fields__}
        key = json.dumps(value, sort_keys=True)
        if key not in self.index:
            self.index[key] = len(self.results)
            self.results.append(value)
        case = {**case, "result": self.index[key]}
        if uid is not None:
            case["uid"] = uid
        if fmt != "openprinttag":
            case["format"] = fmt
        self.cases.append(case)


def main():
    c = Corpus()
    for name, raw in BASES.items():
        for uid in UIDS:
            for fmt in FORMATS:
                c.add({"base": name}, raw, uid, fmt)
        uid = REAL_PRUSA_TEST01_TAG_UID_HEX if name == "test01" else None
        for cut in range(len(raw)):
            c.add({"base": name, "cut": cut}, raw[:cut], uid)
        # The NDEF message: from the capability container to the terminator TLV.
        end = raw.index(0xFE, 4 + 4 + raw[7]) + 1 if raw[5] == 0xFF else len(raw)
        for at in range(end):
            for byte in BYTES:
                if raw[at] != byte:
                    c.add({"base": name, "at": at, "byte": byte}, raw[:at] + bytes([byte]) + raw[at + 1:], uid)
    for key in MAIN_KEYS:
        for value in VALUES:
            main = {**BASE_MAIN, key: value}
            payload = cbor2.dumps({}) + cbor2.dumps(main)
            c.add({"payload": payload.hex()}, nfcv(payload), "E0040108662F6FBC")
    for value in VALUES:
        payload = cbor2.dumps({2: 0}) + cbor2.dumps(BASE_MAIN)
        aux_at = len(payload)
        payload = cbor2.dumps({2: aux_at}) + cbor2.dumps(BASE_MAIN)
        aux_at2 = len(payload)
        payload = cbor2.dumps({2: aux_at2}) + cbor2.dumps(BASE_MAIN) + cbor2.dumps({0: value})
        c.add({"payload": payload.hex()}, nfcv(payload))
    for value in VALUES:
        payload = cbor2.dumps({0: value}) + cbor2.dumps(BASE_MAIN)
        c.add({"payload": payload.hex()}, nfcv(payload))
    for name, payload in RAW.items():
        c.add({"name": name, "payload": payload.hex()}, nfcv(payload), "E0040108662F6FBC")
    # The same builder Spoolman's tests use, with an aux section.
    c.add({"memory": build_openprinttag(BASE_MAIN, {0: 120.5}).hex()}, build_openprinttag(BASE_MAIN, {0: 120.5}))
    # NFC-V memory that is not an OpenPrintTag: blank, other capability containers, other records.
    for name, memory in {
        "blank": bytes(320),
        "short": h("e140"),
        "not e1": h("e240000103") + bytes(10),
        "only terminator": h("e1400001fe"),
        "null tlvs": h("e1400001000000"),
        "other tlv then ndef": h("e14000010102aabb") + build_openprinttag(BASE_MAIN)[4:],
        "tlv length past end": h("e14000010364d2"),
        "three byte length truncated": h("e140000103ff01"),
        "ndef record other mime": h("e1400001030dd2010a" + b"text/plain".hex()) + bytes(10),
        "ndef record well-known": h("e140000103") + bytes([len(b"application/vnd.openprinttag") + 3 + 2, 0xD1,
                                                          len(b"application/vnd.openprinttag"), 2])
        + b"application/vnd.openprinttag" + h("a0a0fe"),
        "ndef record with id": h("e140000103") + bytes([3 + 1 + 28 + 2 + 4, 0xDA, 28, 4, 2]) + h("aabb")
        + b"application/vnd.openprinttag" + h("a0a10a6141fe"),
        "ndef two records": h("e140000103") + bytes([3 + 1 + 10 + 3 + 28 + 4, 0x92, 10, 1]) + b"text/plain" + h("00")
        + bytes([0x52, 28, 4]) + b"application/vnd.openprinttag" + h("a0a10a6141") + h("fe"),
        "ndef message end before ours": h("e140000103") + bytes([3 + 10 + 1 + 3 + 28 + 4, 0xD2, 10, 1])
        + b"text/plain" + h("00") + bytes([0x52, 28, 4]) + b"application/vnd.openprinttag" + h("a0a10a6141")
        + h("fe"),
        "ndef payload length past end": h("e140000103") + bytes([3 + 28 + 2, 0xD2, 28, 200])
        + b"application/vnd.openprinttag" + h("a0a0fe"),
        "ndef long record truncated length": h("e14000010303c21c00"),
    }.items():
        c.add({"name": name, "memory": memory.hex()}, memory)
    OUT.write_text(json.dumps({
        "reference": "Spoolman (Donkie/Spoolman, MIT) eccb05e: spoolman.tags.decode.decode, cbor2 "
                     + importlib.metadata.version("cbor2"),
        "bases": {k: v.hex() for k, v in BASES.items()},
        "results": c.results,
        "cases": c.cases,
    }, separators=(",", ":")) + "\n")
    print(f"{len(c.cases)} tags decoded by Spoolman, {len(c.results)} distinct results -> {OUT}")


main()
