//! The DYMO LabelManager tape widths the hub needs (the PnP and its kin).
//!
//! The printer's code is not here: the LabelManager PnP is printed by its own port,
//! `crates/engine/src/ports/labelle/` (a port of labelle, `docs/plans/agent-engine.md`, Step 3b).
//! What stays is what the hub reads, the dots across a tape: labelle's `SUPPORTED_TAPE_SIZES_MM`
//! and `DymoLabelerFunctions.height_px` (`dymo_labeler.py:73`, `:77`: `int(8 * tape_size_mm / 12)`
//! bytes of 8 dots), the same numbers as the port's `DymoLabeler::height_px`. Open: the hub
//! should read them from the port's constants, which needs the hub to depend on the engine, and
//! that pulls in the device crates and the OctoPrint port; the owner decides.

/// A tape width the head prints on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tape {
    pub width_mm: u32,
    /// Dots across the tape: labelle's `height_px`, 8 × its bytes a line.
    pub dots: usize,
}

impl Tape {
    /// labelle's tape sizes on a LabelManager (`SUPPORTED_TAPE_SIZES_MM`).
    pub fn d1(width_mm: u32) -> Option<Tape> {
        if ![19, 12, 9, 6].contains(&width_mm) {
            return None;
        }
        Some(Tape { width_mm, dots: (8 * width_mm / 12) as usize * 8 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tapes_are_labelles() {
        let dots: Vec<_> = [6, 9, 12, 19].map(|mm| Tape::d1(mm).unwrap().dots).into();
        assert_eq!(dots, [32, 48, 64, 96]);
        assert_eq!(Tape::d1(24), None);
    }
}
