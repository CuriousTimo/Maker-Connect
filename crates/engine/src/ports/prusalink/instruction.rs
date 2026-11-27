//! PrusaLink's `serial/instruction.py` (`reference/Prusa-Link`, `171148ce`): Contains
//! implementation for all the types of instructions enqueueable to the serial queue (#242,
//! section A).
//!
//! The three classes are one struct with its [`Kind`]: `Instruction`, `MatchableInstruction(Instruction)`
//! and `MandatoryMatchableInstruction(MatchableInstruction)`, each method as its class has it. An
//! instruction is shared by the queue's thread and the one waiting for it, so what Python changes
//! on it is behind a lock. `threading.Event` is the CPython port [`Event`]; a `re.Match` the
//! groups it holds, [`Match`].
//!
//! **Not covered:** logging (`log.warning` in `MandatoryMatchableInstruction.confirm`): kept as a
//! comment, as in the other engine ports; `assert`s on arguments the types make sure of
//! (`force is not None`, `sender is not None`, `match is not None`); `match` and `get_matches` of a
//! plain `Instruction`: an `AttributeError` there, nothing captured here.

use std::sync::{Arc, Mutex};

use fancy_regex::{Captures, Regex};
use maker_connect_gcode_metadata::py::PyErr;

use super::util::time;
use crate::ports::python::py_threading::Event;

/// A `re.Match`: its groups by number (`group(0)` the whole match) and by name.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub groups: Vec<Option<String>>,
    pub names: Vec<Option<String>>,
}

impl Match {
    pub fn new(regex: &Regex, captures: &Captures) -> Match {
        Match {
            groups: (0..captures.len())
                .map(|i| captures.get(i).map(|m| m.as_str().to_owned()))
                .collect(),
            names: regex
                .capture_names()
                .map(|n| n.map(str::to_owned))
                .collect(),
        }
    }

    /// `match.group(index)`
    pub fn group(&self, index: usize) -> Option<&str> {
        self.groups.get(index).and_then(|g| g.as_deref())
    }

    /// `match.group(name)`
    pub fn named(&self, name: &str) -> Option<&str> {
        let index = self.names.iter().position(|n| n.as_deref() == Some(name))?;
        self.group(index)
    }
}

/// Which class an instruction is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `Instruction` (`:14-136`): Basic instruction which can be enqueued into SerialQueue
    Instruction,
    /// `MatchableInstruction` (`:139-168`): Matches using captures_matching.
    Matchable,
    /// `MandatoryMatchableInstruction` (`:171-183`): HAS TO MATCH, otherwise refuses confirmation!
    /// This should fix a communication error we're having.
    MandatoryMatchable,
}

/// What Python changes on an instruction after it is made.
#[derive(Debug, Default)]
struct State {
    message: String,
    data: Option<Vec<u8>>,
    number: Option<i64>,
    sent_at: Option<f64>,
    time_to_confirm: Option<f64>,
    _captured: Vec<Arc<Match>>,
}

pub struct Instruction {
    pub kind: Kind,
    /// Some messages need to be sent with numbered lines and with checksums
    /// This shall be exclusive for printing from files
    pub to_checksum: bool,
    state: Mutex<State>,
    /// Event set when the write has been _confirmed by the printer
    pub confirmed_event: Event,
    /// Event set when the write has been sent to the printer
    pub sent_event: Event,
    /// Api for registering instruction regexps
    pub capturing_regexps: Vec<Regex>,
    /// `MatchableInstruction`: Output _captured between command submission and confirmation
    pub capture_matching: Option<Regex>,
}

/// `re.compile(r".*")`, `MatchableInstruction`'s default `capture_matching` (`:145`)
pub fn any_line() -> Regex {
    Regex::new(r".*").unwrap()
}

