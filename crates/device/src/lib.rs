//! Maker Connect's device contract (hub core v2, `docs/archive/plans/hub-core-v2.md` §2.1, §2.10, step 1).
//!
//! Every machine is a device: an [`Identity`], an enabled [`CapabilitySet`] with an [`Epoch`], a
//! declared [`StateSet`], a [`StateTree`] changed by [`Patch`]es, and [`Command`]s answered by an
//! [`Outcome`]. The host checks a command ([`validate::check`]) before a driver sees it.
//!
//! Written by us, in Rust, after the *meaning* of Prusa's connector contract
//! (`reference/prusa-connect` at 2e8c62b: `sdk/connectorapi/driver.go`, `set.go`, `validate.go`,
//! `direction.go`, `devicestate.go`; `reference/prusa-connect/docs/src/content/docs/connector-contract.md`).
//! That code is under the OCL: it was read to understand, nothing is copied.
//!
//! The contract is internal. What reaches a machine is still made by a port that follows its one
//! reference to the letter (CLAUDE.md); a verb maps to the reference's own call, and a verb the
//! reference has no call for is not declared. Our own limits sit in host-side validation.
//!
//! The registries ([`registry`]) hold the agent ports' verbs and state sets
//! (`docs/archive/plans/hub-core-v2-device-map.md`) and the fault codes the validator needs; test fixture
//! rows and the fixture device (`fixture`) exist only under `cfg(test)`. The conformance kit
//! ([`conformance`]) does no I/O of its own.

pub mod capability;
pub mod command;
pub mod conformance;
pub mod fault;
#[cfg(test)]
pub mod fixture;
pub mod identity;
pub mod registry;
pub mod session;
pub mod state;
pub mod validate;

pub use capability::{Arg, CapabilitySet, EnabledSet, Epoch, Kind, Unit, Verb};
pub use command::{Caller, Command, Outcome};
pub use fault::FaultCode;
pub use identity::{FirmwareLabel, Identity};
pub use session::{Driver, Session};
pub use state::{Patch, PatchError, StateSet, StateTree};
