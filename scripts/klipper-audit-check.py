#!/usr/bin/env python3
"""Checks docs/klipper-audit.md against Moonraker's [simplyprint] component, mechanically.

The hub (crates/hub/src/simplyprint.rs) plays the SimplyPrint SERVER for Moonraker. The server
itself is closed, so the reference is the open client: reference/moonraker/moonraker/components/
simplyprint.py. Every item below is taken from that source, never typed by hand:

  sent        message types Moonraker sends (send_sp calls) and the fields inside each, by
              following the dict it builds (literals, key assignments, updates, helper returns)
  accepted    message types it takes from the server (_process_message), its demands
              (_process_demand), the fields it reads of each, and the guards in front of them
  value       the values of state_change.new, connection.new, file_progress.state/message,
              filament_sensor.state
  klipper     the Klipper objects it subscribes to (they make up temps, mesh_data, ...)
  connection  config options, constants, URL forms, headers, timeouts, reconnect rules,
              keep-alive, the send gate, the websocket calls

Each must be named in backticks in docs/klipper-audit.md. Rows whose status says "other route /
in effect / differs / on purpose" are refused; a row that says same or fixed needs a test that
exists in crates/**/*.rs and contains the row's key terms. The other way round: every message
type, demand and field the hub sends or reads must be something Moonraker sends/accepts, or be
named in section "6. What we send or read that Moonraker does not" with why.

Run: python3 -I scripts/klipper-audit-check.py
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PY = ROOT / "reference" / "moonraker" / "moonraker" / "components" / "simplyprint.py"
RS = ROOT / "crates" / "hub" / "src" / "simplyprint.rs"
AUDIT_PATH = ROOT / "docs" / "klipper-audit.md"


# ---------------------------------------------------------------- small parsing helpers

def line_of(text, pos):
    return text.count("\n", 0, pos) + 1


def skip_string(text, i):
    """text[i] is a quote: the index after the string."""
    q = text[i]
    if text.startswith(q * 3, i):
        end = text.find(q * 3, i + 3)
        return len(text) if end < 0 else end + 3
    i += 1
    while i < len(text):
        if text[i] == "\\":
            i += 2
        elif text[i] == q:
            return i + 1
        else:
            i += 1
    return i


def match_close(text, i):
    """text[i] opens a bracket: the index of its closing bracket (strings and comments skipped)."""
    pairs = {"(": ")", "[": "]", "{": "}"}
    depth = 0
    while i < len(text):
        c = text[i]
        if c in "\"'":
            i = skip_string(text, i)
            continue
        if c == "#":
            i = text.find("\n", i)
            if i < 0:
                return len(text)
            continue
        if c in pairs:
            depth += 1
        elif c in ")]}":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return len(text)


def split_top(text):
    """Split on commas outside brackets and strings."""
    parts, depth, start, i = [], 0, 0, 0
    while i < len(text):
        c = text[i]
        if c in "\"'":
            i = skip_string(text, i)
            continue
        if c == "#":
            j = text.find("\n", i)
            i = len(text) if j < 0 else j
            continue
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        elif c == "," and depth == 0:
            parts.append(text[start:i])
            start = i + 1
        i += 1
    parts.append(text[start:])
    return [p.strip() for p in parts if p.strip()]


def statement(text, i):
    """The expression starting at i, up to a newline outside brackets."""
    j = i
    depth = 0
    while j < len(text):
        c = text[j]
        if c in "\"'":
            j = skip_string(text, j)
            continue
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        elif c == "\n" and depth <= 0:
            break
        j += 1
    return text[i:j].strip()


IDENT = re.compile(r"^[A-Za-z_]\w*$")


# ---------------------------------------------------------------- the Python reference

class Reference:
    def __init__(self, path):
        self.text = path.read_text()
        self.funcs = {}  # name -> dict(start, end, text, params, line)
        self._find_functions()
        self.items = {}  # name -> dict(cat, lines)
        self._sub_objs = {}
        self.heaters = []

    # --- functions
    def _find_functions(self):
        lines = self.text.split("\n")
        offsets, pos = [], 0
        for l in lines:
            offsets.append(pos)
            pos += len(l) + 1
        defs = [(i, re.match(r"^( *)(?:async )?def (\w+)\(", l)) for i, l in enumerate(lines)]
        for i, m in defs:
            if not m:
                continue
            indent = len(m.group(1))
            j = i + 1
            while j < len(lines):
                l = lines[j]
                if l.strip() and (len(l) - len(l.lstrip())) <= indent and l.lstrip()[0] not in ")]}":
                    break
                j += 1
            start = offsets[i]
            end = offsets[j] if j < len(lines) else len(self.text)
            body = self.text[start:end]
            open_at = body.index("(")
            params_src = body[open_at + 1:match_close(body, open_at)]
            params = [re.match(r"\*{0,2}(\w+)", p).group(1) for p in split_top(params_src)]
            self.funcs[m.group(2)] = dict(start=start, end=end, text=body, params=params, line=i + 1)

    def func_at(self, pos):
        for name, f in self.funcs.items():
            if f["start"] <= pos < f["end"] and name not in ("update", "start", "stop", "reset"):
                return name
        for name, f in self.funcs.items():
            if f["start"] <= pos < f["end"]:
                return name
        return None

    def add(self, name, cat, line):
        self.items.setdefault(name, dict(cat=cat, lines=[]))["lines"].append(line)

    # --- resolving what a dict holds
    def resolve(self, expr, fname, seen=None):
        """(dotted key paths, dynamic) of the dict an expression is."""
        seen = seen if seen is not None else set()
        expr = expr.strip()
        if expr in ("", "None"):
            return set(), False
        if expr.startswith("{"):
            return self._literal(expr, fname, seen)
        if IDENT.match(expr):
            return self._var(expr, fname, seen)
        m = re.fullmatch(r"self\.cache\.(\w+)", expr)
        if m:
            return self._cache(m.group(1), seen)
        m = re.fullmatch(r'self\.printer_status\["(\w+)"\]', expr)
        if m:
            return set(self._sub_objs.get(m.group(1), [])), False
        return set(), True

    def _literal(self, expr, fname, seen):
        keys, dyn = set(), False
        for entry in split_top(expr[1:match_close(expr, 0)]):
            m = re.match(r'^"(\w+)"\s*:\s*(.*)$', entry, re.S)
            if not m:
                dyn = True
                continue
            keys.add(m.group(1))
            value = m.group(2).strip()
            if IDENT.match(value) or re.fullmatch(r"self\.cache\.\w+", value):
                sub, d = self.resolve(value, fname, seen)
                if sub and not d:
                    keys |= {f"{m.group(1)}.{s}" for s in sub}
        return keys, dyn

    def _var(self, v, fname, seen):
        key = (v, fname)
        if key in seen:
            return set(), False
        seen = seen | {key}
        f = self.funcs[fname]
        text = f["text"]
        keys, dyn, found = set(), False, False
        for m in re.finditer(rf"^[ \t]*{v}(?:\s*:\s*[^=\n]+)?\s*=(?!=)\s*", text, re.M):
            found = True
            rhs = statement(text, m.end())
            if rhs.startswith("{"):
                k, d = self._literal(rhs, fname, seen)
            elif re.match(r"self\.(\w+)\(", rhs) and re.match(r"self\.(\w+)\(", rhs).group(1) in self.funcs and \
                    re.match(r"self\.(\w+)\(", rhs).group(1) != "_get_object_diff":
                k, d = self._returned(re.match(r"self\.(\w+)\(", rhs).group(1), seen)
            elif rhs.startswith("self._get_object_diff("):
                arg0 = split_top(rhs[rhs.index("(") + 1:match_close(rhs, rhs.index("("))])[0]
                k, d = self.resolve(arg0, fname, seen)
            elif re.fullmatch(r'self\.printer_status\["\w+"\]', rhs) or IDENT.match(rhs):
                k, d = self.resolve(rhs, fname, seen)
            else:
                k, d = set(), True
            keys |= k
            dyn |= d
        for m in re.finditer(rf"^[ \t]*{v}\[(.+?)\]\s*=(?!=)", text, re.M):
            found = True
            km = re.fullmatch(r'"(\w+)"', m.group(1))
            if km:
                keys.add(km.group(1))
            else:
                dyn = True
        for m in re.finditer(rf"\b{v}\.update\(", text):
            found = True
            call = text[m.end() - 1:match_close(text, m.end() - 1) + 1]
            arg = split_top(call[1:-1])[0]
            k, d = self.resolve(arg, fname, seen)
            keys |= k
            dyn |= d
        m = re.search(rf"^[ \t]*for {v} in (.+):$", text, re.M)
        if m:
            found = True
            k, d = self._iterated(m.group(1).strip(), seen)
            keys |= k
            dyn |= d
        if v in f["params"]:
            found = True
            idx = f["params"].index(v) - 1  # self
            callers = False
            for other, of in self.funcs.items():
                for cm in re.finditer(rf"self\.{fname}\(", of["text"]):
                    callers = True
                    call = of["text"][cm.end() - 1:match_close(of["text"], cm.end() - 1) + 1]
                    args = split_top(call[1:-1])
                    if 0 <= idx < len(args):
                        k, d = self.resolve(args[idx], other, seen)
                        keys |= k
                        dyn |= d
            if not callers:
                dyn = True
        return keys, dyn or not found

    def _returned(self, fname, seen):
        keys, dyn = set(), False
        text = self.funcs[fname]["text"]
        rets = re.findall(r"^[ \t]*return (\w+)$", text, re.M)
        if not rets:
            return set(), True
        for r in rets:
            k, d = self._var(r, fname, seen)
            keys |= k
            dyn |= d
        return keys, dyn

    def _iterated(self, expr, seen):
        m = re.fullmatch(r"self\.(\w+)", expr)
        if not m:
            return set(), True
        keys, dyn, found = set(), False, False
        for other, of in self.funcs.items():
            for am in re.finditer(rf"self\.{m.group(1)}\.append\(", of["text"]):
                found = True
                call = of["text"][am.end() - 1:match_close(of["text"], am.end() - 1) + 1]
                k, d = self.resolve(split_top(call[1:-1])[0], other, seen)
                keys |= k
                dyn |= d
        return keys, dyn or not found

    def _cache(self, name, seen):
        keys, dyn, found = set(), False, False
        for other, of in self.funcs.items():
            text = of["text"]
            for m in re.finditer(rf"self\.cache\.{name}\.update\(", text):
                found = True
                call = text[m.end() - 1:match_close(text, m.end() - 1) + 1]
                k, d = self.resolve(split_top(call[1:-1])[0], other, seen)
                keys |= k
                dyn |= d
            for m in re.finditer(rf"^[ \t]*self\.cache\.{name}(?:\s*:\s*[^=\n]+)?\s*=(?!=)\s*", text, re.M):
                rhs = statement(text, m.end())
                if rhs in ("{}", '""', '""', "None") or rhs.startswith('"'):
                    continue
                if rhs.startswith("dict("):
                    rhs = rhs[5:-1]
                found = True
                k, d = self.resolve(rhs, other, seen)
                keys |= k
                dyn |= d
            for m in re.finditer(rf"^[ \t]*self\.cache\.{name}\[(.+?)\]\s*=(?!=)", text, re.M):
                found = True
                km = re.fullmatch(r'"(\w+)"', m.group(1))
                if km:
                    keys.add(km.group(1))
                else:
                    dyn = True
        return keys, dyn or not found

    # --- extraction
    def extract(self):
        t = self.text
        self._subscriptions()
        self._sent()
        self._accepted()
        self._values()
        self._connection()
        return self.items

    def _subscriptions(self):
        f = self.funcs["_on_klippy_ready"]
        text = f["text"]
        m = re.search(r"sub_objs = \{", text)
        lit = text[m.end() - 1:match_close(text, m.end() - 1) + 1]
        for entry in split_top(lit[1:-1]):
            em = re.match(r'"(\w+)"\s*:\s*\[(.*)\]$', entry, re.S)
            fields = re.findall(r'"(\w+)"', em.group(2))
            self._sub_objs[em.group(1)] = fields
            for fld in fields:
                self.add(f"klipper:{em.group(1)}.{fld}", "klipper",
                         line_of(self.text, f["start"] + m.start()))
        for m in re.finditer(r"sub_objs\[([^\]]+)\] = \[([^\]]*)\]", text):
            for fld in re.findall(r'"(\w+)"', m.group(2)):
                self.add(f"klipper:[{m.group(1)}].{fld}", "klipper", line_of(self.text, f["start"] + m.start()))
        for m in re.finditer(r'query_objects\(\{"(\w+)": None\}', text):
            self.add(f"klipper:query {m.group(1)}", "klipper", line_of(self.text, f["start"] + m.start()))
        # heater keys of temps
        for m in re.finditer(r'self\.heaters\[\w+\] = "(\w+)"|tool_id = "(\w+)"$|tool_id = "(tool)" \+ htr', text, re.M):
            name = m.group(1) or m.group(2) or (m.group(3) + "<N>")
            self.heaters.append((name, line_of(self.text, f["start"] + m.start())))

    def _sent(self):
        t = self.text
        gate_types = {}
        sf = self.funcs["send_sp"]
        m = re.search(r"evt_name in \(([^)]*)\)", sf["text"])
        extra_keys = {}
        if m:
            um = re.search(r"data\.update\(\{(.*?)\}\)", sf["text"], re.S)
            for typ in re.findall(r'"(\w+)"', m.group(1)):
                for k in re.findall(r'"(\w+)"\s*:', um.group(1)):
                    extra_keys[(typ, k)] = line_of(t, sf["start"] + m.start())
        seen_types = set()
        for m in re.finditer(r"(?<!def )\bsend_sp\(", t):
            fname = self.func_at(m.start())
            call = t[m.end() - 1:match_close(t, m.end() - 1) + 1]
            args = split_top(call[1:-1])
            tm = re.fullmatch(r'"(\w+)"', args[0]) if args else None
            if not tm:
                continue
            typ, line = tm.group(1), line_of(t, m.start())
            self.add(f"sent:{typ}", "sent type", line)
            seen_types.add(typ)
            if len(args) < 2:
                continue
            keys, dyn = self.resolve(args[1], fname)
            if typ == "temps" and dyn:
                dyn = False
                for name, hl in self.heaters:
                    self.add(f"sent:temps.{name}", "sent field", hl)
            for k in keys:
                self.add(f"sent:{typ}.{k}", "sent field", line)
            if dyn:
                self.add(f"sent:{typ}.*", "sent field", line)
        for (typ, k), line in extra_keys.items():
            self.add(f"sent:{typ}.{k}", "sent field", line)
        # the wrapper every message goes in
        m = re.search(r'packet = \{(.*?)\}', t, re.S)
        for k in re.findall(r'"(\w+)"\s*:', m.group(1)):
            self.add(f"sent-envelope:{k}", "connection", line_of(t, m.start()))

    def _accepted(self):
        t = self.text
        pm = self.funcs["_process_message"]
        text = pm["text"]
        base = pm["start"]
        for m in re.finditer(r'packet\.get\("(\w+)"', text):
            self.add(f"accepted-envelope:{m.group(1)}", "connection", line_of(t, base + m.start()))
        heads = list(re.finditer(r'^        (?:el)?if event == "(\w+)":', text, re.M))
        for i, h in enumerate(heads):
            end = heads[i + 1].start() if i + 1 < len(heads) else len(text)
            branch = text[h.start():end]
            typ = h.group(1)
            self.add(f"accepted:{typ}", "accepted type", line_of(t, base + h.start()))
            fields = {}
            for fm in re.finditer(r'\bdata\.get\("(\w+)"|\bdata\["(\w+)"\]|"(\w+)" in data\b|\bdata\.pop\("(\w+)"', branch):
                name = next(g for g in fm.groups() if g)
                fields.setdefault(name, line_of(t, base + h.start() + fm.start()))
            for name, line in fields.items():
                self.add(f"accepted:{typ}.{name}", "accepted field", line)
            if re.search(r"_update_intervals\(", branch):
                for k, line in self._interval_keys():
                    if typ == "connected":
                        self.add(f"accepted:connected.interval.{k}", "accepted field", line)
                    else:
                        self.add(f"accepted:{typ}.{k}", "accepted field", line)
        dm = self.funcs["_process_demand"]
        text = dm["text"]
        base = dm["start"]
        heads = list(re.finditer(r'^        (?:el)?if demand (?:==|in) (".*?"|\[.*?\]):', text, re.M))
        for i, h in enumerate(heads):
            end = heads[i + 1].start() if i + 1 < len(heads) else len(text)
            branch = text[h.start():end]
            names = re.findall(r'"(\w+)"', h.group(1))
            for name in names:
                line = line_of(t, base + h.start())
                self.add(f"accepted:demand:{name}", "accepted demand", line)
                fields = {}
                for fm in re.finditer(r'\bargs\.get\("(\w+)"|\bargs\["(\w+)"\]|"(\w+)" in args\b', branch):
                    fields.setdefault(next(g for g in fm.groups() if g), line_of(t, base + h.start() + fm.start()))
                hand = re.search(r"\.(\w+)\(args\)", branch)
                if hand and hand.group(1) in self.funcs:
                    hf = self.funcs[hand.group(1)]
                    param = hf["params"][1]
                    for fm in re.finditer(rf'\b{param}\.get\("(\w+)"', hf["text"]):
                        fields.setdefault(fm.group(1), line_of(t, hf["start"] + fm.start()))
                for fname, fline in fields.items():
                    self.add(f"accepted:demand:{name}.{fname}", "accepted field", fline)
                if "kconn.is_connected()" in branch:
                    self.add(f"accepted:demand:{name}#klippy-connected", "guard", line)
                gm = re.search(r'self\.cache\.state == "(\w+)"', branch)
                if gm:
                    self.add(f"accepted:demand:{name}#state={gm.group(1)}", "guard", line)
                if re.search(r"if script_list:", branch):
                    self.add(f"accepted:demand:{name}#list-nonempty", "guard", line)
                if re.search(r"isinstance\(url, str\)", branch):
                    self.add(f"accepted:demand:{name}#url-must-be-a-string", "guard", line)
        # state a pause, resume or cancel needs
        rf = self.funcs["_request_print_action"]
        for m in re.finditer(r'if action == "(\w+)":\s*\n\s*if cur_state (?:==|in) (.+?):', rf["text"]):
            for st in re.findall(r'"(\w+)"', m.group(2)):
                self.add(f"accepted:demand:{m.group(1)}#state={st}", "guard", line_of(t, rf["start"] + m.start()))
        gf = self.funcs["_handle_gcode_demand"]
        m = re.search(r"if ident is not None:", gf["text"])
        if m:
            self.add("accepted:demand:gcode#identifier-answers-with-gcode_executed", "guard",
                     line_of(t, gf["start"] + m.start()))
        if "demand = data.pop(\"demand\"" in self.text:
            m = re.search(r'data\.pop\("demand"', self.text)
            self.add("accepted:demand.demand", "accepted field", line_of(t, m.start()))

    def _interval_keys(self):
        m = re.search(r"self\.intervals: Dict\[str, float\] = \{", self.text)
        lit = self.text[m.end() - 1:match_close(self.text, m.end() - 1) + 1]
        return [(k, line_of(self.text, m.start() + lit.index(f'"{k}"'))) for k in re.findall(r'"(\w+)":', lit)]

    def _values(self):
        t = self.text
        for m in re.finditer(r'_update_state\("(\w+)"\)|sp_state = "(\w+)"|self\.state = "(\w+)"', t):
            v = next(g for g in m.groups() if g)
            self.add(f"value:state_change.new={v}", "value", line_of(t, m.start()))
        for m in re.finditer(r'send_sp\("connection", \{"new": "(\w+)"', t):
            self.add(f"value:connection.new={m.group(1)}", "value", line_of(t, m.start()))
        for m in re.finditer(r'"(\w+)" if detected else "(\w+)"', t):
            for v in m.groups():
                self.add(f"value:filament_sensor.state={v}", "value", line_of(t, m.start()))
        cls = t[t.index("class PrintHandler"):t.index("class ProtoLogger")]
        off = t.index("class PrintHandler")
        for m in re.finditer(r'"state": "(\w+)"|\bstate = "(\w+)"|data\["state"\] = "(\w+)"|"state": state_name', cls):
            v = next((g for g in m.groups() if g), None)
            if v:
                self.add(f"value:file_progress.state={v}", "value", line_of(t, off + m.start()))
        for m in re.finditer(r'"message": (f?"[^"]*")|data\["message"\] = (f?"[^"]*")', cls):
            v = (m.group(1) or m.group(2)).lstrip("f").strip('"')
            self.add(f"value:file_progress.message={v}", "value", line_of(t, off + m.start()))

    def _connection(self):
        t = self.text
        for m in re.finditer(r"config\.get\(\s*\"(\w+)\"", t):
            self.add(f"config:{m.group(1)}", "connection", line_of(t, m.start()))
        for m in re.finditer(r"config\.(get_prefix_sections|has_section)\(", t):
            self.add(f"config:{m.group(1)}", "connection", line_of(t, m.start()))
        for m in re.finditer(r"^([A-Z][A-Z_]+) = ", t, re.M):
            self.add(f"const:{m.group(1)}", "connection", line_of(t, m.start()))
        for m in re.finditer(r"^PRE_SETUP_EVENTS = \[(.*?)\]", t, re.M | re.S):
            for e in re.findall(r'"(\w+)"', m.group(1)):
                self.add(f"pre-setup:{e}", "connection", line_of(t, m.start()))
        for m in re.finditer(r'(?:connect_url|\burl) = f"([^"]*)"|self\.connect_url = (connect_url)$', t, re.M):
            self.add(f"url:{m.group(1) or m.group(2)}", "connection", line_of(t, m.start()))
        for m in re.finditer(r'tornado\.websocket\.(\w+)\(', t):
            self.add(f"ws:{m.group(1)}", "connection", line_of(t, m.start()))
        for m in re.finditer(r'self\.ws\.(read_message|write_message|close)\(([^)]*)\)', t):
            what = m.group(1) + ("(" + m.group(2) + ")" if m.group(1) == "close" and m.group(2) else "")
            self.add(f"ws:{what}", "connection", line_of(t, m.start()))
        for m in re.finditer(r'setattr\(self\.ws, "(\w+)"', t):
            self.add(f"ping:{m.group(1)}", "connection", line_of(t, m.start()))
        for m in re.finditer(r"self\.ping_sp_timer\.(start|stop)\(", t):
            self.add(f"ping:ping_sp_timer.{m.group(1)}", "connection", line_of(t, m.start()))
        for name in ("_handle_sp_ping", "_last_sp_ping", "_last_ping_received"):
            m = re.search(rf"\b{name}\b", t)
            self.add(f"ping:{name}", "connection", line_of(t, m.start()))
        for m in re.finditer(r"\b(connect_timeout|request_timeout)=([\d.]+)", t):
            self.add(f"timeout:{m.group(1)}={m.group(2)}", "connection", line_of(t, m.start()))
        for m in re.finditer(r"asyncio\.wait_for\((.+?),\s*([\d.]+)\)", t):
            self.add(f"timeout:wait_for({m.group(1)}, {m.group(2)})", "connection", line_of(t, m.start()))
        for m in re.finditer(r"self\.reconnect_delay(?:: float)? = ([\d.]+)", t):
            self.add(f"reconnect:reconnect_delay={m.group(1)}", "connection", line_of(t, m.start()))
        m = re.search(r"asyncio\.sleep\(self\.reconnect_delay\)", t)
        self.add("reconnect:sleep(reconnect_delay)", "connection", line_of(t, m.start()))
        for m in re.finditer(r"self\.reconnect_token = (None|data\.get\(\"reconnect_token\"\))", t):
            self.add(f"reconnect:reconnect_token={m.group(1)}", "connection", line_of(t, m.start()))
        for m in re.finditer(r'headers = \{(.*?)\}', t, re.S):
            for k, v in re.findall(r'"([\w-]+)":\s*"([^"]*)"', m.group(1)):
                self.add(f"header:{k}: {v}", "connection", line_of(t, m.start()))
        for m in re.finditer(r'accept = "([^"]*)"', t):
            self.add(f"header:Accept: {m.group(1)}", "connection", line_of(t, m.start()))
        for k, line in self._interval_keys():
            self.add(f"interval:{k}", "connection", line)
        sf = self.funcs["send_sp"]
        m = re.search(r"if \(\n(.*?)\n\s*\):", sf["text"], re.S)
        for cond in re.split(r"\s+or\s*$", m.group(1), flags=re.M):
            cond = cond.strip()
            if cond:
                self.add(f"gate:{cond}", "connection", line_of(t, sf["start"] + m.start()))
        m = re.search(r"except jsonw\.JSONDecodeError", t)
        self.add("msg:invalid-JSON-ignored", "connection", line_of(t, m.start()))
        m = re.search(r"isinstance\(message, str\)", t)
        self.add("msg:text-frames-only", "connection", line_of(t, m.start()))
        m = re.search(r"jsonw\.dumps\(packet\)", t)
        self.add("msg:json-text-frame", "connection", line_of(t, m.start()))


# ---------------------------------------------------------------- the hub's Rust

def nontest(text):
    m = re.search(r"#\[cfg\(test\)\]\s*(?:pub(?:\(crate\))?\s+)?mod tests", text)
    return text[: m.start()] if m else text


def json_paths(lit):
    """Dotted key paths of a json!({...}) body."""
    out = set()
    inner = lit.strip()
    if not inner.startswith("{"):
        return out
    for entry in split_top(inner[1:match_close(inner, 0)]):
        m = re.match(r'^"(\w+)"\s*:\s*(.*)$', entry, re.S)
        if not m:
            continue
        out.add(m.group(1))
        v = m.group(2).strip()
        if v.startswith("{"):
            out |= {f"{m.group(1)}.{p}" for p in json_paths(v)}
    return out


def hub_items(code):
    """What the hub sends and reads: name -> list of lines (code before the tests)."""
    code = nontest(code)
    items = {}

    def add(name, pos):
        items.setdefault(name, []).append(line_of(code, pos))

    for m in re.finditer(r'json!\(\{"type": "(\w+)"', code):
        typ = m.group(1)
        add(f"hub-sends:{typ}", m.start())
        start = code.index("{", code.index("json!(", m.start()))
        body = code[start:match_close(code, start) + 1]
        for p in json_paths(body):
            if p.startswith("data."):
                add(f"hub-sends:{typ}.{p[5:]}", m.start())
    for m in re.finditer(r"(?<!fn )\bdemand\(\s*(\"(\w+)\"|match)", code):
        call_open = code.index("(", m.start())
        call = code[call_open:match_close(code, call_open) + 1]
        names = [m.group(2)] if m.group(2) else re.findall(r'=> "(\w+)"', call)
        jm = re.search(r"json!\((\{.*\})\)", call, re.S)
        paths = json_paths(jm.group(1)) if jm else set()
        for n in names:
            add(f"hub-sends:demand:{n}", m.start())
            for p in paths:
                add(f"hub-sends:demand:{n}.{p}", m.start())
    if re.search(r'fn demand\(', code):
        add("hub-sends:demand", code.index("fn demand("))
    # what it reads
    fm = re.search(r"fn heard\(", code)
    body_start = fm.start()
    body = code[body_start:code.index("\nfn end_job", body_start)]
    arms = list(re.finditer(r'^        "(\w+)"[^\n]*=> \{?', body, re.M))
    for i, a in enumerate(arms):
        end = arms[i + 1].start() if i + 1 < len(arms) else len(body)
        arm = body[a.start():end]
        typ = a.group(1)
        add(f"hub-reads:{typ}", body_start + a.start())
        for r in re.finditer(r'\bdata\["(\w+)"\]|\bdata\.get\("(\w+)"\)', arm):
            add(f"hub-reads:{typ}.{r.group(1) or r.group(2)}", body_start + a.start() + r.start())
        for r in re.finditer(r'\bfw\["(\w+)"\]', arm):
            add(f"hub-reads:{typ}.fw.{r.group(1)}", body_start + a.start() + r.start())
        if typ == "temps":
            for r in re.finditer(r'^\s+"(\w+)" =>', arm[arm.index("\n"):], re.M):
                add(f"hub-reads:temps.{r.group(1)}", body_start + a.start() + r.start())
        for r in re.finditer(r'\bdata\["(\w+)"\]\s*==\s*"(\w+)"', arm):
            add(f"hub-value:{typ}.{r.group(1)}={r.group(2)}", body_start + a.start() + r.start())
    sm = re.search(r"fn state\(new: &str\)[^{]*\{(.*?)\n\}", code, re.S)
    for v in re.findall(r'"(\w+)"', sm.group(1)):
        add(f"hub-value:state_change.new={v}", sm.start())
    for r in re.finditer(r'packet\["(\w+)"\]', code):
        add(f"hub-reads-envelope:{r.group(1)}", r.start())
    for r in re.finditer(r'kind == "(\w+)"', code):
        add(f"hub-reads:{r.group(1)}", r.start())
    # not messages: where it listens, what it takes from the request, how it refuses
    for r in re.finditer(r'\.route\(\s*"([^"]+)"', code):
        add(f"hub-route:{r.group(1)}", r.start())
    for r in re.finditer(r'headers\s*\.get\("([\w-]+)"\)', code):
        add(f"hub-header:{r.group(1)}", r.start())
    for r in re.finditer(r"StatusCode::(\w+)", code):
        add(f"hub-status:{r.group(1)}", r.start())
    return items


def compatible(name, theirs):
    """Is a hub item one Moonraker sends/accepts?  Returns None when not a message item."""
    if name.startswith("hub-sends:demand:"):
        rest = name[len("hub-sends:demand:"):]
        return f"accepted:demand:{rest}" in theirs
    if name == "hub-sends:demand":
        return "accepted:demand" in theirs or "accepted:demand.demand" in theirs
    if name.startswith("hub-sends:"):
        return f"accepted:{name[len('hub-sends:'):]}" in theirs
    if name.startswith("hub-reads-envelope:"):
        return f"accepted-envelope:{name.split(':', 1)[1]}" in theirs
    if name.startswith("hub-reads:"):
        rest = name[len("hub-reads:"):]
        return f"sent:{rest}" in theirs or f"sent:{rest.replace('.fw.', '.fw.')}" in theirs
    if name.startswith("hub-value:"):
        return f"value:{name[len('hub-value:'):]}" in theirs
    return False  # route, header, status: not Moonraker's


# ---------------------------------------------------------------- main

def main():
    ref = Reference(PY)
    items = ref.extract()
    hub = hub_items(RS.read_text())
    audit = AUDIT_PATH.read_text() if AUDIT_PATH.exists() else ""

    # Guard: the extraction must not silently shrink when the source changes.
    cats = {}
    for name, v in items.items():
        cats.setdefault(v["cat"], []).append(name)
    floor = {"sent type": 20, "sent field": 60, "accepted type": 5, "accepted demand": 10,
             "accepted field": 15, "value": 10, "klipper": 10, "connection": 40}
    for c, n in floor.items():
        if len(cats.get(c, [])) < n:
            print(f"extraction broke: only {len(cats.get(c, []))} '{c}' items (expected at least {n})")
            sys.exit(2)

    print(f"Moonraker items from {PY.relative_to(ROOT)}: {len(items)}")
    for c in ("sent type", "sent field", "accepted type", "accepted demand", "accepted field",
              "guard", "value", "klipper", "connection"):
        print(f"  {c}: {len(cats.get(c, []))}")
    unresolved = sorted(n for n in items if n.endswith(".*"))
    if unresolved:
        print(f"  (payload not a dict we can follow, named as .*: {', '.join(unresolved)})")

    failed = False

    missing = [n for n in sorted(items) if f"`{n}`" not in audit]
    if missing:
        failed = True
        print(f"NOT in the audit: {len(missing)}")
        for n in missing:
            print(f"  {n}   (simplyprint.py:{items[n]['lines'][0]})")

    # Rows: status words. To the letter: no "other route", "in effect", "differs", "on purpose".
    code = "\n".join(p.read_text() for p in (ROOT / "crates").rglob("*.rs"))
    loose, unproven, cited, rows = [], [], {}, 0
    status_count = {}
    for row in audit.splitlines():
        if not row.startswith("|") or row.startswith("|---"):
            continue
        cells = row.strip().strip("|").split("|")
        if len(cells) < 4:
            continue
        status = cells[-1]
        first = cells[0].strip()
        if first in ("item", "Item", "Code"):
            continue
        rows += 1
        if re.search(r"other route|another route|in effect|differs|on purpose", status):
            loose.append(first)
        if "UNCHECKED" in status:
            status_count["UNCHECKED"] = status_count.get("UNCHECKED", 0) + 1
            continue
        tests = re.findall(r"`([a-z][a-z0-9_]+)`", status)
        if "test" in status and tests:
            cited[first] = tests
        elif re.search(r"\b(same|fixed)\b", status) and "n/a" not in status:
            unproven.append(first)
        else:
            status_count["other"] = status_count.get("other", 0) + 1

    def body(name):
        m = re.search(rf"\bfn {name}\(", code)
        if not m:
            return None
        rest = code[m.start():]
        end = re.search(r"\n    }\n", rest)
        return rest[: end.end()] if end else rest

    no_test, weak = [], []
    for first, tests in cited.items():
        bodies = []
        for t in tests:
            b = body(t)
            if b is None:
                no_test.append(f"{first}: {t}")
            else:
                bodies.append(b)
        text = "\n".join(bodies)
        for n in re.findall(r"`([^`]+)`", first):
            term = re.split(r"[.:=#]", n)[-1] if "=" not in n else n.split("=")[-1]
            term = term.strip("<>*")
            if term and term not in text:
                weak.append(f"{first}: '{term}' not in {', '.join(tests)}")

    print(f"audit rows: {rows}; UNCHECKED: {status_count.get('UNCHECKED', 0)}; with a named test: {len(cited)}")
    if loose:
        failed = True
        print(f"rows done another way than Moonraker (not allowed): {len(loose)}")
        for n in loose:
            print(f"  {n}")
    if unproven:
        failed = True
        print(f"rows same/fixed without a named test (not proven): {len(unproven)}")
        for n in unproven:
            print(f"  {n}")
    if no_test:
        failed = True
        print("tests named in the audit that do not exist:")
        for n in no_test:
            print(f"  {n}")
    if weak:
        failed = True
        print(f"named tests that do not contain their row's terms: {len(weak)}")
        for w in weak:
            print(f"  {w}")

    # The other way round.
    section = ""
    m = re.search(r"^## 6\. What we send or read that Moonraker does not\s*$", audit, re.M)
    if m:
        rest = audit[m.end():]
        nxt = re.search(r"^## ", rest, re.M)
        section = rest[: nxt.start()] if nxt else rest
    named = set(re.findall(r"`([^`]+)`", section))
    ok = [n for n in hub if compatible(n, items)]
    extra = [n for n in sorted(hub) if not compatible(n, items) and n not in named]
    print(f"hub sends/reads: {len(hub)} items; ones Moonraker sends/accepts: {len(ok)}; "
          f"not Moonraker's: {len(hub) - len(ok)}")
    if not m:
        failed = True
        print('section "## 6. What we send or read that Moonraker does not" is missing from the audit')
    if extra:
        failed = True
        print(f"NOT Moonraker's and not named in section 6 of the audit: {len(extra)}")
        for n in extra:
            print(f"  {n}   (simplyprint.rs:{hub[n][0]})")
    if failed:
        sys.exit(1)
    print("every item is in the audit; everything the hub sends or reads is Moonraker's or named in section 6")


if __name__ == "__main__":
    main()
