#!/usr/bin/env python3
"""The DYMO scale port's mechanical check (docs/audits/usbscale.md, docs/plans/agent-engine.md Step 3e).

It takes usbscale's own `print_scale_data` and `UNITS` out of reference/usbscale/usbscale.c
(commit 9ed1cb55a2dfd0daeb41548031a49ab756cb9114) as text, compiles them with a small harness, and
runs them, and our decoder (crates/engine/examples/usbscale_decode.rs), on the same reports.
Then it checks UNITS and the status cases against HID Usage Tables 1.6, Table 33.1 (usages 0x50+n
and 0x70+n), and the report layout against HID POS 1.02, Figure 27. It rewrites the generated block
of docs/audits/usbscale.md and exits 0 only when every row is "identical" or a difference the
audit names as ours (E3, E4, E8); anything else is UNEXPECTED and exits 1.

Harness changes to usbscale's code, all of them (nothing else is changed, the function's text is
compiled as extracted):
  H1  `print_scale_data` and `UNITS` are cut out of usbscale.c by their text (the function from its
      definition line to the closing brace at column 0, the table's initializer); the rest of
      usbscale.c (libusb, argp, main) is not compiled.
  H2  `UNITS` is a 256-entry array: usbscale's 13 entries, then "(beyond UNITS)". usbscale's table
      has 13 entries and `UNITS[unit]` for a unit over 12 reads outside it (undefined); the table is
      padded so the C code can run on units 13 and 255.
  H3  `static uint8_t lastStatus` is reset per vector by running one process per vector
      (and one process for a sequence), the function's text is untouched.
  H4  stdout and stderr are told apart by markers the harness prints before and after each call.
  H5  `human` is false: usbscale's `--human` is not ported.

Run with `python3 -I scripts/usbscale-audit-check.py`. Needs a C compiler (cc) and cargo (set
CARGO_TARGET_DIR to build elsewhere). Standard library only."""
import math, os, re, struct, subprocess, sys, tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
C_SRC = os.path.join(ROOT, "reference", "usbscale", "usbscale.c")
SCALES_H = os.path.join(ROOT, "reference", "usbscale", "scales.h")
HUT = os.path.join(ROOT, "reference", "hid-usage-tables", "hut1_6.txt")
POS = os.path.join(ROOT, "reference", "hid-usage-tables", "pos1_02.txt")
DOC = os.path.join(ROOT, "docs", "audits", "usbscale.md")
BEGIN, END = "<!-- generated: begin -->", "<!-- generated: end -->"
COMMIT = "9ed1cb55a2dfd0daeb41548031a49ab756cb9114"


# ---- usbscale's code, as text ----------------------------------------------------------------

def extract():
    text = open(C_SRC, encoding="utf-8").read()
    lines = text.split("\n")
    start = next(i for i, l in enumerate(lines)
                 if l.startswith("static int print_scale_data(unsigned char* dat, bool human) {"))
    end = next(i for i in range(start, len(lines)) if lines[i] == "}")
    function = "\n".join(lines[start:end + 1])
    m = re.search(r"const char\* UNITS\[13\] = \{(.*?)\};", text, re.S)
    entries = re.findall(r'"([^"]*)"', re.sub(r"//[^\n]*", "", m.group(1)))
    assert len(entries) == 13, entries
    weigh_count = int(re.search(r"#define WEIGH_COUNT (\d+)", open(SCALES_H).read()).group(1))
    weigh_size = int(re.search(r"#define WEIGH_REPORT_SIZE (\d+)", text).group(1))
    return function, entries, (start + 1, end + 1), weigh_count, weigh_size


def case_strings(function):
    """status -> the fprintf(stderr, ...) text of each `case`, from the function's text."""
    out = {}
    for m in re.finditer(r"case 0x([0-9a-f]{2}):(.*?)(?=case 0x|default:)", function, re.S):
        s = re.search(r'fprintf\(stderr, "([^"]*)\\n"\)', m.group(2))
        out[int(m.group(1), 16)] = s.group(1) if s else None
    return out


