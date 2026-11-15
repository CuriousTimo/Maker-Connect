"""`octoprint-port-check.py compare`: reads the reference run's fixtures
(crates/engine/tests/octoprint/reference/) and ours (ours/, written by the Rust test
`ports::octoprint::proof::tests::proof_against_octoprint`), compares them on its own with the same
rules as the Rust test (the two must agree: the numbers of `ours/result.json` are checked against
these), and writes the generated tables of docs/audits/octoprint-marlin.md between the markers.
Exit 0 only when the two comparisons agree; the audit's "Result" says where ours differs."""

import difflib
import json
import os
import sys

BEGIN, END = "<!-- generated: begin -->", "<!-- generated: end -->"
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DOC = os.path.join(ROOT, "docs", "audits", "octoprint-marlin.md")


def load(path):
    with open(path) as f:
        return json.load(f)


def short(line, n=60):
    line = line.rstrip("\n")
    return line if len(line) <= n else line[: n - 1] + "..."


def tick(lines):
    return ", ".join(f"`{short(x)}`" for x in lines) if lines else "nothing"


def first_difference(reference, ours, what):
    """a sentence for the first place two lists of lines differ"""
    ref = [x.rstrip("\n") for x in reference]
    our = [x.rstrip("\n") for x in ours]
    sm = difflib.SequenceMatcher(a=ref, b=our, autojunk=False)
    for tag, i1, i2, j1, j2 in sm.get_opcodes():
        if tag == "equal":
            continue
        after = f" after `{short(ref[i1 - 1])}`" if i1 else " at the start"
        n = sum(1 for t in sm.get_opcodes() if t[0] != "equal")
        more = f" (and {n - 1} more place{'s' if n > 2 else ''})" if n > 1 else ""
        if tag == "insert":
            return f"ours {what} {tick(our[j1:j2])}{after}; the reference nothing{more}"
        if tag == "delete":
            return f"the reference {what} {tick(ref[i1:i2])}{after}; ours nothing{more}"
        return f"ours {what} {tick(our[j1:j2])} where the reference {what} {tick(ref[i1:i2])}{after}{more}"
    return None


# The differences measured, named by what ours writes that the reference does not. A name is a description of
# a measured difference, no rule: nothing is dropped or changed by it. None is measured now (a scenario that
# differs shows as "?", unclassified, and its first run is listed below).
FAMILIES = {}


def family(signature):
    """"" for an identical run; else the name of what differs"""
    writes = tuple(signature["writes"])
    if signature["error"] or signature["reads"] or writes:
        return FAMILIES.get(writes, ("?", "unclassified"))[0]
    return ""


