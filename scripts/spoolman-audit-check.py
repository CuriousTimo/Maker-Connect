#!/usr/bin/env python3
"""Spoolman API audit: real Spoolman answers a request list, recorded as the reference fixture.

The one reference is Spoolman (Donkie/Spoolman, MIT) at SPOOLMAN_COMMIT, checked out in
reference/Spoolman and run with its own runtime from its own uv.lock in reference/.venv-spoolman.
Run with any Python; the script re-runs itself in nothing (it only starts the venv's uvicorn).

1. The routes come from Spoolman's own OpenAPI (`spoolman.api.v1.router.app.openapi()`, read
   by the venv's Python): every (method, path) and every query parameter of it. The request list
   below is written per route; the script refuses to run when a route or a query parameter has
   no request and no entry in NOT_COVERED (with the reason).
2. Spoolman starts on SQLite in a fresh temp dir on a free localhost port (cwd a temp dir that
   holds a copy of pyproject.toml, because Spoolman reads its version from ./pyproject.toml, and
   an empty client_v2/build, because it refuses to start without its web client).
3. Every request is sent, in order, with http.client (no redirects followed, no gzip asked for).
   Recorded: status, every response header except those in IGNORED_HEADERS, the body, with the
   volatile values normalised by the rules in NORMALISATION.
4. Spoolman is stopped and the fixture is written to
   crates/hub/src/spoolman/audit-fixture.json; the Rust test `spoolman::audit` replays the
   same requests against the hub and compares.

Exit 0 only when the fixture is written and every route is covered or listed.
"""

from __future__ import annotations

import http.client
import json
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlsplit, parse_qsl

SPOOLMAN_COMMIT = "eccb05e"
ROOT = Path(__file__).resolve().parent.parent
REF = ROOT / "reference" / "Spoolman"
VENV = ROOT / "reference" / ".venv-spoolman"
VENV_PY = VENV / "bin" / "python"
OUT = ROOT / "crates" / "hub" / "src" / "spoolman" / "audit-fixture.json"

# Headers the HTTP server (uvicorn / axum) sets, not Spoolman's code.
IGNORED_HEADERS = {
    "date": "volatile: the time of the answer",
    "server": "set by the HTTP server (uvicorn), not by Spoolman's code",
}

NORMALISATION = {
    "now": "a string that parses as an ISO 8601 date-time (`T` or space between date and time) "
           "within 300 s of the time the request was made becomes `<now>`; any other date-time "
           "stays as it is (the requests use fixed dates in 2024)",
    "paths": "in /info, `data_dir`, `logs_dir`, `backups_dir` become `<path>` (a temp dir); "
             "`latest_version` and `update_available` become `<volatile>` (Spoolman asks GitHub), and so does its `content-length`, which follows from them; "
             "in /backup, `path` becomes `<path>`",
    "headers": "in header values, `http://127.0.0.1:<port>` becomes `http://<host>` (the redirect's `location`)",
    "json": "JSON bodies are compared as values and by key order: of the keys two objects both have, the order must be the same (Spoolman writes its models' field order; the hub keeps the order it inserts, `preserve_order`, chosen in the hub's own `Cargo.toml` so the order does not depend on what else is in the build)",
    "text": "other bodies (CSV) are compared as text after the date-time rule",
}


# ---------------------------------------------------------------------------------------
# The request list. (name, method, path[, body]); a body is a JSON value, or ("raw", ctype, text).
# Ids are the ones a fresh Spoolman hands out in order, checked while recording (`expect_id`).
# ---------------------------------------------------------------------------------------
JSON = "application/json"


def raw(text, ctype=JSON):
    return ("raw", ctype, text)


def setting(value):
    """The body Spoolman's own client sends to POST /setting/{key}: the JSON text of the value,
    as a JSON string."""
    return json.dumps(json.dumps(value))


REQ: list[tuple] = []


def r(name, method, path, body=None):
    REQ.append((name, method, path, body))


V = "/api/v1"

# -- meta, empty database ------------------------------------------------------------
r("info", "GET", f"{V}/info")
r("health", "GET", f"{V}/health")
r("backup", "POST", f"{V}/backup")
r("empty vendor list", "GET", f"{V}/vendor")
r("empty filament list", "GET", f"{V}/filament")
r("empty spool list", "GET", f"{V}/spool")
r("empty spool group", "GET", f"{V}/spool/group?group_by=location")
r("empty material", "GET", f"{V}/material")
r("empty article-number", "GET", f"{V}/article-number")
r("empty lot-number", "GET", f"{V}/lot-number")
r("empty location", "GET", f"{V}/location")
r("empty search", "GET", f"{V}/search?q=x")
r("tag reader (none)", "GET", f"{V}/tag/reader")
for ent in ("vendor", "filament", "spool"):
    r(f"fields of {ent} (none)", "GET", f"{V}/field/{ent}")
r("field of unknown entity", "GET", f"{V}/field/bogus")
for ent in ("vendors", "filaments", "spools"):
    r(f"export {ent} csv (empty)", "GET", f"{V}/export/{ent}?fmt=csv")
    r(f"export {ent} json (empty)", "GET", f"{V}/export/{ent}?fmt=json")
r("export unknown format", "GET", f"{V}/export/vendors?fmt=xml")
r("export without format", "GET", f"{V}/export/vendors")
r("vendor 1 (none)", "GET", f"{V}/vendor/1")
r("filament 1 (none)", "GET", f"{V}/filament/1")
r("spool 1 (none)", "GET", f"{V}/spool/1")
r("vendor id not a number", "GET", f"{V}/vendor/abc")
r("unknown route", "GET", f"{V}/nonexistent")
r("trailing slash", "GET", f"{V}/vendor/")
r("wrong method", "DELETE", f"{V}/vendor")
r("wrong method on item", "POST", f"{V}/vendor/1")

# -- settings ------------------------------------------------------------------------
SETTING_KEYS = None  # filled from Spoolman's own settings.py before the list is used


def settings_requests(keys):
    r("settings, all, defaults", "GET", f"{V}/setting/")
    for k in keys:
        r(f"setting {k}, default", "GET", f"{V}/setting/{k}")
    r("setting unknown", "GET", f"{V}/setting/nokey")
    r("set setting currency", "POST", f"{V}/setting/currency", raw(setting("USD")))
    r("get setting currency, set", "GET", f"{V}/setting/currency")
    r("set setting round_prices", "POST", f"{V}/setting/round_prices", raw(setting(True)))
    r("set setting print_presets", "POST", f"{V}/setting/print_presets", raw(setting([{"a": 1}])))
    r("set setting dashboard_groups", "POST", f"{V}/setting/dashboard_groups", raw(setting({"location": ["A"]})))
    r("set setting wrong type", "POST", f"{V}/setting/currency", raw(setting(5)))
    r("set setting body not JSON text", "POST", f"{V}/setting/currency", raw('"USD"'))
    r("set setting unknown key", "POST", f"{V}/setting/nokey", raw(setting("x")))
    r("set setting wrong content type", "POST", f"{V}/setting/currency", raw(setting("EUR"), "text/plain"))
    r("set setting empty body", "POST", f"{V}/setting/currency", raw(""))
    r("set setting null resets", "POST", f"{V}/setting/currency", raw("null"))
    r("get setting currency, reset", "GET", f"{V}/setting/currency")
    r("set setting malformed extra field list", "POST", f"{V}/setting/extra_fields_spool", raw(setting([{"x": 1}])))
    r("settings, all, after", "GET", f"{V}/setting/")


# -- vendors (ids 1..4) ---------------------------------------------------------------
r("add vendor 1, name only", "POST", f"{V}/vendor", {"name": "Prusament"})
r("add vendor 2, all fields", "POST", f"{V}/vendor", {
    "name": "Polymaker", "comment": "PolyTerra and friends", "empty_spool_weight": 140,
    "external_id": "polymaker", "extra": {}})