def harness_source(function, entries):
    padded = entries + ["(beyond UNITS)"] * (256 - len(entries))
    table = ",\n".join('    "%s"' % e for e in padded)
    return f"""#include <math.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
const char* UNITS[256] = {{
{table}
}};
{function}
/* one process per sequence: lastStatus (a static in the function) starts at 0 for each */
int main(int argc, char** argv) {{
    setvbuf(stdout, NULL, _IONBF, 0);
    setvbuf(stderr, NULL, _IONBF, 0);
    for (int i = 1; i < argc; i++) {{
        unsigned char dat[16] = {{0}};
        int n = 0;
        for (char* p = argv[i]; *p && n < 16; n++) {{ dat[n] = (unsigned char)strtoul(p, &p, 16); if (*p == ',') p++; }}
        printf("@@%d\\n", i); fprintf(stderr, "@@%d\\n", i);
        int r = print_scale_data(dat, false);
        printf("@@ret %d\\n", r); fprintf(stderr, "@@end\\n");
    }}
    return 0;
}}
"""


def hexs(b):
    return ",".join("%02x" % x for x in b)


def run_c(binary, vectors):
    p = subprocess.run([binary] + [hexs(v) for v in vectors], capture_output=True, text=True, check=True)
    out, err = {}, {}
    for stream, store in ((p.stdout, out), (p.stderr, err)):
        cur = None
        for line in stream.split("\n"):
            m = re.match(r"@@(\d+)$", line)
            if m:
                cur = int(m.group(1)); store[cur] = []
            elif re.match(r"@@(ret|end)", line):
                m2 = re.match(r"@@ret (-?\d+)", line)
                if m2:
                    store[cur].append(("ret", int(m2.group(1))))
            elif cur is not None and line:
                store[cur].append(("text", line))
    res = []
    for i in range(1, len(vectors) + 1):
        ret = next(v for k, v in out[i] if k == "ret")
        res.append((ret, [v for k, v in out[i] if k == "text"], [v for k, v in err[i] if k == "text"]))
    return res


def run_ours(vectors):
    env = dict(os.environ)
    cmd = ["cargo", "run", "-q", "-p", "maker-connect-engine", "--example", "usbscale_decode", "--"] + [hexs(v) for v in vectors]
    p = subprocess.run(cmd, capture_output=True, text=True, cwd=ROOT, env=env)
    if p.returncode != 0:
        sys.exit("cargo failed:\n" + p.stderr)
    return [l.split("\t") for l in p.stdout.strip().split("\n")]


def g(x):
    """C's %g."""
    return "%g" % x


# ---- the vectors ------------------------------------------------------------------------------

def vectors():
    v = []
    for s in [1, 2, 3, 4, 5, 6, 7, 8, 0, 9, 255]:
        v.append(("status %d" % s, [3, s, 2, 0, 5, 0], "status"))
    for u in list(range(0, 13)) + [13, 255]:
        v.append(("unit %d" % u, [3, 4, u, 0, 5, 0], "unit"))
    for e in [0xFF, 0, 0x80, 0x7F]:
        for w in [0, 1, 0x0100, 0xFFFF]:
            v.append(("exponent %d, weight 0x%04x" % (e - 256 if e >= 128 else e, w),
                      [3, 4, 2, e, w & 255, w >> 8], "weight"))
    for r in [3, 4, 0, 5]:
        v.append(("report ID %d" % r, [r, 4, 2, 0, 5, 0], "report"))
    v.append(("6 bytes", [3, 4, 2, 0, 5, 0], "form"))
    v.append(("7 bytes, leading 00", [0, 3, 4, 2, 0, 5, 0], "form"))
    return v