def main(top, fix, normalise):
    scenarios = top["scenarios"]
    summary = load(os.path.join(fix, "reference", "summary.json"))
    rust = load(os.path.join(fix, "ours", "result.json"))
    rows, details, totals = [], [], {"waits_ref": 0, "waits_ours": 0, "masked_ref": 0, "masked_ours": 0}
    problems = []
    fam_runs, fam_scn, fam_example = {}, {}, {}
    counts = {"identical": 0, "differs": 0, "varies": 0, "not reproducible": 0}
    for scn in scenarios:
        name = scn["name"]
        ref = load(os.path.join(fix, "reference", f"{name}.json"))
        runs = load(os.path.join(fix, "ours", f"{name}.json"))["runs"]
        reproducible = summary[name]["reproducible"]
        ref_reads = normalise(scn, ref["reads"])
        n_wait_ref = sum(1 for x in ref["reads"] if x.strip() == "wait")
        totals["waits_ref"] += n_wait_ref
        whole = writes_same = reads_same = 0
        first_bad = None
        for r in runs:
            our_reads = normalise(scn, r["reads"])
            n_wait = sum(1 for x in r["reads"] if x.strip() == "wait")
            totals["waits_ours"] += n_wait
            w, rd, e = r["writes"] == ref["writes"], our_reads == ref_reads, r["error"] == ref["error"]
            writes_same += w
            reads_same += rd
            if w and rd and e:
                whole += 1
            elif first_bad is None:
                first_bad = (r, our_reads, w, rd, e)
        verdict = "identical" if whole == len(runs) else "differs" if whole == 0 else "varies"
        # cross-check with the Rust test's own comparison
        rr = rust[name]
        if (rr["verdict"], rr["identical_runs"], rr["writes_identical_runs"], rr["reads_identical_runs"]) != (verdict, whole, writes_same, reads_same):
            problems.append(f"{name}: the Rust test says {rr['verdict']} {rr['identical_runs']}/{rr['writes_identical_runs']}/{rr['reads_identical_runs']}, this script {verdict} {whole}/{writes_same}/{reads_same}")
        if scn.get("temperatures_moving"):
            import re

            rx = re.compile(r"(?<![A-Za-z0-9_])([TB]\d*:)\s*-?\d+(?:\.\d+)?")
            totals["masked_ref"] += sum(len(rx.findall(x)) for x in ref["reads"])
            totals["masked_ours"] += sum(len(rx.findall(x)) for r in runs for x in r["reads"])

        n = len(runs)
        wc = len(ref["writes"])
        ref_text = f"{wc} writes, {len(ref_reads)} lines read" + (f"; ended: {short(ref['error'], 50)}" if ref["error"] else "")
        if not reproducible:
            result = "**not reproducible** (the reference differs between its own runs): not compared"
            counts["not reproducible"] += 1
        elif verdict == "identical":
            result = f"**identical** ({n} of {n} runs)"
            counts["identical"] += 1
        else:
            names = {}
            for sig in rust[name]["signatures"]:
                label = family(sig["signature"])
                if label:
                    names[label] = names.get(label, 0) + sig["runs"]
                    fam_runs[label] = fam_runs.get(label, 0) + sig["runs"]
                    fam_scn.setdefault(label, set()).add(name)
                    fam_example.setdefault(label, sig["signature"])
            listing = ", ".join(f"{k} ({v})" for k, v in sorted(names.items()) if k)
            head = "**differs**" if verdict == "differs" else f"**varies** ({whole} of {n} runs identical)"
            result = f"{head}: {listing}"
            counts["differs" if verdict == "differs" else "varies"] += 1
        rows.append(f"| `{name}` | {scn['what']} | {ref_text} | writes {writes_same}/{n}, lines {reads_same}/{n} | {result} |")

    out = []
    out.append(
        f"{len(scenarios)} scenarios, each run {len(runs)} times on our side and {5} times on the reference's side (CPython 3.13.12): "
        f"**{counts['identical']} identical**, {counts['differs']} differ, {counts['varies']} vary from run to run, "
        f"{counts['not reproducible']} not reproducible (reference)."
    )
    out.append("")
    out.append("| Scenario | What it does | Reference (OctoPrint's own code, run) | Ours: runs identical in each layer | Result |")
    out.append("|---|---|---|---|---|")
    out.extend(rows)
    out.append("")
    if fam_runs:
        out.append("The differences measured, by name (a name describes a measured difference; it drops or changes nothing). Each is listed with the lines of the first run that had it (`+` ours only, `-` the reference only):")
        out.append("")
        out.append("| Name | Runs | Scenarios | What differs |")
        out.append("|---|---|---|---|")
    else:
        out.append("No difference is measured in a scenario the reference reproduces.")
    for label in sorted(fam_runs):
        ex = fam_example[label]
        what = next((d for k, (l, d) in FAMILIES.items() if label.split("+")[0] == l), "")
        lines = "; ".join(f"writes {tick(ex['writes'])}" if ex["writes"] else "") 
        reads = ", ".join(f"`{short(x, 40)}`" for x in ex["reads"][:6]) + (" ..." if len(ex["reads"]) > 6 else "")
        out.append(f"| {label} | {fam_runs[label]} | {len(fam_scn[label])}: {', '.join(sorted(fam_scn[label])[:5])}{' ...' if len(fam_scn[label]) > 5 else ''} | {what} Writes: {', '.join('`' + short(x, 30) + '`' for x in ex['writes'])}. Lines read: {reads} |")
    out.append("")
    out.append("Normalisation counts (lines dropped, summed over the scenarios; the reference's fixture runs, and our runs):")
    out.append("")
    out.append(f"- `wait` lines dropped: reference {totals['waits_ref']}, ours {totals['waits_ours']} (the virtual printer's `sendWait` is off, so it sends none)")
    out.append(f"- numbers behind `T:`/`B:` masked (only in a scenario with `temperatures_moving`): reference {totals['masked_ref']}, ours {totals['masked_ours']}")
    out.append("")
    out.append("Settings changed from OctoPrint's and the virtual printer's defaults (the same on both sides; `scenarios.json`):")
    out.append("")
    out.append("| Setting | Value | Why |")
    out.append("|---|---|---|")

    def walk(prefix, value):
        if isinstance(value, dict):
            for k, v in value.items():
                yield from walk(prefix + [k], v)
        else:
            yield ".".join(prefix), value

    for side in ("virtual_printer", "octoprint"):
        for path, value in walk([side], top.get(side, {})):
            out.append(f"| `{path}` | `{json.dumps(value)}` | {top['why'].get(path, '')} |")
    for scn in scenarios:
        for side in ("virtual_printer", "octoprint"):
            for path, value in walk([side], scn.get(side, {})):
                out.append(f"| `{path}` (scenario `{scn['name']}` only) | `{json.dumps(value)}` | the printer must report that capability for the prompt plugin to show a prompt (`action_command_prompt`: `enable` is `detected`) |")
    # the ported OctoPrint tests, by name (each Python test function must have a Rust test of the same name)
    import re

    with open(os.path.join(ROOT, "crates", "engine", "src", "ports", "octoprint", "comm.rs")) as f:
        rust_fns = set(re.findall(r"\n\s*fn (\w+)\s*\(", f.read()))
    tests_line = []
    for fname in ("test_comm.py", "test_comm_helpers.py"):
        with open(os.path.join(ROOT, "reference", "OctoPrint-1.11.8", "tests", "util", fname)) as f:
            names = re.findall(r"^\s*def (test\w*)\(", f.read(), re.M)
        missing = [n for n in names if n not in rust_fns]
        tests_line.append(f"`tests/util/{fname}`: {len(names) - len(missing)} of {len(names)} test functions have a Rust test of the same name in `comm.rs`" + (f" (missing: {', '.join(missing)})" if missing else ""))
    out.append("")
    out.append("Ported OctoPrint tests, checked by name: " + "; ".join(tests_line) + ".")
    block = "\n".join(out)

    with open(DOC) as f:
        doc = f.read()
    a, b = doc.index(BEGIN) + len(BEGIN), doc.index(END)
    with open(DOC, "w") as f:
        f.write(doc[:a] + "\n" + block + "\n" + doc[b:])
    print(block.splitlines()[0])
    for p in problems:
        print("DISAGREE:", p)
    return 1 if problems else 0
