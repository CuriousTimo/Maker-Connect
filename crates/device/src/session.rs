//! What a driver (a port behind an adapter, or a hub-side driver) implements.
//!
//! Meaning after `sdk/connectorapi/driver.go` and `connector-contract.md` (read only, OCL). The
//! conformance kit ([`crate::conformance`]) checks every rule written here.

use crate::capability::{CapabilitySet, Epoch};
use crate::command::{Command, Outcome};
use crate::identity::Identity;
use crate::state::Patch;
use std::sync::mpsc::Receiver;

/// One kind of device a driver talks to. Keeps no per-device state.
pub trait Driver {
    /// The device kind: a [`crate::registry::STATE_SETS`] kind.
    fn kind(&self) -> &'static str;

    /// Everything this driver can do for any device. Pure: the same set every call, reads
    /// nothing, never blocks. A model profile and a device probe only narrow it.
    fn baseline(&self) -> CapabilitySet;

    /// Opens a session to its device, given the model profile to narrow by its probe (never
    /// widen). The host still resolves baseline ∩ profile ∩ probe and commands against that.
    fn connect(&self, profile: &CapabilitySet) -> Result<Box<dyn Session>, String>;
}

/// One connected device. Shared between threads: commands may come from several at once.
pub trait Session: Send + Sync {
    /// What the device said it is. `model` is not empty.
    fn identity(&self) -> Identity;

    /// The enabled set and its epoch: within one epoch, always the same set.
    fn capabilities(&self) -> (CapabilitySet, Epoch);

    /// Carries out one command the host has checked. A verb not in its set is refused with
    /// `device/CAPABILITY_NOT_SUPPORTED`; a refusal by the device is [`Outcome::Refused`] with
    /// the device's own words; [`Outcome::Ok`] only for an outcome it observed (never after
    /// close, never on a timeout, never when no answer came).
    fn do_command(&self, cmd: &Command) -> Outcome;

    /// The state channel: patches in the order the device produced them. Given once (`None`
    /// after that); it ends when the session closes.
    fn take_state(&self) -> Option<Receiver<Patch>>;

    /// Lets go of the device and ends the state channel. Safe to call more than once.
    fn close(&self);
}
