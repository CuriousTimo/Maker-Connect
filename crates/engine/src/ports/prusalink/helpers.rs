//! PrusaLink's `serial/helpers.py` (`reference/Prusa-Link`, `171148ce`): Contains helper functions,
//! for instruction enqueuing.
//!
//! The `threading.Event` is [`Event`], the `Instruction` and its two subclasses the [`Instruction`]
//! of `instruction.rs`, `QUIT_INTERVAL` the [`QUIT_INTERVAL`] of `const.rs`, the `re.Pattern` a
//! [`Regex`]. The `SerialQueue` is only a type here (`:12`): what the helpers use of it is
//! [`SerialQueue`].
//!
//! **Not covered:**
//! - The `SerialQueue` class itself (`serial/serial_queue.py:49`): ported in `serial_queue.rs`
//!   (section C). [`SerialQueue`] is what these helpers use of it: `enqueue_one` (`:332`) and
//!   `enqueue_list` (`:348`), implemented by `serial_queue::SerialQueue`.
//! - Python's default arguments: `wait_for_instruction`'s defaults are the source's (`lambda: True`,
//!   the `Event()` of `:17`, `QUIT_INTERVAL`); here the caller passes them, `None` being the
//!   default.

use std::sync::{Arc, LazyLock};

use fancy_regex::Regex;
use maker_connect_gcode_metadata::py::PyErr;

use super::consts::QUIT_INTERVAL;
use super::instruction::Instruction;
use crate::ports::python::py_threading::Event;

/// What the helpers use of `SerialQueue` (`serial/serial_queue.py:49`): `enqueue_one` (`:332`) and
/// `enqueue_list` (`:348`).
///
/// Implemented by `serial_queue::SerialQueue` (section C); a trait so the helpers' tests can give
/// their own queue, as PrusaLink's `test_serial_helpers.py` gives a mock.
pub trait SerialQueue {
    /// `enqueue_one(instruction, to_front=False)` (`serial_queue.py:332`): Enqueue one instruction.
    /// Don't interrupt, if anyone else is enqueueing instructions.
    ///
    /// :param instruction: the thing to be enqueued
    /// :param to_front: whether to enqueue to front of the queue
    fn enqueue_one(&self, instruction: Arc<Instruction>, to_front: bool);

    /// `enqueue_list(instruction_list, to_front=False)` (`serial_queue.py:348`): Enqueue list of
    /// instructions. Don't interrupt, if anyone else is enqueueing instructions.
    ///
    /// :param instruction_list: the list to enqueue
    /// :param to_front: whether to enqueue to front of the queue
    fn enqueue_list(&self, instruction_list: Vec<Arc<Instruction>>, to_front: bool);
}

/// What `wait_for_instruction` uses of the instruction: `wait_for_confirmation` (Python passes any
/// object that has it; PrusaLink's tests a `Mock`).
pub trait WaitForConfirmation {
    fn wait_for_confirmation(&self, timeout: Option<f64>) -> bool;
}

impl WaitForConfirmation for Instruction {
    fn wait_for_confirmation(&self, timeout: Option<f64>) -> bool {
        Instruction::wait_for_confirmation(self, timeout)
    }
}

/// The `Event()` of `wait_for_instruction`'s default (`:17`): created once at import time and
/// shared by every call that does not pass `should_wait_evt`.
static DEFAULT_SHOULD_WAIT_EVT: LazyLock<Event> = LazyLock::new(Event::new);

/// `wait_for_instruction(instruction, should_wait=lambda: True, should_wait_evt=Event(),
/// check_every=QUIT_INTERVAL)` (`:15-30`)
///
/// Wait until the instruction is done, or we shouldn't wait anymore
///
/// :param instruction: The instruction to wait for
/// :param should_wait: a lambda returning true if we should continue waiting
/// :param should_wait_evt: an event, if set, means this should quit
/// :param check_every: how fast to consult the should_wait lambda
pub fn wait_for_instruction(
    instruction: &dyn WaitForConfirmation,
    should_wait: &dyn Fn() -> bool,
    should_wait_evt: Option<&Event>,
    check_every: Option<f64>,
) -> bool {
    let default_event = &DEFAULT_SHOULD_WAIT_EVT;
    let should_wait_evt = should_wait_evt.unwrap_or(default_event);
    let check_every = check_every.unwrap_or(QUIT_INTERVAL);
    while should_wait() && !should_wait_evt.is_set() {
        if instruction.wait_for_confirmation(Some(check_every)) {
            return true;
        }
    }
    false
}