r("add vendor 3, float weight", "POST", f"{V}/vendor", {"name": "eSun", "empty_spool_weight": 250.5})
r("add vendor 4, unknown field ignored", "POST", f"{V}/vendor", {"name": "Ignored", "bogus": 1})
r("add vendor without name", "POST", f"{V}/vendor", {})
r("add vendor, name too long", "POST", f"{V}/vendor", {"name": "x" * 65})
r("add vendor, negative weight", "POST", f"{V}/vendor", {"name": "Neg", "empty_spool_weight": -1})
r("add vendor, name a number", "POST", f"{V}/vendor", {"name": 5})
r("add vendor, extra not a map", "POST", f"{V}/vendor", {"name": "E", "extra": "x"})
r("add vendor, comment too long", "POST", f"{V}/vendor", {"name": "C", "comment": "c" * 1025})
r("add vendor, body not JSON", "POST", f"{V}/vendor", raw("notjson"))
r("add vendor, empty body", "POST", f"{V}/vendor", raw(""))
r("add vendor, unknown extra key", "POST", f"{V}/vendor", {"name": "X", "extra": {"nokey": "1"}})
r("get vendor 1", "GET", f"{V}/vendor/1")
r("get vendor 2", "GET", f"{V}/vendor/2")
r("get vendor 99", "GET", f"{V}/vendor/99")
r("patch vendor 1 comment", "PATCH", f"{V}/vendor/1", {"comment": "Czech"})
r("patch vendor 1 weight", "PATCH", f"{V}/vendor/1", {"empty_spool_weight": 193})
r("patch vendor 1 empty", "PATCH", f"{V}/vendor/1", {})
r("patch vendor 1 comment null", "PATCH", f"{V}/vendor/1", {"comment": None})
r("patch vendor 99", "PATCH", f"{V}/vendor/99", {"comment": "x"})
r("patch vendor 2 external_id", "PATCH", f"{V}/vendor/2", {"external_id": "poly"})
r("patch vendor 4 name", "PATCH", f"{V}/vendor/4", {"name": "Renamed"})
r("list vendors", "GET", f"{V}/vendor")
r("list vendors name part", "GET", f"{V}/vendor?name=pru")
r("list vendors name two terms", "GET", f"{V}/vendor?name=pru,poly")
r("list vendors name exact", "GET", f"{V}/vendor?name=%22Polymaker%22")
r("list vendors name exact, no match", "GET", f"{V}/vendor?name=%22poly%22")
r("list vendors external_id", "GET", f"{V}/vendor?external_id=poly")
r("list vendors external_id empty", "GET", f"{V}/vendor?external_id=")
r("list vendors sort name asc", "GET", f"{V}/vendor?sort=name:asc")
r("list vendors sort name desc", "GET", f"{V}/vendor?sort=name:desc")
r("list vendors sort two fields", "GET", f"{V}/vendor?sort=empty_spool_weight:desc,id:asc")
r("list vendors sort without direction", "GET", f"{V}/vendor?sort=name")
r("list vendors sort bad direction", "GET", f"{V}/vendor?sort=name:sideways")
r("list vendors sort unknown field", "GET", f"{V}/vendor?sort=bogus:asc")
r("list vendors limit", "GET", f"{V}/vendor?limit=2")
r("list vendors limit and offset", "GET", f"{V}/vendor?limit=2&offset=1")
r("list vendors offset only", "GET", f"{V}/vendor?offset=3")
r("list vendors limit 0", "GET", f"{V}/vendor?limit=0")
r("list vendors limit negative", "GET", f"{V}/vendor?limit=-1")
r("list vendors limit not a number", "GET", f"{V}/vendor?limit=abc")
r("list vendors offset negative", "GET", f"{V}/vendor?offset=-1")

# -- filaments (ids 1..4) --------------------------------------------------------------
r("add filament 1, minimal", "POST", f"{V}/filament", {"density": 1.24, "diameter": 1.75})
r("add filament 2, all fields", "POST", f"{V}/filament", {
    "name": "PETG Galaxy Black", "vendor_id": 1, "material": "PETG", "price": 29.9, "density": 1.27,
    "diameter": 1.75, "weight": 1000, "spool_weight": 193, "article_number": "PM1", "comment": "dark",
    "settings_extruder_temp": 250, "settings_bed_temp": 85, "color_hex": "#2B2B33",
    "external_id": "ext-1", "extra": {}})
r("add filament 3, alpha colour", "POST", f"{V}/filament", {
    "name": "PolyTerra Red", "vendor_id": 2, "material": "PLA", "density": 1.24, "diameter": 1.75,
    "weight": 750, "color_hex": "ff0000cc", "article_number": "PT2"})
r("add filament 4, multi colour", "POST", f"{V}/filament", {
    "name": "Rainbow", "vendor_id": 2, "material": "PLA", "density": 1.24, "diameter": 2.85,
    "weight": 500, "multi_color_hexes": "#FF0000,00FF00", "multi_color_direction": "coaxial"})
r("add filament without density", "POST", f"{V}/filament", {"diameter": 1.75})
r("add filament, density 0", "POST", f"{V}/filament", {"density": 0, "diameter": 1.75})
r("add filament, bad colour", "POST", f"{V}/filament", {"density": 1, "diameter": 1, "color_hex": "XYZ123"})
r("add filament, colour 5 long", "POST", f"{V}/filament", {"density": 1, "diameter": 1, "color_hex": "12345"})
r("add filament, both colours", "POST", f"{V}/filament", {
    "density": 1, "diameter": 1, "color_hex": "FF0000", "multi_color_hexes": "FF0000,00FF00",
    "multi_color_direction": "coaxial"})
r("add filament, multi without direction", "POST", f"{V}/filament", {
    "density": 1, "diameter": 1, "multi_color_hexes": "FF0000,00FF00"})
r("add filament, one multi colour", "POST", f"{V}/filament", {
    "density": 1, "diameter": 1, "multi_color_hexes": "FF0000", "multi_color_direction": "coaxial"})
r("add filament, direction without multi", "POST", f"{V}/filament", {
    "density": 1, "diameter": 1, "multi_color_direction": "coaxial"})
r("add filament, bad direction", "POST", f"{V}/filament", {
    "density": 1, "diameter": 1, "multi_color_hexes": "FF0000,00FF00", "multi_color_direction": "zig"})
r("add filament, vendor 99", "POST", f"{V}/filament", {"density": 1, "diameter": 1, "vendor_id": 99})
r("add filament, weight 0", "POST", f"{V}/filament", {"density": 1, "diameter": 1, "weight": 0})
r("add filament, name too long", "POST", f"{V}/filament", {"density": 1, "diameter": 1, "name": "x" * 65})
r("add filament, negative price", "POST", f"{V}/filament", {"density": 1, "diameter": 1, "price": -1})
r("get filament 1", "GET", f"{V}/filament/1")
r("get filament 2", "GET", f"{V}/filament/2")
r("get filament 4", "GET", f"{V}/filament/4")
r("get filament 99", "GET", f"{V}/filament/99")
r("patch filament 1 name", "PATCH", f"{V}/filament/1", {"name": "Plain", "material": "ABS"})
r("patch filament 1 vendor", "PATCH", f"{V}/filament/1", {"vendor_id": 3})
r("patch filament 1 colour", "PATCH", f"{V}/filament/1", {"color_hex": "00FF00"})
r("patch filament 1 colour to multi", "PATCH", f"{V}/filament/1", {
    "color_hex": None, "multi_color_hexes": "00FF00,0000FF", "multi_color_direction": "longitudinal"})
