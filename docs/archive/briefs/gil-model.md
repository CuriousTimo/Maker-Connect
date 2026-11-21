# Brief (design, to approve before building): CPython's GIL in the OctoPrint port's runtime

## Why

The proof (`docs/audits/octoprint-marlin.md`) has 0–1 of 24 scenarios identical in all 5 runs. Both
differences come from thread ordering that CPython's GIL makes deterministic and our Rust threads
(truly parallel) do not:

1. At connect, `_onConnected` (comm.py:3717-3729) starts the temperature and SD `RepeatedTimer`s
   (`run_first=True`) before `_changeState(STATE_OPERATIONAL)`. In CPython, `Thread.start`
   (threading.py:956-982) waits on `_started`; the new thread (`_bootstrap_inner`, :1028-1044) sets
   it and keeps the GIL into `run()` → `RepeatedTimer.run` (util/__init__.py:1293-1300) → the first
   `_poll_temperature` sees "not operational" and sends nothing, and only when it blocks in
   `finished.wait` does the starter continue. Ours: `Thread::start` (py_threading.rs:239-257) spawns
   and returns; the poll races the state change and often sends `M105`.
2. After `M112`, `_trigger_emergency_stop` (comm.py:5488-5522) writes and closes without blocking, so
   the monitor never reads the virtual printer's echo (it is woken but needs the GIL; by then the
   connection is closed, virtual.py:2194/2207, comm.py:4178-4192). Ours reads it (the bridge
   process hands the line over at once).

## Proposal

Port the part of CPython's runtime that decides this: the GIL, in `py_threading.rs` (CPython 3.13
is the approved second source for the stdlib behaviour OctoPrint stands on).

- A process-wide GIL (a mutex with a hand-off; no timing of ours). Every thread the port starts
  (`Thread`, `Timer`, `RepeatedTimer`, `ResettableTimer`, the event manager's worker, comm's monitor
  and sending threads) holds it while it runs Python-equivalent code.
- `Thread::start` keeps CPython's handshake: the starter releases the GIL only while it waits on
  `_started`; the new thread takes the GIL, sets `_started`, and runs until it blocks.
- Every blocking point releases the GIL while it waits and takes it back after, exactly where CPython
  does: `Event.wait`, `Condition.wait`, `Lock`/`RLock` acquire, `Queue.get`/blocking `put`,
  `time.sleep`, `Thread.join`, and the serial transport's blocking `read`/`readline`/`write`
  (pyserial's blocking calls release the GIL in CPython's I/O). The engine side (the port's top edge
  talking to the engine's channels) releases it while it waits on a channel.
- Not modelled, written down: CPython's forced switch every 5 ms (`sys.getswitchinterval`) for
  CPU-bound stretches longer than that (none of the paths here run that long).
- The proof harness: `Bridge::call` (proof.rs) releases the GIL while it waits on the pipe; the
  bridge (`scripts/octoprint-vp-bridge.py`) hands a read back so that, as in-process, a line queued
  during a non-blocking stretch of the writer is only seen once the writer blocks (find the
  faithful way; no special case for M112).

## Risks

A missed release is a deadlock (loud, a hang), not a silent difference: every scenario runs with a
timeout; a test of the GIL itself (1000 iterations: a `run_first` timer started before a flag is set
must see the flag unset; `start()` returns only after `_started`).

## Proof that it works

The proof rule unchanged: 24 scenarios × 5 runs, all identical, with the two entries removed from
`known-differences.json`. No change to what comm.rs does.

## Questions for the reviewers

Is porting the GIL the faithful and right fix (vs anything smaller)? Does it break the engine's
own threading (O1: a thread per machine; the engine is ours, not Python — the GIL must cover only
the port's Python-equivalent threads, not the engine or the hub link)? Should the GIL be per port
instance (each machine its own "Python process") rather than process-wide, since one agent may run
several ports? Anything that cannot be made faithful?
