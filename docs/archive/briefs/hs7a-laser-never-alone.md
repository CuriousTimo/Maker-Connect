# Brief: HS7a — a laser job never starts by itself; remove "Laser off"

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust; git repo, never commit unless told; no
hardware). PATH: `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`. The owner
released the hub side ("merge. dan de hub-kant", 2026-11-15); this is its first step.

The owner's rule (memory `maker-connect-laser-never-unattended`; `docs/plans/agent-engine.md` "a laser
never starts unattended"): a laser job is started only by a person. Nothing of this reaches the
wire: `add_job` is the hub's own database insert.

1. **The guard.** `store.add_job` (`store.rs` ~915) is the one insert into `jobs`. Give it a
   required parameter, e.g. `StartedBy::Person { user, session }` / `StartedBy::Auto`, and update
   every caller. For a laser, `Auto` is refused, and so is `Person` without a session (`Access` has
   `session_id`/`token_id`, `access.rs` ~103-115). The callers:
   - `queue.rs` ~258 `start_queued`: Auto.
   - `queue.rs` ~117 "Reprint on the printer" (idle + last file fits → the hub starts a job): Auto.
   - `farm.rs` ~628 `dispatch`: Auto.
   - `files.rs` ~708 `print`: Person (it checks `Level::Use` + `check_ppe`, ~664-672).
   - `files.rs` ~830 the "restart" (Start again from there): Person (it checks PPE, ~765).
   - `simplyprint.rs` ~509: Klipper (not a laser).
   A refused laser in `start_queued` / `dispatch` / the reprint is skipped silently, never counted
   as started, and not retried in a hot loop. Refusal text (ours, UI): "a laser starts only when
   someone at the laser starts it". No new permission: a person start is as `print` does today
   (Use + PPE + a session); "operate" for a laser Start belongs to HS7b.
2. **Laser copies wait.** `files.rs` ~684-700 queues copies 2–50; after this nothing starts them
   (no person start from the queue exists until HS7b). Say so where the queue is shown: "waits for
   someone at the laser". Add no button.
3. **"Laser off" goes entirely** (no users yet, no compatibility to keep — owner 2026-11-15): the
   button and its hint text (`machine_page.rs` ~1396), `main.rs` ~1205 and ~1226
   (`"laser-off" => ("Laser off","M5")`), ~1402 (its entry in the "never waits for PPE" list), the
   test `access.rs` ~3051-3072, the `LaserOff` variant (`protocol/src/lib.rs` ~506), the
   laserfoundry port's handling (`engine/src/ports/laserfoundry/port.rs` ~113, `tests.rs` ~95/121)
   and the OctoPrint port's match arm (`octoprint/port.rs` ~618). No adapters.
4. **Stop on an idle laser (G6):** find whether the laser port has a stop that works with no job
   (LaserFoundry's Stop is a soft reset). If yes, show Stop on the laser's machine page also with no
   job (`machine_page.rs` ~661, `data-job-actions`) and test it; if the port has no job-less stop,
   report it and change nothing (a separate brief).
5. **Tests:** `start_queued`, the reprint and `dispatch` never start a laser job; `print` and
   "restart" do, with a session + Use + PPE; an API token on `print` for a laser is refused (check
   whether tokens reach it); laser copies wait; FDM machines keep their queue, reprint and farm
   behaviour; "Laser off" is gone.

`cargo test --workspace` must pass; no new warnings. Report briefly: what changed (file:line), tests,
and point 4's finding.