r("patch filament 1 colour null", "PATCH", f"{V}/filament/1", {
    "multi_color_hexes": None, "multi_color_direction": None})
r("patch filament 99", "PATCH", f"{V}/filament/99", {"name": "x"})
r("patch filament 1 empty", "PATCH", f"{V}/filament/1", {})
r("list filaments", "GET", f"{V}/filament")
r("list filaments vendor_name", "GET", f"{V}/filament?vendor_name=prus")
r("list filaments vendor_id", "GET", f"{V}/filament?vendor_id=2")
r("list filaments vendor_id two", "GET", f"{V}/filament?vendor_id=1,2")
r("list filaments vendor.name", "GET", f"{V}/filament?vendor.name=poly")
r("list filaments vendor.id", "GET", f"{V}/filament?vendor.id=1")
r("list filaments name", "GET", f"{V}/filament?name=petg")
r("list filaments material", "GET", f"{V}/filament?material=PLA")
r("list filaments material exact", "GET", f"{V}/filament?material=%22PL%22")
r("list filaments material two", "GET", f"{V}/filament?material=petg,pla")
r("list filaments article_number", "GET", f"{V}/filament?article_number=PM1")
r("list filaments color_hex", "GET", f"{V}/filament?color_hex=2B2B33")
r("list filaments color_hex similar", "GET", f"{V}/filament?color_hex=2B2B34&color_similarity_threshold=5")
r("list filaments color_hex far", "GET", f"{V}/filament?color_hex=FFFFFF&color_similarity_threshold=1")
r("list filaments external_id", "GET", f"{V}/filament?external_id=ext-1")
r("list filaments sort name", "GET", f"{V}/filament?sort=name:asc")
r("list filaments sort vendor.name desc", "GET", f"{V}/filament?sort=vendor.name:desc,id:asc")
r("list filaments sort weight", "GET", f"{V}/filament?sort=weight:desc")
r("list filaments sort bad", "GET", f"{V}/filament?sort=name")
r("list filaments limit offset", "GET", f"{V}/filament?limit=2&offset=1")
r("list filaments limit bad", "GET", f"{V}/filament?limit=x")
r("list filaments empty result", "GET", f"{V}/filament?name=zzz")

# -- spools (ids 1..5) -----------------------------------------------------------------
r("add spool 1, minimal", "POST", f"{V}/spool", {"filament_id": 2})
r("add spool 2, all fields", "POST", f"{V}/spool", {
    "filament_id": 2, "initial_weight": 1000, "spool_weight": 200, "used_weight": 100,
    "location": "Shelf A", "lot_nr": "L1", "comment": "first", "price": 20,
    "first_used": "2024-05-01T10:00:00Z", "last_used": "2024-05-02T10:00:00+02:00", "extra": {}})
r("add spool 3, remaining weight", "POST", f"{V}/spool", {
    "filament_id": 3, "remaining_weight": 120, "location": "Shelf B", "lot_nr": "L2"})
r("add spool 4, filament without weight", "POST", f"{V}/spool", {"filament_id": 1, "used_weight": 10})
r("add spool 5, archived", "POST", f"{V}/spool", {
    "filament_id": 3, "archived": True, "location": "Shelf A", "first_used": "2024-06-01T00:00:00",
    "last_used": "2024-06-02T00:00:00"})
r("add spool without filament", "POST", f"{V}/spool", {})
r("add spool, filament 99", "POST", f"{V}/spool", {"filament_id": 99})
r("add spool, remaining and used", "POST", f"{V}/spool", {
    "filament_id": 2, "remaining_weight": 1, "used_weight": 1})
r("add spool, remaining without filament weight", "POST", f"{V}/spool", {
    "filament_id": 1, "remaining_weight": 100})
r("add spool, negative weight", "POST", f"{V}/spool", {"filament_id": 2, "initial_weight": -1})
r("add spool, location too long", "POST", f"{V}/spool", {"filament_id": 2, "location": "x" * 65})
r("add spool, bad date", "POST", f"{V}/spool", {"filament_id": 2, "first_used": "yesterday"})
r("add spool, archived not a bool", "POST", f"{V}/spool", {"filament_id": 2, "archived": "maybe"})
r("add spool, unknown extra key", "POST", f"{V}/spool", {"filament_id": 2, "extra": {"nokey": "1"}})
r("get spool 1", "GET", f"{V}/spool/1")
r("get spool 2", "GET", f"{V}/spool/2")
r("get spool 3", "GET", f"{V}/spool/3")
r("get spool 4", "GET", f"{V}/spool/4")
r("get spool 5 (archived)", "GET", f"{V}/spool/5")
r("get spool 99", "GET", f"{V}/spool/99")
r("patch spool 1 location", "PATCH", f"{V}/spool/1", {"location": "Shelf A", "comment": "c"})
r("patch spool 1 used_weight", "PATCH", f"{V}/spool/1", {"used_weight": 50})
r("patch spool 1 remaining_weight", "PATCH", f"{V}/spool/1", {"remaining_weight": 900})
r("patch spool 1 filament", "PATCH", f"{V}/spool/1", {"filament_id": 3})
r("patch spool 1 filament back", "PATCH", f"{V}/spool/1", {"filament_id": 2})
r("patch spool 1 location null", "PATCH", f"{V}/spool/1", {"location": None})
r("patch spool 1 first_used", "PATCH", f"{V}/spool/1", {"first_used": "2024-03-01T08:00:00Z"})
r("patch spool 1 empty", "PATCH", f"{V}/spool/1", {})
r("patch spool 99", "PATCH", f"{V}/spool/99", {"comment": "x"})
r("use spool 1 weight", "PUT", f"{V}/spool/1/use", {"use_weight": 10})
r("use spool 1 length", "PUT", f"{V}/spool/1/use", {"use_length": 1000})
r("use spool 1 negative weight", "PUT", f"{V}/spool/1/use", {"use_weight": -5})
r("use spool 1 more than left", "PUT", f"{V}/spool/1/use", {"use_weight": 5000})
r("use spool 99", "PUT", f"{V}/spool/99/use", {"use_weight": 1})
r("use spool 4 length", "PUT", f"{V}/spool/4/use", {"use_length": 100})
r("use spool 3 weight", "PUT", f"{V}/spool/3/use", {"use_weight": 20.5})
r("measure spool 2", "PUT", f"{V}/spool/2/measure", {"weight": 800})
r("measure spool 2 below tare", "PUT", f"{V}/spool/2/measure", {"weight": 100})
r("measure spool 99", "PUT", f"{V}/spool/99/measure", {"weight": 800})
r("measure spool 3", "PUT", f"{V}/spool/3/measure", {"weight": 500})
r("list spools", "GET", f"{V}/spool")
r("list spools allow_archived", "GET", f"{V}/spool?allow_archived=true")
r("list spools allow_archived bad", "GET", f"{V}/spool?allow_archived=maybe")
r("list spools filament_name", "GET", f"{V}/spool?filament_name=petg")
r("list spools filament_id", "GET", f"{V}/spool?filament_id=3&allow_archived=true")
r("list spools filament_material", "GET", f"{V}/spool?filament_material=PLA")
r("list spools vendor_name", "GET", f"{V}/spool?vendor_name=prus")
r("list spools vendor_id", "GET", f"{V}/spool?vendor_id=2")
r("list spools filament.name", "GET", f"{V}/spool?filament.name=red")
r("list spools filament.id", "GET", f"{V}/spool?filament.id=2")
r("list spools filament.material", "GET", f"{V}/spool?filament.material=petg")
r("list spools filament.multi_color_direction", "GET", f"{V}/spool?filament.multi_color_direction=coaxial")
r("list spools filament.vendor.name", "GET", f"{V}/spool?filament.vendor.name=prus&sort=id:desc")
r("list spools filament.vendor.id", "GET", f"{V}/spool?filament.vendor.id=1")
r("list spools location", "GET", f"{V}/spool?location=Shelf%20A&allow_archived=true")
r("list spools location exact", "GET", f"{V}/spool?location=%22Shelf%20A%22")
r("list spools location empty", "GET", f"{V}/spool?location=")
r("list spools lot_nr", "GET", f"{V}/spool?lot_nr=L1")
r("list spools first_used range", "GET", f"{V}/spool?first_used=2024-04-01T00:00:00Z%7C2024-05-15T00:00:00Z")
r("list spools first_used open end", "GET", f"{V}/spool?first_used=2024-04-01T00:00:00Z%7C&allow_archived=true")
r("list spools first_used empty", "GET", f"{V}/spool?first_used=")
r("list spools last_used", "GET", f"{V}/spool?last_used=%7C2024-05-31T00:00:00Z")
r("list spools registered", "GET", f"{V}/spool?registered=2000-01-01T00:00:00Z%7C")
r("list spools first_used bad", "GET", f"{V}/spool?first_used=notadate")
r("list spools sort remaining_weight", "GET", f"{V}/spool?sort=remaining_weight:desc")
r("list spools sort used_weight", "GET", f"{V}/spool?sort=used_weight:asc,id:desc")
r("list spools sort location", "GET", f"{V}/spool?sort=location:asc&allow_archived=true")
r("list spools sort filament.name", "GET", f"{V}/spool?sort=filament.name:asc,id:asc")
r("list spools sort filament.vendor.name", "GET", f"{V}/spool?sort=filament.vendor.name:desc,id:asc")
r("list spools sort price", "GET", f"{V}/spool?sort=price:desc")
r("list spools sort first_used", "GET", f"{V}/spool?sort=first_used:desc&allow_archived=true")
r("list spools sort bad", "GET", f"{V}/spool?sort=id")
r("list spools limit offset", "GET", f"{V}/spool?allow_archived=true&limit=2&offset=1")
r("list spools limit bad", "GET", f"{V}/spool?limit=x")
r("spool group by location", "GET", f"{V}/spool/group?group_by=location&allow_archived=true")
r("spool group by material", "GET", f"{V}/spool/group?group_by=material")
r("spool group, every filter", "GET", f"{V}/spool/group?group_by=location&filament.name=p&filament.id=2&filament.material=PETG&filament.multi_color_direction=coaxial&filament.vendor.name=p&filament.vendor.id=1&location=Shelf&lot_nr=L&allow_archived=true&include_empty=true&preview=true&first_used=%7C&last_used=%7C&registered=%7C&sort=location:asc&limit=5&offset=0")
r("spool group by bad", "GET", f"{V}/spool/group?group_by=bogus")
r("spool group without group_by", "GET", f"{V}/spool/group")
r("rename spool field value", "PATCH", f"{V}/spool/field/location", {"value": "Shelf B", "new_value": "Shelf C"})
r("rename spool field, missing body", "PATCH", f"{V}/spool/field/location", {})
r("rename spool field, bad field", "PATCH", f"{V}/spool/field/bogus", {"value": "a", "new_value": "b"})

