# PrusaLink port (MK3 family, agent): audit

The one reference is PrusaLink (`reference/Prusa-Link`, branch `websockets`, `171148ce`) with its
Connect SDK (branch `websockets`, `c0d7d233`), ported section by section
(`docs/archive/plans/mk3-port-plan.md` §4). Each section is proved by running PrusaLink's own code
and ours on the same examples (`crates/engine/src/ports/prusalink/driver_*.py`, run by the
section's Rust test with `reference/.venv-prusalink`, Python 3.9.6, here, and with CPython 3.13
in CI's PrusaLink job).

## A0: `config.py` and extendparser's `Get` (#241)

Ours: `crates/engine/src/ports/prusalink/config.rs`, on CPython 3.13.12's `configparser`
(`crates/connect-sdk/src/configparser.rs`; `BasicInterpolation`, `getboolean`, `ConfigParser.set`'s
check, `in` and `parser[section].items()` added for it).

Proof: `config/tests.rs` `config_and_settings_are_prusalinks` runs 34 examples through
`driver_config.py` (the real `Config` and `Settings`) and through ours: every model value's type
and `str()`, `log_settings`, the log handler's kind and format, what `update_sections` and `write`
give (or its error), the settings file written, `is_wizard_needed`, `use_connect`, and each
exception's class and text. **Result: 34 of 34 identical.** The examples include a missing and an
empty file, overrides from the arguments, Python `int()` (`+8_080`, `1_`), `getboolean`
(`TRUE`, `maybe`), `%(name)s` from `[DEFAULT]`, `%%`, a missing name, bad syntax, the depth
limit, an unsupported printer type, a parsing error and a `%` that `update_sections` refuses.

What the MK3 port reads (the call paths in `printer_adapter/`, `sdk_augmentation/`,
`conditions.py`, `util.py`): `cfg.printer.{port, baudrate, reset_disabling, settings, directory,
storage}`, `cfg.daemon.{threshold_file, power_panic_file, printer_number}`, `cfg.http.port`,
`cfg.cameras.auto_detect`, `settings.printer.{type, network_error_chime}`,
`settings.service_connect.{hostname, port, tls, token}`, `settings.service_local.api_key`,
`settings.use_connect()`, `is_wizard_needed()`, `update_sections()`. The rest is read by the web
side and `multi_instance/` only. `cfg.printer.storage` is not set by `config.py` (its option is
commented out, `:184`): a read is `AttributeError`, as in PrusaLink.

**Not covered:** `set_global_log_level` and the log handler itself (Python's `logging`; what
`get_log_handler` reads from the file is covered); `Settings.instance` (one per process in
PrusaLink, one per printer module here, #218); `bidict` (only `in` of `PRINTER_CONF_TYPES`'s keys
is used); `get_option`'s `list`, `tuple` and `set` targets (unused); `pwd` on Windows (an error
names the call); a `SysLogHandler` that cannot connect (the examples run in the foreground); an
`OSError`'s text (its `repr` is given).

## A: constants and data (#242)

