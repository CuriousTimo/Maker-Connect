# MK3: PrusaLink's websockets branch as the reference

Date: 2026-11-19  
Status: accepted; places #4; moves the pin of [mk3-port-plan.md](../../archive/plans/mk3-port-plan.md) §1  
Decided by: owner ("dit is toch de referentie die we zochten voor websocket voor de mk3 in onze agent")

## Context

#4 (the MK3 family) was parked until PrusaLink speaks the WebSocket to Prusa Connect
(prusa3d/prusa-connect#9). The MK3 port plan pinned PrusaLink master `c453996d` and SDK `3d21ae59`,
both of which talk HTTP to Connect.

PrusaLink now has that WebSocket on a branch Prusa maintains:

- PrusaLink, branch `websockets`, at `171148ce7dbc7f1fb0633ea519523f3258877e2e` (2026-11-05). It is 71
  commits ahead of master and is mostly the work of PrusaLink's maintainer. Its `requirements.txt:1` takes
  the SDK `@websockets`. Version: `__version__ = "1.0.0.dev0"` (`prusa/link/__init__.py:8`).
- Connect SDK (printer), branch `websockets`, at `c0d7d233d4ce2081e6c020d2e98fb92ad9b51521`
  (2026-11-03). Telemetry and events go over the WebSocket (`prusa/connect/printer/ws.py`, new). It also
  has the lazy file protocol (`fileinfo.py`). Version: `__version__ = "0.9.0dev0"`
  (`prusa/connect/printer/__init__.py:39`).
- Against master, `prusa/` differs by +331/−105 lines in 18 files (PrusaLink) and by +609/−228 lines in
  6 files (SDK).

## Decision

- The MK3 port follows this pair, to the letter. Every ported file names it as its source.
- These are not the reference:
  - The SDK branch `feat/ws-connect-local` (`04c5bd59`, 2026-11-17, 4 commits ahead): inline transfer and
    reconnect for Connect local. It becomes the reference once PrusaLink pins it.
  - PrusaLink's `xbuddy-websocket-bridge` (`2a0a333`, 2026-05-20): one experiment, not continued.
  - `printer-simulator-dont-delete` (2023, 232 behind master): at most a test aid.
- The MK3 is the `prusa` kind on the agent. Its serial side is PrusaLink's own, not the OctoPrint port
  (no fusing). Order: after agent v2 (#12).
- How the MK3 port reaches the hub is open: [OQ-45](../risks-and-open.md#oq-45-how-the-mk3-port-reaches-the-hub).

## Where it shows

[README.md](../README.md) (#4 row), [links.md](../links.md), [risks-and-open.md](../risks-and-open.md),
[mk3-port-plan.md](../../archive/plans/mk3-port-plan.md) §1 and §3.4.