# -- distinct values and locations -------------------------------------------------------
r("materials", "GET", f"{V}/material")
r("article numbers", "GET", f"{V}/article-number")
r("lot numbers", "GET", f"{V}/lot-number")
r("locations", "GET", f"{V}/location")
r("rename location", "PATCH", f"{V}/location/Shelf%20A", {"name": "Shelf Z"})
r("rename location with slash", "PATCH", f"{V}/location/Rack/1", {"name": "Rack 2"})
r("rename location, empty name", "PATCH", f"{V}/location/Shelf%20Z", {"name": ""})
r("rename location, no body", "PATCH", f"{V}/location/Shelf%20Z", {})
r("rename location, unknown", "PATCH", f"{V}/location/Nowhere", {"name": "Somewhere"})
r("locations after rename", "GET", f"{V}/location")
r("spools after rename", "GET", f"{V}/spool?allow_archived=true")

# -- extra fields ---------------------------------------------------------------------------
r("define spool field, datetime", "POST", f"{V}/field/spool/last_dried", {"name": "Last dried", "field_type": "datetime"})
r("define spool field, choice", "POST", f"{V}/field/spool/dryness", {
    "name": "Dryness", "field_type": "choice", "multi_choice": False, "choices": ["dry", "damp"], "default_value": "\"dry\""})
r("define spool field, text", "POST", f"{V}/field/spool/shelf", {"name": "Shelf", "field_type": "text", "order": 3})
r("define spool field, integer with unit", "POST", f"{V}/field/spool/dried_hours", {
    "name": "Dried", "unit": "h", "field_type": "integer"})
r("define spool field, boolean", "POST", f"{V}/field/spool/opened", {"name": "Opened", "field_type": "boolean"})
r("define filament field", "POST", f"{V}/field/filament/finish", {"name": "Finish", "field_type": "text"})
r("define vendor field", "POST", f"{V}/field/vendor/country", {"name": "Country", "field_type": "text"})
r("define field, type change", "POST", f"{V}/field/spool/shelf", {"name": "Shelf", "field_type": "integer"})
r("define field, name change", "POST", f"{V}/field/spool/shelf", {"name": "Shelf no.", "field_type": "text"})
r("define field, bad type", "POST", f"{V}/field/spool/bad", {"name": "Bad", "field_type": "colour"})
r("define field, bad key", "POST", f"{V}/field/spool/Bad-Key", {"name": "Bad", "field_type": "text"})
r("define field, choice without choices", "POST", f"{V}/field/spool/nochoice", {"name": "N", "field_type": "choice"})
r("define field, bad entity", "POST", f"{V}/field/bogus/x", {"name": "X", "field_type": "text"})
r("define field, no body", "POST", f"{V}/field/spool/x", {})
r("fields of spool", "GET", f"{V}/field/spool")
r("fields of filament", "GET", f"{V}/field/filament")
r("fields of vendor", "GET", f"{V}/field/vendor")
r("add spool 6, with extra", "POST", f"{V}/spool", {
    "filament_id": 2, "extra": {"last_dried": "\"2024-05-01T10:00:00Z\"", "dryness": "\"damp\"", "shelf": "\"S1\""}})
r("add spool, extra value of the wrong type", "POST", f"{V}/spool", {
    "filament_id": 2, "extra": {"dried_hours": "\"abc\""}})
r("add spool, extra not a choice", "POST", f"{V}/spool", {"filament_id": 2, "extra": {"dryness": "\"soggy\""}})
r("add spool, extra not JSON text", "POST", f"{V}/spool", {"filament_id": 2, "extra": {"shelf": "S1"}})
r("patch spool 6 extra", "PATCH", f"{V}/spool/6", {"extra": {"shelf": "\"S2\"", "opened": "true"}})
r("patch spool 6 extra again", "PATCH", f"{V}/spool/6", {"extra": {"dried_hours": "12"}})
r("patch filament 2 extra", "PATCH", f"{V}/filament/2", {"extra": {"finish": "\"matte\""}})
r("patch vendor 1 extra", "PATCH", f"{V}/vendor/1", {"extra": {"country": "\"CZ\""}})
r("get spool 6", "GET", f"{V}/spool/6")
r("get filament 2 with extra", "GET", f"{V}/filament/2")
r("get vendor 1 with extra", "GET", f"{V}/vendor/1")
r("list spools by extra", "GET", f"{V}/spool?extra.shelf=S2")
r("list spools by extra datetime", "GET", f"{V}/spool?extra.dried_hours=12")
r("list filaments by extra", "GET", f"{V}/filament?extra.finish=matte")
r("list vendors by extra", "GET", f"{V}/vendor?extra.country=CZ")
r("field values", "GET", f"{V}/field/spool/shelf/values")
r("field values unknown key", "GET", f"{V}/field/spool/nokey/values")
r("delete field", "DELETE", f"{V}/field/spool/opened")
r("delete field again", "DELETE", f"{V}/field/spool/opened")
r("fields of spool after delete", "GET", f"{V}/field/spool")
r("rename spool field value by extra", "PATCH", f"{V}/spool/field/extra.shelf", {"value": "S2", "new_value": "S3"})