/// `enqueue_instruction(queue, message, to_front=False, to_checksum=False)` (`:33-48`)
///
/// Creates an instruction, which it enqueues right away
///
/// :param queue: the queue to enqueue into
/// :param message: the gcode you wish to send to the printer
/// :param to_front: Whether the instruction has a higher priority
/// :param to_checksum: Whether to number and checksum the instruction (use only for print
/// instructions!)
/// :return the enqueued instruction
pub fn enqueue_instruction(
    queue: &dyn SerialQueue,
    message: &str,
    to_front: bool,
    to_checksum: bool,
) -> Result<Arc<Instruction>, PyErr> {
    let instruction = Arc::new(Instruction::new(message, to_checksum, None, None)?);
    queue.enqueue_one(instruction.clone(), to_front);
    Ok(instruction)
}

/// `enqueue_matchable(queue, message, regexp, to_front=False, to_checksum=False, has_to_match=True)`
/// (`:52-81`)
///
/// Creates a matchable instruction, which it enqueues right away
///
/// :param queue: the queue to enqueue into
/// :param message: the gcode you wish to send to the printer
/// :param regexp: the regular expression which the instruction needs to match, otherwise it will
/// refuse confirmation
/// :param to_front: Whether the instruction has a higher priority
/// :param to_checksum: Whether to number and checksum the instruction (use only for print
/// instructions!)
/// :return the enqueued instruction
// pylint: disable=too-many-arguments
pub fn enqueue_matchable(
    queue: &dyn SerialQueue,
    message: &str,
    regexp: &Regex,
    to_front: bool,
    to_checksum: bool,
    has_to_match: bool,
) -> Result<Arc<Instruction>, PyErr> {
    // `MandatoryMatchableInstruction` (`:73`) and `MatchableInstruction` (`:77`): the port's
    // `Instruction` with its kind.
    let instruction = Arc::new(Instruction::matchable(
        message,
        to_checksum,
        regexp.clone(),
        has_to_match,
    )?);
    queue.enqueue_one(instruction.clone(), to_front);
    Ok(instruction)
}

/// `enqueue_list_from_str(queue, message_list, regexp, to_front=False, to_checksum=False)`
/// (`:84-109`)
///
/// Creates a list of instructions, which it enqueues right away
///
/// :param queue: Queue to enqueue into
/// :param message_list: List of gcodes you wish to send to the printer
/// :param regexp: a regexp to match each instruction output to (this is used by the execute gcode
/// command, so it enqueues with ok / unknown gcode regexp. Keep in mind, that instruction which
/// won't match will refuse to be confirmed)
/// :param to_front: Whether the instruction has a higher priority
/// :param to_checksum: Whether to number and checksum the instruction (use only for print
/// instructions!)
/// :return List of enqueued instructions
pub fn enqueue_list_from_str(
    queue: &dyn SerialQueue,
    message_list: &[&str],
    regexp: &Regex,
    to_front: bool,
    to_checksum: bool,
) -> Result<Vec<Arc<Instruction>>, PyErr> {
    let mut instruction_list: Vec<Arc<Instruction>> = vec![];
    for message in message_list {
        let instruction = Arc::new(Instruction::matchable(
            message,
            to_checksum,
            regexp.clone(),
            false,
        )?);
        instruction_list.push(instruction);
    }
    queue.enqueue_list(instruction_list.clone(), to_front);
    Ok(instruction_list)
}

#[cfg(test)]
mod tests;
