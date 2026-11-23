#!/usr/bin/env python3
"""The hardware page (#209): checks docs/hardware.json and writes the wiki page Hardware from it.

Every row has one status, and the script refuses a row without its proof:
  verified     proof.date (YYYY-MM-DD), proof.commit (in this repo's history), proof.how
  same-driver  proof.source (the reference's own source), proof.as (a verified model it is driven like)
  audit        proof.audit (a file under docs/audits/)
  planned      proof.issue (a number)

  scripts/hardware-page.py --check          check only (CI)
  scripts/hardware-page.py --wiki FILE      check, then write the wiki page to FILE
"""

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "docs" / "hardware.json"
REPO = "https://github.com/CuriousTimo/Maker-Connect"
LABEL = {
    "verified": "Verified on hardware",
    "same-driver": "Same driver, not tested",
    "audit": "Audit done, waiting for hardware",
    "planned": "Planned",
}


def commit_known(commit):
    run = subprocess.run(
        ["git", "-C", str(ROOT), "cat-file", "-e", f"{commit}^{{commit}}"],
        capture_output=True,
    )
    if run.returncode == 0:
        return True
    shallow = subprocess.run(
        ["git", "-C", str(ROOT), "rev-parse", "--is-shallow-repository"],
        capture_output=True, text=True,
    ).stdout.strip()
    return shallow == "true"  # a shallow clone (CI) cannot tell; the full check runs locally


def check(data):
    errors = []
    verified = {r["model"] for g in data["groups"] for r in g["rows"] if r.get("status") == "verified"}
    for group in data["groups"]:
        for row in group["rows"]:
            where = f'{group["name"]} / {row.get("model", "?")}'
            proof = row.get("proof") or {}
            status = row.get("status")
            for key in ("model", "via"):
                if not row.get(key):
                    errors.append(f"{where}: no {key}")
            if status == "verified":
                if not re.fullmatch(r"\d{4}-\d{2}-\d{2}", proof.get("date", "")):
                    errors.append(f"{where}: verified needs proof.date YYYY-MM-DD")
                if not proof.get("how"):
                    errors.append(f"{where}: verified needs proof.how")
                commit = proof.get("commit", "")
                if not re.fullmatch(r"[0-9a-f]{7,40}", commit) or not commit_known(commit):
                    errors.append(f"{where}: verified needs proof.commit in this repo ({commit!r})")
            elif status == "same-driver":
                if not proof.get("source"):
                    errors.append(f"{where}: same-driver needs proof.source")
                if proof.get("as") not in verified:
                    errors.append(f"{where}: same-driver needs proof.as, a verified model ({proof.get('as')!r})")
            elif status == "audit":
                audit = proof.get("audit", "")
                if not audit.startswith("docs/audits/") or not (ROOT / audit).is_file():
                    errors.append(f"{where}: audit needs proof.audit, a file under docs/audits/")
            elif status == "planned":
                if not isinstance(proof.get("issue"), int):
                    errors.append(f"{where}: planned needs proof.issue, a number")
            else:
                errors.append(f"{where}: unknown status {status!r}")
    return errors


def issue_links(text):
    return re.sub(r"#(\d+)", lambda m: f"[#{m[1]}]({REPO}/issues/{m[1]})", text)


def proof_text(row):
    p = row["proof"]
    s = row["status"]
    if s == "verified":
        return f'{p["date"]}, [{p["commit"]}]({REPO}/commit/{p["commit"]}): {p["how"]}'
    if s == "same-driver":
        return f'Driven like the {p["as"]}: {p["source"]}'
    if s == "audit":
        return f'[{Path(p["audit"]).name}]({REPO}/blob/main/{p["audit"]})'
    return f'[#{p["issue"]}]({REPO}/issues/{p["issue"]})'


def wiki(data):
    out = [
        "<!-- Made by scripts/hardware-page.py from docs/hardware.json; do not edit here. -->",
        "",
        "What Maker Connect works with, and how each claim is proved. Every row has one status, and "
        "the script that makes this page refuses a row without its proof:",
        "",
        "- **Verified on hardware**: tested on the real machine, with the date and the commit.",
        "- **Same driver, not tested**: the reference's own source drives it like a verified model.",
        "- **Audit done, waiting for hardware**: an audit with result identical.",
        "- **Planned**: an issue.",
        "",
        f"The data is [docs/hardware.json]({REPO}/blob/main/docs/hardware.json); "
        "[maker-connect.xyz/hardware](https://maker-connect.xyz/hardware/) shows the same.",
    ]
    for group in data["groups"]:
        out += ["", f'## {group["name"]}', "", "| Model | How it connects | Status | Proof | Note |", "|---|---|---|---|---|"]
        for row in group["rows"]:
            cells = [row["model"], row["via"], f'**{LABEL[row["status"]]}**', proof_text(row), issue_links(row.get("note", ""))]
            out.append("| " + " | ".join(c.replace("|", "\\|") for c in cells) + " |")
    return "\n".join(out) + "\n"


def main():
    args = sys.argv[1:]
    data = json.loads(DATA.read_text())
    errors = check(data)
    if errors:
        print("hardware.json refused:", *errors, sep="\n  ", file=sys.stderr)
        sys.exit(1)
    if args[:1] == ["--wiki"] and len(args) == 2:
        Path(args[1]).write_text(wiki(data))
    elif args != ["--check"]:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    rows = sum(len(g["rows"]) for g in data["groups"])
    print(f"hardware.json: {rows} rows, every one with its proof")


if __name__ == "__main__":
    main()