# -- tags ------------------------------------------------------------------------------------
r("link tag to spool 2", "POST", f"{V}/spool/2/tag", {"uid": "04:a2:b3:c4", "format": "ntag"})
r("link same tag again", "POST", f"{V}/spool/2/tag", {"uid": "04a2b3c4"})
r("link tag held by a spool to a filament", "POST", f"{V}/filament/2/tag", {"uid": "04-A2-B3-C4"})
r("link tag to spool 3, held", "POST", f"{V}/spool/3/tag", {"uid": "04A2B3C4"})
r("link tag to filament 2", "POST", f"{V}/filament/2/tag", {"uid": "AA:BB:CC:DD", "format": "tag"})
r("link tag, not hex", "POST", f"{V}/spool/2/tag", {"uid": "not hex"})
r("link tag, empty", "POST", f"{V}/spool/2/tag", {"uid": ""})
r("link tag, too long", "POST", f"{V}/spool/2/tag", {"uid": "AB" * 40})
r("link tag to spool 99", "POST", f"{V}/spool/99/tag", {"uid": "01020304"})
r("link tag to filament 99", "POST", f"{V}/filament/99/tag", {"uid": "01020304"})
r("link tag, no body", "POST", f"{V}/spool/2/tag", {})
r("get spool 2 with tag", "GET", f"{V}/spool/2")
r("get filament 2 with tag", "GET", f"{V}/filament/2")
r("list spools by tag", "GET", f"{V}/spool?tag=04a2b3c4")
r("list filaments by tag", "GET", f"{V}/filament?tag=AABBCCDD")
r("list spools by unknown tag", "GET", f"{V}/spool?tag=FFFFFFFF")
r("scan tag of a spool", "POST", f"{V}/tag/scan", {"uid": "04 a2 b3 c4", "reader_id": "scale"})
r("scan tag of a filament", "POST", f"{V}/tag/scan", {"uid": "AABBCCDD", "reader_id": "scale", "name": "Scale"})
r("scan unknown tag", "POST", f"{V}/tag/scan", {"uid": "11223344", "reader_id": "door"})
r("scan, bad reader id", "POST", f"{V}/tag/scan", {"uid": "11223344", "reader_id": "bad reader!"})
r("scan, no uid", "POST", f"{V}/tag/scan", {})
r("scan, not hex", "POST", f"{V}/tag/scan", {"uid": "zz"})
r("tag readers", "GET", f"{V}/tag/reader")
r("unlink tag from spool 2", "DELETE", f"{V}/spool/2/tag/04:A2:B3:C4")
r("unlink tag again", "DELETE", f"{V}/spool/2/tag/04A2B3C4")
r("unlink tag, not hex", "DELETE", f"{V}/spool/2/tag/zz")
r("unlink tag from spool 99", "DELETE", f"{V}/spool/99/tag/04A2B3C4")
r("unlink tag from filament 2", "DELETE", f"{V}/filament/2/tag/AABBCCDD")
r("unlink tag from filament 99", "DELETE", f"{V}/filament/99/tag/AABBCCDD")

# -- search and export ------------------------------------------------------------------------------
r("search text", "GET", f"{V}/search?q=petg")
r("search two terms", "GET", f"{V}/search?q=polymaker%20red")
r("search number", "GET", f"{V}/search?q=3")
r("search colour", "GET", f"{V}/search?q=%23ff0000")
r("search colour threshold", "GET", f"{V}/search?q=red&color_similarity_threshold=50")
r("search limit and spools per filament", "GET", f"{V}/search?q=pla&limit=1&spools_per_filament=2")
r("search archived", "GET", f"{V}/search?q=polyterra&allow_archived=true")
r("search without q", "GET", f"{V}/search")
r("search limit too big", "GET", f"{V}/search?q=a&limit=1000")
for ent in ("vendors", "filaments", "spools"):
    r(f"export {ent} csv", "GET", f"{V}/export/{ent}?fmt=csv")
    r(f"export {ent} json", "GET", f"{V}/export/{ent}?fmt=json")
r("export spools with archived", "GET", f"{V}/export/spools?fmt=json&allow_archived=true")

# -- refused changes, on spare items (so a hub that accepts what Spoolman refuses cannot change
#    the items the rest of the list depends on) --------------------------------------------
r("add vendor 5, spare", "POST", f"{V}/vendor", {"name": "Spare"})
r("add filament 5, spare", "POST", f"{V}/filament", {"density": 1.24, "diameter": 1.75})
r("add spool 7, spare", "POST", f"{V}/spool", {"filament_id": 5})
r("patch vendor name null", "PATCH", f"{V}/vendor/5", {"name": None})
r("patch vendor name too long", "PATCH", f"{V}/vendor/5", {"name": "x" * 65})
r("patch filament density null", "PATCH", f"{V}/filament/5", {"density": None})
r("patch filament vendor 99", "PATCH", f"{V}/filament/5", {"vendor_id": 99})
r("patch spool both weights", "PATCH", f"{V}/spool/7", {"remaining_weight": 1, "used_weight": 1})
r("patch spool filament null", "PATCH", f"{V}/spool/7", {"filament_id": None})
r("patch spool filament 99", "PATCH", f"{V}/spool/7", {"filament_id": 99})
r("use spool both", "PUT", f"{V}/spool/7/use", {"use_weight": 1, "use_length": 1})
r("use spool neither", "PUT", f"{V}/spool/7/use", {})
r("use spool not a number", "PUT", f"{V}/spool/7/use", {"use_weight": "a"})
r("measure spool without weight", "PUT", f"{V}/spool/7/measure", {})
r("measure spool without initial weight", "PUT", f"{V}/spool/7/measure", {"weight": 800})
r("spare spool 7 after", "GET", f"{V}/spool/7")
r("spare filament 5 after", "GET", f"{V}/filament/5")
r("spare vendor 5 after", "GET", f"{V}/vendor/5")
r("delete spare spool 7", "DELETE", f"{V}/spool/7")
r("delete spare filament 5", "DELETE", f"{V}/filament/5")
r("delete spare vendor 5", "DELETE", f"{V}/vendor/5")

# -- deleting ------------------------------------------------------------------------------------------
r("delete filament with spools", "DELETE", f"{V}/filament/2")
r("delete vendor with filaments", "DELETE", f"{V}/vendor/1")
r("delete spool 5", "DELETE", f"{V}/spool/5")
r("delete spool 5 again", "DELETE", f"{V}/spool/5")
r("get spool 5 after delete", "GET", f"{V}/spool/5")
r("delete spool 99", "DELETE", f"{V}/spool/99")
r("delete spool id not a number", "DELETE", f"{V}/spool/abc")
for i in (1, 2, 3, 4, 6):
    r(f"delete spool {i}", "DELETE", f"{V}/spool/{i}")
r("list spools after delete", "GET", f"{V}/spool?allow_archived=true")
r("delete filament 99", "DELETE", f"{V}/filament/99")
for i in (1, 2, 3, 4):
    r(f"delete filament {i}", "DELETE", f"{V}/filament/{i}")
r("delete vendor 99", "DELETE", f"{V}/vendor/99")
for i in (1, 2, 3, 4):
    r(f"delete vendor {i}", "DELETE", f"{V}/vendor/{i}")
