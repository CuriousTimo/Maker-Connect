#!/bin/sh
# The OctoPrint port's tests that need a process of their own: OctoPrint's event manager is global and ends with
# SHUTDOWN, so each of these runs in a fresh process and is `#[ignore]` in the unit suite (`cargo test --workspace`).
#   scripts/octoprint-own-process-tests.sh          the top edge on a fake printer, and K4's unplugged cable
#   scripts/octoprint-own-process-tests.sh proof    the proof against OctoPrint's own code (needs
#                                                   reference/.venv-octoprint-313; see docs/audits/octoprint-marlin.md)
set -e
cd "$(dirname "$0")/.."
export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH
run() {
    cargo test -p maker-connect-engine --lib -- --ignored --exact "ports::octoprint::$1" --nocapture
}
if [ "$1" = proof ]; then
    run proof::tests::proof_against_octoprint
else
    run port::tests::the_top_edge_on_a_fake_printer
    run port::tests::k4_an_unplugged_cable_is_gone
fi
