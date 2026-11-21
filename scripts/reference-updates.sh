#!/usr/bin/env bash
# What changed upstream in each reference since the commit our port is pinned to, limited to the
# files that port is a translation of. Read only: it fetches, it changes nothing in the repo or
# in the ports. Run from the repository root: scripts/reference-updates.sh
#
# One line per reference: directory | our pin | the upstream branch or tag pattern to compare
# with | the paths the port translates. Keep it in step with each port's header.
set -u
REFS=(
  "reference/OctoPrint-1.11.8|1.11.8|tags:1.11.*|src/octoprint/util/comm.py src/octoprint/printer/standard.py src/octoprint/printer/profile.py src/octoprint/settings/__init__.py src/octoprint/schema/config src/octoprint/events.py src/octoprint/util/__init__.py src/octoprint/util/files.py src/octoprint/util/text.py src/octoprint/filemanager/__init__.py src/octoprint/plugins/action_command_prompt src/octoprint/server/api/printer.py src/octoprint/server/api/job.py src/octoprint/server/api/files.py src/octoprint/plugins/virtual_printer"
  "reference/labelle|5e8f1e5|origin/main|src/labelle/lib/devices src/labelle/lib/constants.py src/labelle/lib/utils.py src/labelle/lib/render_engines src/labelle/cli/cli.py"
  "reference/laserfoundry|v0.6.0|tags:v*|crates/device crates/gcode crates/ui-egui/src/lib.rs crates/ui-egui/src/settings_ui.rs"
)
# Not git checkouts here (copies of single files): check their newer patch releases by hand.
COPIES="reference/cpython-3.13 (CPython 3.13.x), reference/pillow-12.3.0 (Pillow 12.x), pyserial 3.5 and pyusb 1.3.1 in the venvs"

for entry in "${REFS[@]}"; do
  IFS='|' read -r dir pin upstream paths <<<"$entry"
  echo "== $dir (pinned to $pin)"
  if ! git -C "$dir" fetch -q --tags origin 2>/dev/null; then
    echo "   could not fetch; skipped"; continue
  fi
  case "$upstream" in
    tags:*)
      newest=$(git -C "$dir" tag -l "${upstream#tags:}" --sort=-v:refname | head -1)
      target=${newest:-$pin} ;;
    *) target=$upstream ;;
  esac
  if [ "$(git -C "$dir" rev-parse "$pin")" = "$(git -C "$dir" rev-parse "$target")" ]; then
    echo "   up to date ($target)"; continue
  fi
  echo "   newest: $target ($(git -C "$dir" log -1 --format='%h %ad' --date=short "$target"))"
  # shellcheck disable=SC2086
  changes=$(git -C "$dir" log --format='   %h %ad %s' --date=short "$pin..$target" -- $paths)
  if [ -z "$changes" ]; then echo "   no change in the ported files"; else echo "$changes"; fi
done
echo "== by hand: $COPIES"