Ours: `crates/engine/src/ports/prusalink/`, one file per PrusaLink file: `consts.rs` (`const.py`;
`PRINTER_CONF_TYPES` moved here from `config.rs`), `regular_expressions.rs`
(`printer_adapter/structures/regular_expressions.py`, compiled with `fancy_regex`),
`model_classes.rs` (`printer_adapter/structures/model_classes.py`, pydantic 1.10.13 models as
structs), `util.rs` (`util.py` `:105-250`, `:300-351`; `get_gcode` and `get_print_stats_gcode`
moved here from `print_stats.rs`; `loop_until`, `:51-73`, which `updatable.py` needs),
`module_data_classes.rs`, `mc_singleton.rs` (one per printer module, as the agent hosts several,
#218), `heap.rs`, `instruction.rs` (`serial/instruction.py`; a `re.Match` as the groups it holds),
`updatable.rs` (`printer_adapter/updatable.py`; `threading` is the CPython port `ports/python/py_threading.rs`, moved there from `ports/octoprint` as
the plan's section 0 asks).
The libraries of the plan's §5, ported as PrusaLink uses them: `pydantic.rs` (`deep_update`,
pydantic 1.10.13), `sortedcontainers.rs` (`SortedKeyList`, sortedcontainers 2.4.0), `blinker.rs`
(`Signal`, blinker 1.9.0).

Proof: `driver_constants.py` imports PrusaLink's own module, lists every name its source defines at
the top level (from the AST: assignments, annotated assignments, classes), and gives each value as
Python built it (a regex as its pattern and flags, an enum class as its members in order, a set
sorted, a dict in its order). The Rust tests make the same form from ours and compare name by name,
the names in the source's order.

- `consts/tests.rs` `identical_to_prusalink`: **Result: 90 of 90 names identical** (`DATA_PATH`
  only in that it ends in `prusa/link/data`).
- `regular_expressions/tests.rs` `identical_to_prusalink`: **Result: 68 of 68 names identical**,
  each pattern character for character, flags `re.UNICODE`.
- `model_classes/tests.rs` `enums_identical_to_prusalink`: the 11 names, and the 6 enums' members
  in order with their values. **Result: identical.** `dicts_identical_to_prusalink`: 26 models made
  with the same fields (none, some, all; nested `slot` and `slots`) and `dict()` with and without
  `exclude_none`, compared as JSON text (key order included). **Result: 26 of 26 identical.**
- `util/tests.rs` `identical_to_prusalink`: 111 calls through `driver_util.py` (the real functions)
  and ours, the result or the exception's class and text: `get_clean_path` (20 paths: `//`, `///`,
  `.`, empty), `get_gcode`, `file_is_on_sd`, `make_fingerprint`, `fat_datetime_to_tuple` (each of
  `datetime`'s checks, leap years), `get_print_stats_gcode`, `get_d3_code` (both refusals, a
  negative address), `round_to_five` (half to even), `decode_line` (all 256 bytes of `cp437`, NUL,
  Python's whitespace), `walk_dict`, `slots_with_param`, `_parse_little_endian_uint32`
  (`bytes.fromhex` and `struct` errors), `power_panic_delay` (no file, `using_rip_port`, a field
  missing: the class only), `ensure_directory` (new, existing, nested, a file in the way, under a
  file: `NotADirectoryError`), and `is_potato_cpu` on the machine. **Result: 111 of 111 identical.**
- `util/tests.rs` is joined by `updatable/tests.rs` `identical_to_prusalink`: `loop_until`'s calls
  in order until its event is set (`to_run`, `run_every_sec`, three rounds), and a
  `ThreadedUpdatable` that stops itself at its third `update`: three updates, its thread ended,
  `thread_name` and `update_interval` the same. **Result: identical.**
- `module_data_classes/tests.rs` `identical_to_prusalink`: 18 models of the 10 classes made with the
  same fields (the defaults from only the required ones; states, sets, ordered dicts), `dict()` with
  and without `exclude_none` compared as JSON text, `Port.__str__`, `JobData.get_job_id_for_api()`
  (idle and not). **Result: 18 of 18 identical.**
- `mc_singleton/tests.rs` `identical_to_prusalink`: `get_instance()` before, the first instance,
  after, a second (`RuntimeError` and its text), after, within one cell. **Result: identical
  within a printer module; differs in its scope:** one instance per class per printer module, not
  per process, as the agent hosts several printers (#218; as `Settings.instance`, `config.rs`;
  proxy, 2026-11-27: approved). Two printer modules each have their own `Job`, `Model`, ...; nothing
  in PrusaLink is shared between printers, as it hosts one.
- `heap/tests.rs` `identical_to_prusalink`: 800 operations on PrusaLink's `MinHeap` and `MaxHeap`
  (equal values, pops by `heap_index` anywhere, by plain index, past the end): after each, the result
  and every item's place, `heap_value` and `heap_index`. **Result: 800 of 800 identical.**
- `instruction/tests.rs` `identical_to_prusalink`: 180 instructions (the 3 classes, 10 messages:
  spaces, empty, beyond ASCII, a newline; numbered with a checksum or not; no capture, forced, two
  captures): `str`/`repr`, `fill_data`'s bytes and number or the `UnicodeEncodeError`'s text,
  `match`, `get_matches`, `capture_matching`, sending, confirming (refused without a capture),
  `reset`; `get_checksum` on 4 byte strings, all 256 bytes among them. **Result: 180 of 180
  identical.**
- `pydantic/tests.rs` `deep_update_identical_to_pydantic`: 11 mappings (nested, replaced by a value
  and the reverse, several updates, `None`, lists), compared as JSON text. **Result: 11 of 11
  identical.**
- `sortedcontainers/tests.rs` `identical_to_sortedcontainers`: 600 `add`, `remove` (also of one not
  there: the `ValueError`'s text), `in`, with a load factor of 4 and many equal keys: after each, the
  result, the order and the sublists. **Result: 600 of 600 identical.**
- `blinker/tests.rs` `identical_to_blinker`: 400 operations on a `Signal` (`connect` weak and strong,
  `disconnect`, a weak receiver's last reference dropped, `send`, `len(receivers)` and its truth; a
  receiver that disconnects itself and one that connects another while sending): after each, the
  receivers called with their sender and the count. **Result: 400 of 400 identical in which
  receivers are called, with what, and the count; differs in the order of the calls:** the source
  calls them in the order of a set of ids (`receivers_for`, `base.py:326-362`; "The order receivers
  are called is undefined", `send`), which follows memory addresses, ours in the order of
  connecting (proxy, 2026-11-27: approved; the reference's order depends on addresses and cannot be
  reproduced). Where it shows: `byte_position.value_changed_signal` has three receivers,
  `_get_progress_from_byte_position` (`printer_polling.py:320`), `_guess_time_remaining`
  (`:330`, it reads the `progress_from_bytes` the first sets) and `byte_position_changed`
  (`prusa_link.py:347`): ours calls them in that order, the reference in an order of its run. The
  calls are compared sorted for that reason.

**Not covered:** `instance_id` (a random `uuid4()` read only by `web/main.py` and
`service_discovery.py`); `DATA_PATH`'s value (where the package lies); `MINIMAL_FIRMWARE`'s
`packaging.version.Version` (the text is the same; the parse comes with its one user,
`printer_polling.py:878`, section F1); Python's `$` before a final `\n` (PrusaLink matches lines after
`decode_line`'s `.strip()`, so none ends in `\n`); pydantic's validation when a model is made (a
value of another type converted or refused: here each field takes its own type, and the callers'
ports convert as pydantic does; `power_panic_delay` reads what PrusaLink itself wrote, and the text
of pydantic's `ValidationError` is not compared); `slots_with_param` (no caller) with a value of
another type than the field's; `power_panic_delay`'s wait of `PP_MOVES_DELAY` (20 s; not run) and
its `log.info`; `ensure_directory`'s `chown` (needs root) and `persist_file` (`flush` and `fsync`:
nothing to compare); `pwd`/`chown` on Windows (Linux first, owner 2026-11-27); `prctl_name` and
`Thread`'s profiling (the system thread name, cProfile); pydantic's `dict()` of a `set` (here sorted)
and `FilePrinterData.dict()` (its deque holds instructions; only `Port.dict()` has a caller,
`web/settings.py:78`); a plain `Instruction`'s `match` (an `AttributeError` there); the `log`
calls (kept as comments); blinker's sender filter, `receiver_connected`/`receiver_disconnected`,
`muted`, `send_async` and `Namespace` (PrusaLink connects to `ANY` only and uses none of them).

## B1: the serial port, its parser and the helpers (#243)

Ours: `crates/engine/src/ports/prusalink/`: `serial.rs` (`serial/serial.py`, PrusaLink's own termios
port, not pySerial; Linux only, as the reference: `termios.IUCLC` does not exist on macOS, where
PrusaLink's `Serial()` raises `AttributeError`), `serial_parser.rs` (`serial/serial_parser.py`),
`helpers.rs` (`serial/helpers.py`; `SerialQueue` is only the two methods the helpers call, until
section B2). `queue.Queue` is the CPython port `ports/python/py_queue.rs`, moved there from
`ports/octoprint` as the plan's section 0 asks.

- `serial/tests.rs` `identical_to_prusalink`: 14 scripts on PrusaLink's `Serial` and ours, each
  over its own pseudo terminal: the terminal's attributes after the open (`tcgetattr`: the four
  flag words, both speeds, every `cc`), or the open's exception (baudrate 9600; a missing path, a
  regular file, a directory); then 6 rounds of 40 ops from the printer's end and the port's
  (lines sent whole, in two pieces, with CR LF, XON/XOFF and bytes over 0x7f; `readline`,
  `write` and what the other end received, `dtr` set, `is_open`; the other end hanging up, the
  port closed, and what follows), each op's value or the exception's class and text.
  **Result: 14 of 14 identical** (in a Debian container with CPython 3.13: the module does not
  build on macOS).
- `serial_parser/tests.rs` `identical_to_prusalink`: 500 operations on `SerialParser` (adds with
  equal, raised, lowered and infinite priorities, the same handler twice, removals of what is there
  and not, lines that match one pattern, several or none): after each, the result (the
  `RuntimeError`'s text), the handlers called with the match's groups, and `pattern_list` in order
  with each pairing's `str`; and `RegexPairing`'s `str` at 6 priorities with 0, 1 and 2 handlers.
  **Result: 500 of 500 and 18 of 18 identical, except the order of the handlers called** (blinker's
  `send`, section A: compared sorted).
- `helpers/tests.rs` `identical_to_prusalink`: 70 calls of `enqueue_instruction`,
  `enqueue_matchable` and `enqueue_list_from_str` (`to_front`, `to_checksum`, `has_to_match`, an
  empty message, one beyond ASCII, a newline in a message, first, in the middle and last of a list,
  an empty list): what the queue was given and what the call returned (class, `str`,
  `to_checksum`, `capture_matching`, `capturing_regexps`; the `RuntimeError`'s text), and whether
  those are the same objects; 7 waits of `wait_for_instruction` (confirmed at once or later,
  `should_wait` running out, the event set): the result and how often it asked the instruction.
  **Result: 70 of 70 and 7 of 7 identical.**
- PrusaLink's own tests, ported test by test: `test_serial.py` (11, over a pseudo terminal; its
  `patch.object` as an `Os` of the test's), `test_serial_parser.py` (5) with `test_serial_parser_handlers.py` (11), and
  `test_serial_helpers.py` (7): **all pass.**

**Not covered:** the `log` calls (kept as comments); `Serial` on macOS and Windows (not in the
reference: Linux first, owner 2026-11-27); a whole `float` priority (ours writes `2.0` as `2`;
PrusaLink's priorities are `time()` and `inf`); `SerialParser`'s `MCSingleton` scope (one per printer
module, as section A); its matching with `fancy_regex`, not `re`, compared only on the test's
patterns and lines; a `SerialParser` handler that raises (a Rust handler
has no exception: a panic, `serial_parser.rs` difference 1); the parser's thread is compared only
by PrusaLink's own tests.

## B2: the conditions (#244)

Ours: `crates/engine/src/ports/prusalink/conditions.rs` (`conditions.py`), on the SDK's conditions
(`crates/connect-sdk/src/conditions.rs`, the SDK's `conditions.py`, proved by the SDK's own
`tests/test_conditions.py` and `test_errors.py` there). PrusaLink's tree under `ROOT_COND`, its
trackers and the status functions are one `Conditions` per printer module; a `LinkError` class is an
`ErrorClass` (the class attributes as Python resolves them through the bases, taken from PrusaLink's
module by a script, in the source's order), an instance a `LinkError`.

- `conditions/tests.rs` `identical_to_prusalink`: the 51 classes (`LinkError` and its 50
  subclasses, in the source's order from its AST): the base, `title`, `text`, `id`, `status_code`,
  `use_basic_template`, an instance without and with details (`details`, `path`, `template`, `args`,
  `url`, `gen_headers()` before and after a url; for a class without a `text` the `AttributeError`).
  **Result: 51 of 51 identical.** Then the tree as imported and through 219 ops (a condition of
  PrusaLink's tree or of the SDK's chain set OK, NOK or back to UNKNOWN, `use_connect_errors` on and
  off, a settings file with and without a Connect token; at the end everything fixed, children
  first): after each the result (the `ValueError`'s text), `status()`, `printer_status()`,
  `connect_status()` and the SDK's `COND_TRACKER.get_worst()`. **Result: 220 of 220 identical,
  `status()` compared sorted by name:** its order follows a `set` of conditions
  (`_children`, the SDK's `conditions.py:49`, `__iter__` `:163-166`), which is undefined; ours the
  order of making them. The same names, states and messages.
- **Differs in scope** (proxy, 2026-11-27: approved): one tree and pair of trackers per printer
  module, as `MCSingleton` (section A), not module globals. The SDK's chain (`INTERNET`, `HTTP`,
  `TOKEN`, `API`, `COND_TRACKER`) stays one per process: `INTERNET.set_parent(LAN)` (`:44`) joins it
  to the first module's `LAN`, and a second module fails there with the SDK's own `ValueError`,
  "Can't replace a parent with another" (the SDK's `conditions.py:60-61`; compared with
  PrusaLink's, and `a_second_module_fails_at_the_sdks_chain`). Several MK3 modules in one agent wait
  for the SDK's chain per printer (#218). `connect_status` takes the module's `Settings` as its
  argument, where PrusaLink reads `Settings.instance` (`:117`), one per module as in section A0.
- PrusaLink's own `tests/test_conditions.py`, ported test by test: 6 of its 10 (the other 4 are
  `set_url`, `json_response` and `text_response`, not covered): **all pass.**

**Not covered:** `set_url`, `json_response` and `text_response` (`:147-149`, `:155-177`): poorwsgi's
web layer (`construct_url`, `JSONResponse`, `TextResponse`), which the agent does not have (plan
`docs/archive/plans/mk3-port-plan.md:54`); they are ported when a web section needs them.
`poorwsgi.state`'s codes are their numbers. The `assert HTTP is not None`, `TOKEN` (`:21-22`).

## C: the serial queue (#245)

### `serial/is_planner_fed.py`

Ours: `crates/engine/src/ports/prusalink/is_planner_fed.rs`, on the heaps of section A
(`heap.rs`). The constants it reads (`QUEUE_SIZE`, `HEAP_RATIO`, `IGNORE_ABOVE`, `DEFAULT_THRESHOLD`,
`USE_DYNAMIC_THRESHOLD`) are a `Constants` the planner holds, `const.py`'s by default; PrusaLink's
own tests patch the module's globals.

- `is_planner_fed/tests.rs` `identical_to_prusalink`: 20 cases, 84 operations (PrusaLink's own tests
  as operations; the threshold file with whitespace, CRLF, underscores, `inf`, `nan`, an exponent, bytes
  that are not UTF-8, empty, a folder; a path without a folder; queues of 0, 1, 2 and 20; broken
  heaps to balance and remove from; 25000 confirmation times with `const.py`'s constants, then
  `save` and a new planner from the file), 25163 values processed: after each operation the error,
  if any, and the state (both heaps with `value`, `heap_value`, `heap_index` and heap name, the
  queue, `threshold`, `get_dynamic_threshold()`, `is_fed`, `default_threshold`, the file's bytes),
  after each value `is_fed` and the threshold. **Result: identical.**
- PrusaLink's own `tests/test_is_planner_fed.py`, ported test by test: 14 of 14 **pass.**

**Not covered:** logging (`log.debug`, `:134-135`); a negative `QUEUE_SIZE` (`deque(maxlen=-1)`,
a `ValueError`; ours is a `usize`).

### `printer_adapter/structures/item_updater.py`

Ours: `crates/engine/src/ports/prusalink/item_updater.rs`, on the CPython 3.13 ports of `RLock`,
`Thread`, `Event` (`py_threading`) and `queue.Queue`/`queue.PriorityQueue` with `heapq`
(`python/py_queue.rs`, from `reference/cpython-3.13/heapq.py`), and section A's blinker.

- `item_updater/tests.rs` `identical_to_prusalink`: 18 cases, 246 operations (PrusaLink's own tests
  as operations with a set clock in place of sleeping; gatherers that raise, run out, only have side
  effects or set their own value; validators that refuse or raise; values that stay the same or are
  `None`; disabled items; a receiver that invalidates the group of the item that just became valid;
  timers with equal times), the three threads' loops one turn at a time: after each operation the
  error, if any, the signals and writes in order, and the state of the items, the groups and the
  three queues (the timer heaps in their order). **Result: identical**, except where the source's
  order is a set's (difference 3 below): those cases' logs are compared sorted.
- `priority_queue_identical_to_cpython`: CPython 3.13's own `queue.PriorityQueue` and ours through
  3000 puts and gets of `(time, item)`, many with equal times and some with equal names.
  **Result: identical.**
- PrusaLink's own `tests/test_item_updater.py`, ported test by test: 15 of 15 **pass**, on macOS
  and in the Linux container. Two tests differ where the source's test leans on the GIL (here
  threads run at once): `test_disabling` waits for the item to become valid before reading
  `invalidate_at` (the mock is called before `_set_value` schedules it, `:370`); `test_group` clears
  `gather_1`'s event before `invalidate(item_1)` and sets it again at the end, so item_1's second
  gather cannot make the group valid again before `invalidate(item_2)` (else `:145` sees a second
  `became_invalid`). The tests measure wall-clock time with the source's margins (`THRESHOLD`
  0.05 s; `test_scheduled_invalidation`'s deadline `:266`, about 0.05 s before its last results,
  which it meets because `sleep(0.1)` runs late): the numbers are the source's, and the tests take
  turns (one updater at a time).

**Differs** (owner, 2026-11-27: approved; listed in the module's doc):
1. A value is Python's `Any`; here an item has a type parameter, and `None` is the type's own `None`.
2. An item holds its groups weakly; the group's owner keeps it (as `printer_polling.py` does).
3. Orders the source leaves undefined (`item.in_groups`, `ItemUpdater.items`, a group's
   `valid_items` and `invalid_items` are sets): ours the order of making or adding.
4. `time()` is the updater's clock, `time.time` unless given (the comparison patches the module's
   `time`).
5. The bodies of the three threads' `while self.running:` loops are their own functions, so the
   comparison can run one turn.

**Not covered:** logging; `prctl_name()` and the threads' names; `add_item`'s `TypeError` for what is
not a `WatchedItem` (the types allow only items); `WatchedItem.__repr__` (an address); receivers,
gather, write or validation functions that change an item other than through the updater; a write
function that raises.

### `serial/serial_queue.py`

Ours: `crates/engine/src/ports/prusalink/serial_queue.rs`, on section B1's `SerialParser`, the
`IsPlannerFed` above, section B2's conditions, the CPython 3.13 ports of `Thread` and `Event`
(`py_threading`) and section A's blinker.

- `serial_queue/tests.rs` `identical_to_prusalink`: 19 cases, 542 operations (PrusaLink's own tests
  as operations; lines fed to the real `SerialParser`: confirmations, resends of known, unknown and
  unnumbered messages, captured and missing output, `busy`, temperatures as signs of life; the
  clock set by the case; `SERIAL` going `NOK`; a monitored queue saving its threshold on stop), run
  by `driver_serial_queue.py` against PrusaLink's own `SerialQueue` and `MonitoredSerialQueue` with
  the real `SerialParser` and `IsPlannerFed`, `Thread` logged and not started: after each operation
  the error, if any, the writes, closes, signals and threads in order, and the state (the queues,
  the slots, the history, the numbers, the flags, the planner's `is_fed` and count, `RPI_ENABLED`,
  the parser's pairings). **Result: identical.**
- PrusaLink's own `tests/test_serial_queue.py`, ported test by test: 46 of 46 **pass**, on macOS
  and in the Linux container, without its 4 assertions on `logs.trigger` (`:296`, `:403`, `:506`,
  `:517`; not covered, below).

**Differs** (owner, 2026-11-27: approved; listed in the module's doc):
1. What the tests patch is given to the queue, the source's own by default: the adapter
   (`write`, `close`), the parser (`add_handler`, `remove_handler`), the planner, `Thread` (the
   sender `:124`, the monitor `:613`, the recovery `:544`) and `time()` (as `item_updater`'s 4).
2. `RPI_ENABLED` and `SERIAL` are the printer module's conditions (section B2), given to the queue.
3. `MonitoredSerialQueue` is the same type with a monitor part; its `wait_stopped` calls
   `super().stop()` (`:660-663`), so the sender thread is not joined: kept as the source.
4. `metaclass=MCSingleton`: one queue per printer module (as `SerialParser`, section B1).
5. Python's attributes, each read or written in one statement under the GIL, are one `Mutex` held
   for that statement only, never while the queue calls out; `write_lock` where the source takes it.
6. The output capture handler is a bound method in the source (equal per instruction); ours makes
   one at hookup and removes that one at teardown.

**Not covered:** logging and `InterestingLogRotator.trigger` (`:450`, `:496`, `:648`; the 4 test
assertions on it); `prctl_name()` and the threads' names; the handlers' `assert sender is not None`
and `assert match is not None` (`:371-372`, `:381`); `int()` of a `cmd_number` with Unicode digits
beyond ASCII or beyond an `i64` (`:286-291`, `:382`).