r("vendors after delete", "GET", f"{V}/vendor")
r("locations after delete", "GET", f"{V}/location")

# ---------------------------------------------------------------------------------------
# Routes and query parameters that have no request, and why.
# ---------------------------------------------------------------------------------------
NOT_COVERED = {
    ("GET", "/external/filament"): "SpoolmanDB import: the catalogue is fetched from the internet (9745 filaments)",
    ("GET", "/external/filament/search"): "SpoolmanDB import: the catalogue is fetched from the internet",
    ("GET", "/external/material"): "SpoolmanDB import: the catalogue is fetched from the internet",
}
# Spoolman's own answers that are crashes, left out: `PATCH /spool/{id}` with `"extra": null`
# is a 500 `Internal Server Error` in Spoolman at this commit.
NOT_COVERED_PARAMS: dict[tuple[str, str, str], str] = {}

# ---------------------------------------------------------------------------------------

_ISO = re.compile(r"\d{4}-\d\d-\d\d[T ]\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)?")


def parse_ts(s):
    t = s.replace(" ", "T").replace("Z", "+00:00")
    d = datetime.fromisoformat(t)
    if d.tzinfo is None:
        d = d.replace(tzinfo=timezone.utc)
    return d.timestamp()


def norm_ts(s, now):
    try:
        return "<now>" if abs(parse_ts(s) - now) <= 300 else s
    except ValueError:
        return s


def norm_value(v, now):
    if isinstance(v, dict):
        return {k: norm_value(x, now) for k, x in v.items()}
    if isinstance(v, list):
        return [norm_value(x, now) for x in v]
    if isinstance(v, str) and _ISO.fullmatch(v):
        return norm_ts(v, now)
    return v


def norm_text(t, now):
    return _ISO.sub(lambda m: norm_ts(m.group(0), now), t)


def norm_special(path, body):
    p = urlsplit(path).path
    if p == "/api/v1/info" and isinstance(body, dict):
        for k in ("data_dir", "logs_dir", "backups_dir"):
            body[k] = "<path>"
        for k in ("latest_version", "update_available"):
            body[k] = "<volatile>"
    if p == "/api/v1/backup" and isinstance(body, dict) and "path" in body:
        body["path"] = "<path>"
    return body


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def venv_json(code, cwd):
    out = subprocess.run([str(VENV_PY), "-I", "-c", code], cwd=cwd, check=True, capture_output=True, text=True,
                         env={"PATH": f"{VENV}/bin:{os.environ['PATH']}", "SPOOLMAN_DIR_DATA": str(cwd)})
    return json.loads(out.stdout)


def route_regex(template):
    return re.compile("^" + re.sub(r"\{[^}]+:path\}", ".+", re.sub(r"\{[^}]+\}", "[^/]+", template)) + "$")