def classify(label, kind, dat, c, ours):
    """-> (result text, ok): ok False is UNEXPECTED."""
    ret, c_out, c_err = c
    reject = ours[0] == "reject"
    raw = dat[4] | (dat[5] << 8) if len(dat) > 5 else 0
    expo = dat[3] - 256 if dat[3] >= 128 else dat[3]
    if ret == -1 and c_err == ["Error reading scale data"]:
        # usbscale refuses the report and ends
        if reject:
            return "differs, ours (E3/E8): usbscale exits (-1), prints `Error reading scale data`; ours refuses the report, the same text as the problem, and reads on", True
        return "UNEXPECTED: usbscale refuses the report, ours decodes it", False
    if reject:
        return "UNEXPECTED: ours refuses a report usbscale decodes", False
    status_text, unit_name, bits, o_exp, o_raw = ours[1], ours[2], int(ours[3], 16), int(ours[4]), int(ours[5])
    want = struct.pack(">d", float(raw) * math.pow(10, expo))
    if struct.pack(">Q", bits) != want or o_exp != expo or o_raw != raw:
        return "UNEXPECTED: ours decodes weight/exponent/raw otherwise than `raw * pow(10, expt)`", False
    status = dat[1]
    if ret == 0:
        # status 4: usbscale prints "%g <UNITS[unit]>"
        o_line = "%s %s" % (g(struct.unpack(">d", struct.pack(">Q", bits))[0]), unit_name)
        if c_out == [o_line]:
            return "identical", True
        c_weight, _, c_unit = c_out[0].partition(" ")
        if c_weight == g(struct.unpack(">d", struct.pack(">Q", bits))[0]) and dat[2] > 12 and c_unit == "(beyond UNITS)" and unit_name == "unknown unit %d" % dat[2]:
            return "differs, ours (E4): the weight is identical (`%s`); usbscale reads `UNITS[%d]` outside its 13 entries (undefined; the harness pads the table), ours says `%s`" % (c_weight, dat[2], unit_name), True
        return "UNEXPECTED: usbscale printed `%s`, ours `%s`" % (c_out, o_line), False
    if ret == 1:
        if c_err == [status_text] or (c_err == [] and False):
            return "identical", True
        return "UNEXPECTED: usbscale printed `%s`, ours `%s`" % (c_err, status_text), False
    # ret == -1, not a refused report: status 1 or an unknown status
    if status == 1 and c_err == ["Scale reports Fault"] and status_text == "Scale reports Fault":
        return "differs, ours (E3): usbscale exits (-1) at status 1; ours reports the status, the same text, and reads on", True
    # usbscale prints a status only when it differs from `lastStatus`, which starts at 0: status 0 is silent
    said = ["Unknown status code: %d" % status] if status != 0 else []
    if status not in (1, 2, 3, 4, 5, 6, 7, 8) and c_err == said and status_text == "unknown %d" % status:
        return ("differs, ours (E3, E4): usbscale %s and exits (-1); ours reports the status as `%s` and reads on" % (("prints `Unknown status code: %d`" % status) if status else "prints nothing (a status equal to `lastStatus`, which starts at 0)", status_text)), True
    return "UNEXPECTED: usbscale returned -1 with `%s`; ours `%s`" % (c_err, status_text), False


# ---- the Usage Tables ---------------------------------------------------------------------------

STATUS_HUT = {1: ("71", "Scale Status Fault", "Scale reports Fault"),
              2: ("72", "Scale Status Stable at Center of Zero", "Scale is zero'd..."),
              3: ("73", "Scale Status In Motion", "Weighing..."),
              4: ("74", "Scale Status Weight Stable", None),
              5: ("75", "Scale Status Under Zero", "Scale reports Under Zero"),
              6: ("76", "Scale Status Over Weight Limit", "Scale reports Over Weight"),
              7: ("77", "Scale Status Requires Calibration", "Scale reports Calibration Needed"),
              8: ("78", "Scale Status Requires Rezeroing", "Scale reports Re-zeroing Needed!")}
UNIT_HUT = {1: ("51", "Weight Unit Milligram", "mg"), 2: ("52", "Weight Unit Gram", "g"),
            3: ("53", "Weight Unit Kilogram", "kg"), 4: ("54", "Weight Unit Carats", "cd"),
            5: ("55", "Weight Unit Taels", "taels"), 6: ("56", "Weight Unit Grains", "gr"),
            7: ("57", "Weight Unit Pennyweights", "dwt"), 8: ("58", "Weight Unit Metric Ton", "tonnes"),
            9: ("59", "Weight Unit Avoir Ton", "tons"), 10: ("5A", "Weight Unit Troy Ounce", "ozt"),
            11: ("5B", "Weight Unit Ounce", "oz"), 12: ("5C", "Weight Unit Pound", "lbs")}