/// `message.encode("ASCII")`: `UnicodeEncodeError` at the first character beyond ASCII.
fn encode_ascii(message: &str) -> Result<Vec<u8>, PyErr> {
    let mut run: Option<(usize, usize)> = None;
    let chars: Vec<char> = message.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if !c.is_ascii() {
            match run {
                Some((start, _)) => run = Some((start, i + 1)),
                None => run = Some((i, i + 1)),
            }
        } else if run.is_some() {
            break;
        }
    }
    let Some((start, end)) = run else {
        return Ok(message.as_bytes().to_vec());
    };
    // CPython: one bad character "can't encode character '\xfc' in position 3", a run "can't
    // encode characters in position 3-4"
    let text = if end - start == 1 {
        let c = chars[start] as u32;
        let escaped = if c < 0x100 {
            format!("\\x{c:02x}")
        } else if c < 0x10000 {
            format!("\\u{c:04x}")
        } else {
            format!("\\U{c:08x}")
        };
        format!(
            "'ascii' codec can't encode character '{escaped}' in position {start}: ordinal not in range(128)"
        )
    } else {
        format!(
            "'ascii' codec can't encode characters in position {start}-{}: ordinal not in range(128)",
            end - 1
        )
    };
    Err(PyErr::Other("UnicodeEncodeError", text))
}

impl Instruction {
    /// `Instruction.__init__(message, to_checksum=False, data=None, number=None)` (`:16-50`)
    pub fn new(
        message: &str,
        to_checksum: bool,
        data: Option<Vec<u8>>,
        number: Option<i64>,
    ) -> Result<Instruction, PyErr> {
        if message.matches('\n').count() != 0 {
            return Err(PyErr::Other(
                "RuntimeError",
                "Instructions cannot contain newlines.".to_owned(),
            ));
        }

        Ok(Instruction {
            kind: Kind::Instruction,
            to_checksum,
            state: Mutex::new(State {
                // Can be changed before the instruction is sent.
                message: message.to_owned(),
                // If already sent, this will contain the sent bytes
                data,
                // If we know our number, it is saved here (used by message history)
                number,
                // Measuring the time between sending and confirmation will hopefully
                // enable me to determine if the motion planner buffer is full
                sent_at: None,
                time_to_confirm: None,
                _captured: vec![],
            }),
            confirmed_event: Event::new(),
            sent_event: Event::new(),
            capturing_regexps: vec![],
            capture_matching: None,
        })
    }

    /// `MatchableInstruction.__init__(*args, capture_matching=re.compile(r".*"), **kwargs)`
    /// (`:143-153`); `mandatory`: `MandatoryMatchableInstruction`, which has the same.
    pub fn matchable(
        message: &str,
        to_checksum: bool,
        capture_matching: Regex,
        mandatory: bool,
    ) -> Result<Instruction, PyErr> {
        let mut instruction = Instruction::new(message, to_checksum, None, None)?;
        instruction.kind = if mandatory {
            Kind::MandatoryMatchable
        } else {
            Kind::Matchable
        };
        instruction.capturing_regexps = vec![capture_matching.clone()];
        instruction.capture_matching = Some(capture_matching);
        Ok(instruction)
    }

    pub fn message(&self) -> String {
        self.state.lock().unwrap().message.clone()
    }

    /// `instruction.message = ...`: Can be changed before the instruction is sent.
    pub fn set_message(&self, message: &str) {
        self.state.lock().unwrap().message = message.to_owned();
    }

    pub fn data(&self) -> Option<Vec<u8>> {
        self.state.lock().unwrap().data.clone()
    }

    pub fn number(&self) -> Option<i64> {
        self.state.lock().unwrap().number
    }

    pub fn sent_at(&self) -> Option<f64> {
        self.state.lock().unwrap().sent_at
    }

    pub fn time_to_confirm(&self) -> Option<f64> {
        self.state.lock().unwrap().time_to_confirm
    }

    /// `__str__` (`:52-53`), and `__repr__` (`:55-56`)
    pub fn str(&self) -> String {
        format!(
            "Instruction '{}'",
            maker_connect_gcode_metadata::py::strip(&self.message())
        )
    }