def main():
    if not (REF / "spoolman").is_dir() or not VENV_PY.exists():
        sys.exit("reference/Spoolman or reference/.venv-spoolman missing (uv sync --frozen --no-dev there)")
    commit = subprocess.run(["git", "-C", str(REF), "rev-parse", "--short=7", "HEAD"], capture_output=True,
                            text=True).stdout.strip()
    if commit != SPOOLMAN_COMMIT:
        sys.exit(f"reference/Spoolman is at {commit}, the audit is for {SPOOLMAN_COMMIT}")
    # the venv matches uv.lock (no change when it does)
    subprocess.run(["uv", "sync", "--frozen", "--no-dev", "--no-install-project", "--project", str(REF)],
                   check=True, env={**os.environ, "UV_PROJECT_ENVIRONMENT": str(VENV)},
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    # A fixed path, never $TMPDIR or a random name: Spoolman's /info answer holds this path, and its length decides
    # whether the answer passes 500 bytes, where Spoolman's gzip middleware starts (and adds
    # `vary: Accept-Encoding`). A $TMPDIR of another length made the recording differ per machine.
    work = Path("/tmp/maker-connect-spoolman-audit")  # one fixed path: the same length every run
    shutil.rmtree(work, ignore_errors=True)
    work.mkdir()
    (work / "cwd" / "client_v2" / "build").mkdir(parents=True)
    (work / "cwd" / "client_v2" / "build" / "200.html").write_text("x")
    shutil.copy(REF / "pyproject.toml", work / "cwd" / "pyproject.toml")
    (work / "data").mkdir()
    sys.path.insert(0, str(REF))
    # The routes and the settings, from Spoolman's own code.
    spec = venv_json(
        f"import sys,json;sys.path.insert(0,{str(REF)!r});"
        "from spoolman.api.v1.router import app;from spoolman.settings import SETTINGS;"
        "o=app.openapi();"
        "print(json.dumps({'ops':[[m.upper(),p,[x['name'] for x in d.get('parameters',[]) if x['in']=='query']]"
        " for p,v in o['paths'].items() for m,d in v.items()],'settings':list(SETTINGS)}))", work / "data")
    settings_requests(spec["settings"])
    # Sort the settings block into its place: after the meta requests, before the vendors.
    idx = next(i for i, q in enumerate(REQ) if q[0] == "add vendor 1, name only")
    cut = next(i for i, q in enumerate(REQ) if q[0] == "settings, all, defaults")
    block = REQ[cut:]
    del REQ[cut:]
    REQ[idx:idx] = block

    # Coverage, mechanically: every route, every query parameter.
    hit_ops: dict[tuple[str, str], set] = {}
    for name, method, path, body in REQ:
        p = urlsplit(path)
        tpath = p.path.removeprefix("/api/v1")
        for m, t, qs in spec["ops"]:
            if m == method and route_regex(t).match(tpath) or (m == method and t == "/setting/" and tpath == "/setting/"):
                hit_ops.setdefault((m, t), set()).update(k for k, _ in parse_qsl(p.query, keep_blank_values=True))
    problems = []
    for m, t, qs in spec["ops"]:
        if (m, t) not in hit_ops:
            if (m, t) not in NOT_COVERED:
                problems.append(f"no request for {m} {t}")
            continue
        for q in qs:
            if q not in hit_ops[(m, t)] and (m, t, q) not in NOT_COVERED_PARAMS:
                problems.append(f"no request uses query parameter {q} of {m} {t}")
    if problems:
        sys.exit("coverage:\n  " + "\n  ".join(problems))

    port = free_port()
    log = open(work / "spoolman.log", "w")
    env = {**os.environ, "PATH": f"{VENV}/bin:{os.environ['PATH']}", "SPOOLMAN_DIR_DATA": str(work / "data"),
           "SPOOLMAN_DIR_LOGS": str(work / "data"), "SPOOLMAN_DIR_BACKUPS": str(work / "data"),
           "SPOOLMAN_AUTOMATIC_BACKUP": "FALSE", "SPOOLMAN_CHECK_FOR_UPDATES": "FALSE"}
    proc = subprocess.Popen([str(VENV_PY), "-I", "-m", "uvicorn", "spoolman.main:app", "--app-dir", str(REF),
                             "--host", "127.0.0.1", "--port", str(port)],
                            cwd=work / "cwd", env=env, stdout=log, stderr=subprocess.STDOUT,
                            start_new_session=True)
    try:
        for _ in range(120):
            try:
                c = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
                c.request("GET", "/api/v1/health")
                if c.getresponse().status == 200:
                    break
            except OSError:
                time.sleep(0.5)
        else:
            sys.exit("Spoolman did not start; see " + str(work / "spoolman.log"))

        recorded = []
        for name, method, path, body in REQ:
            headers, data = {}, None
            if isinstance(body, tuple):
                headers["Content-Type"] = body[1]
                data = body[2].encode()
            elif body is not None:
                headers["Content-Type"] = JSON
                data = json.dumps(body).encode()
            now = time.time()
            c = http.client.HTTPConnection("127.0.0.1", port, timeout=60)
            c.request(method, path, body=data, headers=headers)
            resp = c.getresponse()
            payload = resp.read()
            hdrs = {k.lower(): re.sub(r"http://127\.0\.0\.1:\d+", "http://<host>", v) for k, v in resp.getheaders() if k.lower() not in IGNORED_HEADERS}
            ctype = hdrs.get("content-type", "")
            entry = {"name": name, "method": method, "path": path}
            if isinstance(body, tuple):
                entry["raw_body"] = {"content_type": body[1], "text": body[2]}
            elif body is not None:
                entry["body"] = body
            entry["status"] = resp.status
            if urlsplit(path).path == "/api/v1/info" and "content-length" in hdrs:
                hdrs["content-length"] = "<volatile>"  # see NORMALISATION["paths"]
            entry["headers"] = hdrs
            text = payload.decode("utf-8", "replace")
            if "json" in ctype:
                entry["response_json"] = norm_value(norm_special(path, json.loads(text)), now)
            elif text:
                entry["response_text"] = norm_text(text, now)
            recorded.append(entry)
        # The ids the requests assume.
        creations = [e for e in recorded if e["method"] == "POST" and e["status"] == 200
                     and isinstance(e.get("response_json"), dict) and "id" in e["response_json"]]
        expect = {"vendor": 0, "filament": 0, "spool": 0}
        for e in creations:
            kind = e["path"].split("/")[3]
            expect[kind] += 1
            if e["response_json"]["id"] != expect[kind]:
                sys.exit(f"{e['name']}: Spoolman gave id {e['response_json']['id']}, the list assumes {expect[kind]}")
        info = next(e for e in recorded if e["name"] == "info")["response_json"]
        pkgs = venv_json(
            "import json,importlib.metadata as m,sys;"
            "print(json.dumps({'python':sys.version.split()[0],**{n:m.version(n) for n in "
            "['fastapi','starlette','uvicorn','pydantic','SQLAlchemy','aiosqlite','alembic']}}))", work / "data")
        fixture = {
            "reference": {"project": "Spoolman (Donkie/Spoolman, MIT)", "commit": SPOOLMAN_COMMIT,
                          "version": info["version"], "database": "SQLite", "runtime": pkgs,
                          "cwd_note": "run with a temp cwd holding pyproject.toml and an empty client_v2/build"},
            "ignored_headers": IGNORED_HEADERS,
            "normalisation": NORMALISATION,
            "not_covered_routes": {f"{m} {t}": why for (m, t), why in NOT_COVERED.items()},
            "requests": recorded,
        }
        OUT.parent.mkdir(parents=True, exist_ok=True)
        OUT.write_text(json.dumps(fixture, indent=1, ensure_ascii=False) + "\n")
        print(f"{len(recorded)} requests recorded from Spoolman {info['version']} ({SPOOLMAN_COMMIT}) -> {OUT}")
        print(f"{len(spec['ops'])} routes in its OpenAPI, {len(NOT_COVERED)} not covered")
    finally:
        try:
            os.killpg(proc.pid, signal.SIGTERM)
            proc.wait(timeout=20)
        except Exception:
            os.killpg(proc.pid, signal.SIGKILL)
        shutil.rmtree(work, ignore_errors=True)


DIFFS = ROOT / "crates" / "hub" / "src" / "spoolman" / "audit-differences.txt"
DOC = ROOT / "docs" / "audits" / "spoolman.md"


def read_differences():
    """The Rust test's measured list: {number: (head, [lines])} and its first line."""
    text = DIFFS.read_text().split("\n")
    blocks, cur = {}, None
    for line in text[1:]:
        m = re.match(r"^(\d{3}) ", line)
        if m:
            cur = int(m.group(1))
            blocks[cur] = (line, [])
        elif cur is not None and line.strip():
            blocks[cur][1].append(line.strip())
    return text[0], blocks


def brief(entry):
    """What Spoolman answered, in a few words."""
    st = entry["status"]
    if "response_json" in entry:
        j = entry["response_json"]
        if isinstance(j, list):
            what = f"list of {len(j)}"
        elif isinstance(j, dict) and set(j) == {"message"}:
            what = "`" + j["message"][:60].replace("|", "/") + "`"
        elif isinstance(j, dict) and "detail" in j:
            what = "validation error"
        elif isinstance(j, dict):
            what = "object"
        else:
            what = "JSON"
    elif "response_text" in entry:
        what = "text, %d lines" % len(entry["response_text"].splitlines())
    else:
        what = "no body"
    total = entry["headers"].get("x-total-count")
    return f"{st} {what}" + (f", x-total-count {total}" if total is not None else "")


CLASSES = [
    ("Spoolman answers, the hub has no such route or setting (404, 403, 501)",
     lambda L: any(re.match(r"status: Spoolman 200, ours (404|403|501)", x) for x in L)),
    ("Spoolman refuses (400, 404, 415, 422), the hub accepts and does it (200)",
     lambda L: any(re.match(r"status: Spoolman (400|404|415|422), ours 200", x) for x in L)),
    ("Both refuse, with another status",
     lambda L: any(re.match(r"status: Spoolman (400|404|405|422|307), ours (400|403|404|422|500)", x) for x in L)),
    ("JSON key order differs (the hub inserts its fields in its own order)",
     lambda L: any("key order differs" in x for x in L)),
    ("The hub sends null fields where Spoolman leaves them out",
     lambda L: any(x.startswith("body: ours sends") for x in L)),
    ("Colour hex in lower case (`2b2b33`) where Spoolman keeps the case it was given",
     lambda L: any(re.search(r"color_hex: Spoolman \"[0-9A-F]+\", ours \"[0-9a-f]+\"", x) for x in L)),
    ("A filament's `spool_weight` that Spoolman takes from its vendor's `empty_spool_weight` is null in the hub",
     lambda L: any(re.search(r"spool_weight: Spoolman [0-9.]+, ours null", x) for x in L)),
    ("`vary: Accept-Encoding` (Spoolman's gzip layer) is not sent by the hub",
     lambda L: any(x.startswith("header vary") for x in L)),
    ("Error bodies are plain text in the hub, JSON (`message` or `detail`) in Spoolman",
     lambda L: any(x.startswith("header content-type") for x in L)),
    ("`x-total-count` differs (the list itself differs)",
     lambda L: any(x.startswith("header x-total-count") for x in L)),
    ("Spoolman's extra answer headers: `x-total-spools`, `x-total-remaining-weight`, `content-disposition`, `location`",
     lambda L: any(re.match(r"header (x-total-spools|x-total-remaining-weight|content-disposition|location)", x)
                   for x in L)),
]


def write_doc():
    fixture = json.loads(OUT.read_text())
    summary, blocks = read_differences()
    reqs = fixture["requests"]
    ref = fixture["reference"]
    rt = ref["runtime"]
    total = len(reqs)
    identical = sum(1 for n in range(1, total + 1) if blocks[n][1] == ["identical"])
    differ = total - identical
    assert summary.startswith(f"{identical} of {total} identical"), (summary, identical, total)
    out = []
    w = out.append
    w("# Spoolman API: the hub's `/api/v1` against Spoolman\n")
    w(f"**The one reference:** Spoolman (github.com/Donkie/Spoolman, MIT) at commit `{ref['commit']}`, "
      f"version {ref['version']}, in `reference/Spoolman`, run as it runs from source: uvicorn "
      f"{rt['uvicorn']}, FastAPI {rt['fastapi']}, Starlette {rt['starlette']}, Pydantic {rt['pydantic']}, "
      f"SQLAlchemy {rt['SQLAlchemy']}, aiosqlite {rt['aiosqlite']}, Alembic {rt['alembic']} (the versions of "
      f"its own `uv.lock`, in `reference/.venv-spoolman`), CPython {rt['python']}, on SQLite. "
      "Ours: `crates/hub/src/spoolman.rs`. The owner decided the hub is API- and data-compatible with "
      "Spoolman, not a copy: the API and the data are Spoolman's, the user interface is ours.\n")
    w("Proof: `scripts/spoolman-audit-check.py` starts real Spoolman on a fresh SQLite database on a free "
      "localhost port, sends it the request list below in order (`http.client`, no redirects followed, "
      "no gzip asked for) and writes status, headers and body of every answer to "
      "`crates/hub/src/spoolman/audit-fixture.json`. The Rust test `spoolman::audit` "
      "(`crates/hub/src/spoolman/audit.rs`) sends the same requests in the same order to the hub on a "
      "fresh store (signed in as the other hub tests are) and compares status, headers and body with "
      "the same normalisation. The measured differences are written in "
      "`crates/hub/src/spoolman/audit-differences.txt`; the test fails when they are not exactly that "
      "list, and this page is written from it (`scripts/spoolman-audit-check.py --doc`). The list is a "
      "snapshot of what the hub answers, written by the test from the hub's output, not judged by hand; "
      "it is only kept equal to the measurement.\n")
    w("The routes and the query parameters are read from Spoolman's own OpenAPI "
      f"(`spoolman.api.v1.router.app.openapi()`): the script stops when a route or a query parameter has "
      f"no request and no entry in its not-covered list. It covers {47 - len(fixture['not_covered_routes'])} "
      f"of Spoolman's 47 routes.\n")
    w(f"## Result\n\n**{identical} of {total} requests identical; {differ} differ.** "
      "The hub's answers are not Spoolman's yet: the classes of difference, with the number of requests "
      "each touches (a request can be in several):\n")
    w("| Class | Requests |\n|---|---|")
    for title, pred in CLASSES:
        n = sum(1 for k in range(1, total + 1) if pred(blocks[k][1]))
        w(f"| {title} | {n} |")
    w("")
    w("## Normalisation and what is not compared\n")
    for k, v in fixture["normalisation"].items():
        w(f"- **{k}**: {v}.")
    for h, why in fixture["ignored_headers"].items():
        w(f"- Header `{h}` is not compared: {why}.")
    w("- A vendor, filament or spool that the hub made for a `POST` that Spoolman refused is deleted through "
      "the hub's own API before the next request, so one difference does not shift the ids of the rest; "
      "the request is marked in the table. A location the hub made by name (a spool's `location`) is not "
      "deleted: it stays in the hub's list of places.")
    w("- Nothing is cut from the differences: every difference of every request is in the table. Equal "
      "differences in the items of one list are one line with `(xN)`, and the null fields are one line. "
      "Only the shown values are shortened (70 characters, 150 for a whole body; the comparison is on "
      "the full values), and the Spoolman column's message is shortened to 60.")
    w("- Requests that Spoolman refuses and that change something if accepted (`PATCH`, `PUT`) are sent "
      "last, to spare items (vendor 5, filament 5, spool 7).")
    w("")
    w("## Ours, not differences\n")
    w("Headers the hub adds that Spoolman does not send (`OURS_HEADERS` in the test; not listed as "
      "differences):\n")
    for line in OURS_NOTES:
        w(f"- {line}")
    w("")
    w("## Ours (owner)\n")
    w("Maker Connect's own additions. They are not in Spoolman, they leave Spoolman's fields as they are, "
      "and none of the requests below sets them (Spoolman would refuse the keys), so they are not compared:\n")
    for line in OWNER_NOTES:
        w(f"- {line}")
    w("")
    w("## Not covered\n")
    w("- SpoolmanDB import and Spoolman's three `/external/*` routes (" + ", ".join(
        f"`{r}`" for r in fixture["not_covered_routes"]) + "): the catalogue comes from the internet "
      "(9745 filaments); the hub's `spoolmandb.rs` is not compared either.")
    w("- The request bodies, the fields in them, the status codes asked for and the value variants "
      "(lengths, types, spellings) are chosen by hand; only the routes and the query parameters are "
      "derived from the OpenAPI. A case that is not in the list is not checked.")
    for line in NOT_COVERED_NOTES:
        w(f"- {line}")
    w("")
    w("## Every request\n")
    w("Spoolman: its answer (recorded). Ours: the hub's status. Result: **identical** (status, headers, "
      "body, after the normalisation) or exactly where it differs: `only Spoolman` = the field or "
      "header is Spoolman's alone, `only ours` = the hub's alone, `(xN)` = the same difference in N "
      "items of the answer.\n")
    w("| # | Request | Spoolman | Ours | Result |\n|---|---|---|---|---|")
    for n, e in enumerate(reqs, 1):
        head, lines = blocks[n]
        status_ours = e["status"]
        for x in lines:
            m = re.match(r"status: Spoolman \d+, ours (\d+)", x)
            if m:
                status_ours = m.group(1)
        if lines == ["identical"]:
            ours, result = f"{status_ours}, same", "**identical**"
        else:
            ours = f"{status_ours}, differs"
            cells = [x.replace("|", "/").replace("`", "'") for x in lines]
            result = "<br>".join(cells)
        req = f"`{e['method']} {e['path'].removeprefix('/api/v1')}`".replace("|", "%7C")
        w(f"| {n} | {e['name']}: {req} | {brief(e)} | {ours} | {result} |")
    w("")
    DOC.write_text("\n".join(out))
    print(f"{DOC}: {identical} of {total} identical")


OURS_NOTES = [
    "`access-control-expose-headers: x-total-count` on lists: the hub lets a browser page read the "
    "count; Spoolman sends it only when its CORS is configured.",
    "`/info` has `server: Maker Connect` beside Spoolman's fields (the hub's own name for what it is).",
]

OWNER_NOTES = [
    "A filament's `extra` keys `kind` and `unit` (a label tape counts metres), `color_name`, `ink`, "
    "`media`, `width_mm` (a tape's width) and `minimum`.",
    "A spool's `extra` keys `loaded_in` (the machine it is loaded in) and `slot` (its slot in a dry box).",
    "Locations are places in the hub (with a code and a label); they go out as their names, and a new "
    "name makes a place.",
    "A spool's number is its code: spool 7 is the label `S-0007` (not in the API).",
    "The count-off: `use` and `measure` land on the amount of the spool as the hub's Materials page shows it.",
]

NOT_COVERED_NOTES = [
    "WebSocket sequences (`/api/v1`, `/vendor`, `/filament`, `/spool` and `/{id}`, `/setting/{key}`, "
    "`/tag/scan/{reader}`): not run against Spoolman; the hub's own test checks one `updated` event of a spool.",
    "gzip: Spoolman compresses answers of 500 bytes or more for a client that sends `accept-encoding`; no request does.",
    "CORS and the trusted-origin and trusted-host guards (`SPOOLMAN_CORS_ORIGIN`, `SPOOLMAN_TRUSTED_ORIGINS`): "
    "no `Origin` or foreign `Host` is sent.",
    "Databases other than SQLite, `SPOOLMAN_BASE_PATH`, automatic backups, `/metrics`, the web client.",
    "Spoolman's own crash: `PATCH /spool/{id}` with `\"extra\": null` is a 500 in Spoolman at this commit; not asked.",
    "Request bodies larger than a few kilobytes, concurrent requests, a restart between requests.",
    "The hub's own endpoints that Spoolman does not have (`/export`, tag readers' live `GET /tag/scan`, "
    "`/api/v1/field/.../values` beyond one request): not asked of Spoolman, so there is nothing to compare.",
]


if __name__ == "__main__":
    if "--doc" in sys.argv:
        write_doc()
    else:
        main()