def hut_table():
    """usage id (hex, upper) -> name, from Table 33.1 of hut1_6.txt."""
    lines = open(HUT, encoding="utf-8").read().split("\n")
    first = next(i for i, l in enumerate(lines) if re.match(r"\s+30\s+Scale Attribute Report\s+CL\s+33.3", l))
    last = next(i for i in range(first, len(lines)) if "Table 33.1: Scales Page" in lines[i])
    table = {}
    for l in lines[first:last]:
        m = re.match(r"\s+([0-9A-F]{2})\s+(Weight Unit[A-Za-z ]*|Scale Status[A-Za-z ]*?)\s{2,}(NAry|Sel|CL)\s", l)
        if m:
            table[m.group(1)] = m.group(2).strip()
    return table


def pos_figure27():
    lines = open(POS, encoding="utf-8").read().split("\n")
    first = next(i for i, l in enumerate(lines) if "Figure 27: Example Scale Data Input Report" in l and "...." not in l)
    rows = {}
    for l in lines[first:first + 14]:
        m = re.match(r"\s+(\d)\s+(Report ID = \d|Scale Status|Weight Unit|Data Scaling|Data Weight LSB|Data Weight MSB)\s*$", l)
        if m:
            rows[int(m.group(1))] = m.group(2)
    return rows


# ---- main --------------------------------------------------------------------------------------