    /// `confirm(force=False)`: `Instruction.confirm` (`:58-67`): Return False, if getting
    /// confirmed but not wanting to (not used in the base implementation anymore);
    /// `MandatoryMatchableInstruction.confirm` (`:176-183`).
    pub fn confirm(&self, force: bool) -> bool {
        if self.kind == Kind::MandatoryMatchable {
            // Yes, matchables HAVE TO match now!
            if self.state.lock().unwrap()._captured.is_empty() && !force {
                // log.warning("Instruction %s did not capture its expected output, "
                //             "so it REFUSES to be confirmed!", self.message)
                return false;
            }
            // super().confirm()
        }
        let mut state = self.state.lock().unwrap();
        let sent_at = state
            .sent_at
            .expect("AssertionError: assert self.sent_at is not None");
        state.time_to_confirm = Some(time() - sent_at);
        drop(state);
        self.confirmed_event.set();
        true
    }

    /// `sent()` (`:69-75`): Sets the instruction sent Event and writes the timestamp, when the
    /// instruction got sent
    pub fn sent(&self) {
        self.sent_event.set();
        self.state.lock().unwrap().sent_at = Some(time());
    }

    /// `output_captured(sender, match)`: `Instruction` (`:77-83`): Output _captured event
    /// handler, this type does not capture anything though; `MatchableInstruction` (`:155-158`):
    /// Appends _captured output to the instructions _captured list
    pub fn output_captured(&self, r#match: Arc<Match>) {
        if self.kind != Kind::Instruction {
            self.state.lock().unwrap()._captured.push(r#match);
        }
    }

    /// `wait_for_send(timeout=None)` (`:85-87`): Proxy call to wait method of the sent Event
    pub fn wait_for_send(&self, timeout: Option<f64>) -> bool {
        self.sent_event.wait(timeout)
    }

    /// `wait_for_confirmation(timeout=None)` (`:89-91`): Proxy call to wait method of the
    /// confirmed Event
    pub fn wait_for_confirmation(&self, timeout: Option<f64>) -> bool {
        self.confirmed_event.wait(timeout)
    }

    /// `is_sent()` (`:93-95`): Returns whether this instruction has been sent yet
    pub fn is_sent(&self) -> bool {
        self.sent_event.is_set()
    }

    /// `is_confirmed()` (`:97-99`): Returns whether this instruction has been confirmed yet
    pub fn is_confirmed(&self) -> bool {
        self.confirmed_event.is_set()
    }

    /// `reset()` (`:101-104`): Resets the send status of an instruction
    pub fn reset(&self) {
        self.state.lock().unwrap().sent_at = None;
        self.sent_event.clear();
    }

    /// `fill_data(message_number)` (`:106-123`): Puts together binary data to send as for the
    /// given instruction. The specific data might contain a message number and a checksum. Also
    /// a newline gets appended at the end
    pub fn fill_data(&self, message_number: i64) -> Result<(), PyErr> {
        let mut state = self.state.lock().unwrap();
        let mut data = encode_ascii(&state.message)?;
        if self.to_checksum {
            let number_part = format!("N{message_number} ").into_bytes();
            let mut to_checksum = number_part;
            to_checksum.extend_from_slice(&data);
            to_checksum.push(b' ');
            let checksum = Self::get_checksum(&to_checksum);
            let checksum_data = format!("*{checksum}").into_bytes();
            data = to_checksum;
            data.extend_from_slice(&checksum_data);
            state.number = Some(message_number);
        }
        data.push(b'\n');
        state.data = Some(data);
        Ok(())
    }

    /// `get_checksum(data)` (`:125-136`): Goes over the given bytes and returns a checksum, which
    /// is constructed by XORing each byte of data to a zero
    pub fn get_checksum(data: &[u8]) -> u8 {
        let mut checksum = 0;
        for byte in data {
            checksum ^= byte;
        }
        checksum
    }

    /// `MatchableInstruction.match(index=0)` (`:160-164`): If match with an index exists, return
    /// it, otherwise return None
    pub fn r#match(&self, index: usize) -> Option<Arc<Match>> {
        let state = self.state.lock().unwrap();
        if !state._captured.is_empty() && state._captured.len() > index {
            return Some(state._captured[index].clone());
        }
        None
    }

    /// `MatchableInstruction.get_matches()` (`:166-168`): Returns the list of all _captured
    /// matches
    pub fn get_matches(&self) -> Vec<Arc<Match>> {
        self.state.lock().unwrap()._captured.clone()
    }
}

#[cfg(test)]
mod tests;