def main():
    function, entries, (l0, l1), weigh_count, weigh_size = extract()
    strings = case_strings(function)
    with tempfile.TemporaryDirectory() as tmp:
        src = os.path.join(tmp, "h.c")
        open(src, "w").write(harness_source(function, entries))
        binary = os.path.join(tmp, "h")
        subprocess.run(["cc", "-O0", "-o", binary, src, "-lm"], check=True)
        vs = vectors()
        # one process per vector: lastStatus is 0 for each (H3)
        c_results = [run_c(binary, [v[1]])[0] for v in vs]
        # the same status twice, then another, then back: usbscale prints a status once per change
        seq = [[3, 2, 2, 0, 5, 0], [3, 2, 2, 0, 6, 0], [3, 3, 2, 0, 5, 0], [3, 3, 2, 0, 5, 0], [3, 2, 2, 0, 5, 0]]
        seq_res = run_c(binary, seq)
    ours = run_ours([v[1] for v in vs])
    rows, bad = [], 0
    for (label, dat, kind), c, o in zip(vs, c_results, ours):
        res, ok = classify(label, kind, dat, c, o)
        bad += 0 if ok else 1
        said = (c[2] or c[1] or ["(nothing)"])[0]
        shown = "%s, returns %d" % (said, c[0])
        o_shown = (("refuses: " + o[1]) if o[0] == "reject" else "`%s` / `%s` / %s" % (o[1] or "(no text)", o[2], g(struct.unpack(">d", struct.pack(">Q", int(o[3], 16)))[0])))
        rows.append((label, hexs(dat), shown, o_shown, res))
    # usbscale prints a status message once per change
    printed = [bool(r[2]) for r in seq_res]
    seq_ok = printed == [True, False, True, False, True] and all(r[0] == 1 for r in seq_res)
    bad += 0 if seq_ok else 1

    # the text of each case, from the function itself
    texts_ok = all(strings.get(s) == STATUS_HUT[s][2] for s in (1, 2, 3, 5, 6, 7, 8)) and strings.get(4) is None
    bad += 0 if texts_ok else 1

    # HUT
    hut = hut_table()
    hut_rows = []
    for s, (uid, name, text) in STATUS_HUT.items():
        ok = hut.get(uid) == name
        bad += 0 if ok else 1
        hut_rows.append(("status %d" % s, "0x%s %s" % (uid, name), "`%s`" % (text if text else "(prints the weight)"), "identical" if ok else "UNEXPECTED: Table 33.1 has `%s`" % hut.get(uid)))
    for n, (uid, name, short) in UNIT_HUT.items():
        ok = hut.get(uid) == name and entries[n] == short
        bad += 0 if ok else 1
        hut_rows.append(("unit %d" % n, "0x%s %s" % (uid, name), "`%s`" % entries[n], "identical" if ok else "UNEXPECTED: Table 33.1 has `%s`" % hut.get(uid)))
    hut_rows.append(("unit 0", "0x50 Weight Unit (the collection, no unit)", "`%s` (usbscale: unknown unit)" % entries[0], "identical" if hut.get("50") == "Weight Unit" and entries[0] == "units" else "UNEXPECTED"))
    bad += 0 if hut.get("50") == "Weight Unit" and entries[0] == "units" else 1
    fig = pos_figure27()
    fig_ok = fig == {0: "Report ID = 3", 1: "Scale Status", 2: "Weight Unit", 3: "Data Scaling", 4: "Data Weight LSB", 5: "Data Weight MSB"}
    bad += 0 if fig_ok else 1

    n_id = sum(1 for r in rows if r[4] == "identical")
    n_diff = sum(1 for r in rows if r[4].startswith("differs"))
    n_bad = sum(1 for r in rows if r[4].startswith("UNEXPECTED"))
    n_hut_id = sum(1 for r in hut_rows if r[3] == "identical")
    result = ("usbscale audit: %d reports: %d identical, %d differs (ours), %d UNEXPECTED; "
              "UNITS and status cases against HUT 33.1: %d of %d identical; Figure 27 layout %s; "
              "status messages once per change %s; usbscale's own strings found %s"
              % (len(rows), n_id, n_diff, n_bad, n_hut_id, len(hut_rows), "identical" if fig_ok else "UNEXPECTED",
                 "confirmed" if seq_ok else "UNEXPECTED", "yes" if texts_ok else "NO"))

    block = [
        "usbscale `%s`: `print_scale_data` (usbscale.c:%d-%d) and `UNITS`, compiled from `reference/usbscale/usbscale.c` by `scripts/usbscale-audit-check.py` "
        "and run (usbscale's return value, its stdout and stderr) beside the Rust decoder (`ports::usbscale::usbscale::print_scale_data`, `crates/engine/examples/usbscale_decode.rs`) on the same six or seven bytes. "
        "`WEIGH_COUNT` = %d (`scales.h:15`), `WEIGH_REPORT_SIZE` = %d." % (COMMIT[:7], l0, l1, weigh_count, weigh_size),
        "",
        "Checker result line: `%s`" % result,
        "",
        "### Reports (a fresh usbscale process each: its static `lastStatus` is 0)",
        "",
        "| Report | Bytes | usbscale (said, returns) | Ours (status text / unit / weight) | Result |",
        "|---|---|---|---|---|",
    ]
    for r in rows:
        block.append("| %s | `%s` | %s | %s | %s |" % (r[0], r[1], r[2], r[3], ("**%s**" % r[4]) if r[4] == "identical" else r[4]))
    block += ["", "### Status messages once per change (one usbscale process, `lastStatus` kept)", "",
              "Reports with status 2, 2, 3, 3, 2: usbscale printed a message for %s of the five (%s). Ours: one event per change of the decoded reading, a repeat is nothing (port test `one_event_per_change_and_nothing_on_no_change`): **differs, ours (E7)**: usbscale prints text once per status change and (status 4) ends; ours reports every changed reading." % (sum(printed), "".join("1" if p else "0" for p in printed)), "",
              "### `UNITS` and the status cases against HID Usage Tables 1.6, Table 33.1", "",
              "| Item | Usage Tables | usbscale | Result |", "|---|---|---|---|"]
    for r in hut_rows:
        block.append("| %s | %s | %s | %s |" % (r[0], r[1], r[2], ("**%s**" % r[3]) if r[3] == "identical" else r[3]))
    block += ["", "HID POS 1.02 section 6.2.4, Figure 27 (Scale Data Report): byte 0 Report ID = 3, 1 Scale Status, 2 Weight Unit, 3 Data Scaling, 4 Data Weight LSB, 5 Data Weight MSB: **%s** with usbscale's `dat[0]..dat[5]`." % ("identical" if fig_ok else "UNEXPECTED")]
    generated = "\n".join(block)

    doc = open(DOC, encoding="utf-8").read()
    new = re.sub(re.escape(BEGIN) + r".*?" + re.escape(END), lambda m: BEGIN + "\n" + generated + "\n" + END, doc, flags=re.S)
    if new != doc:
        open(DOC, "w", encoding="utf-8").write(new)
    print(result)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
