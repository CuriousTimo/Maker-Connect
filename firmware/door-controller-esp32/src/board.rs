//! The Kincony KC868-A6: relays and door inputs on two PCF8574 I2C expanders.
//!
//! Facts from Kincony's own schematic (`KC868-A6 schematic.pdf`, drawn 2022-07-06):
//! - I2C: SDA = GPIO4, SCL = GPIO15 (U22 ESP32 pins 26 `IIC_SDA` and 23 `IIC_SCL`).
//! - Inputs: U3 PCF8574, A0 = GND, A1 = 3V3, A2 = GND, so address 0x22. P0..P5 = `IN_D1`..`IN_D6`.
//!   Each terminal `INPUT_Dn` drives an EL357 optocoupler (anode at 12 V through 2 kΩ): a contact from
//!   the terminal to GND switches it on and pulls `IN_Dn` LOW. Open contact = HIGH (4.7 kΩ pull-up).
//! - Relays: U6 PCF8574, A0 = GND, A1 = GND, A2 = 3V3, so address 0x24. P0..P5 = `OT1`..`OT6`.
//!   `OTn` goes through a 74HCT14 (inverts) and a ULN2003 (sinks the coil `RLYn`): `OTn` LOW = relay on.
//!   The PCF8574 starts with every pin HIGH, so every relay is off at power-up.
//!
//! The lock's switch (SARY XG-07C-12V, blue/white) is closed while the lock is shut: closed reads LOW,
//! which is [`DoorState::Closed`].

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use door_controller::traits::{DoorState, HwError, Inputs, Relays};
use esp_idf_svc::hal::i2c::I2cDriver;

/// Relays and inputs this board has.
pub const CHANNELS: u32 = 6;
const INPUTS_ADDR: u8 = 0x22;
const RELAYS_ADDR: u8 = 0x24;
/// Every relay off.
const ALL_OFF: u8 = 0xFF;
/// I2C timeout in FreeRTOS ticks (1 ms each, `CONFIG_FREERTOS_HZ=1000`).
const TIMEOUT: u32 = 50;

pub type Bus = Rc<RefCell<I2cDriver<'static>>>;

/// Puts both expanders in their safe state: relays off, input pins HIGH (a PCF8574 pin reads only
/// while it is written HIGH). An ESP32 reset does not reset the expanders, so this runs at start.
pub fn init(bus: &Bus) -> Result<(), HwError> {
    let mut i2c = bus.borrow_mut();
    i2c.write(RELAYS_ADDR, &[ALL_OFF], TIMEOUT)
        .map_err(|_| HwError)?;
    i2c.write(INPUTS_ADDR, &[0xFF], TIMEOUT)
        .map_err(|_| HwError)?;
    Ok(())
}

pub struct BoardRelays {
    pub bus: Bus,
}

impl Relays for BoardRelays {
    fn pulse(&mut self, lock: u32, ms: u32) -> Result<(), HwError> {
        if lock >= CHANNELS {
            return Err(HwError);
        }
        // The crate never asks for more than LOCK_PULSE_MAX_MS; the board holds to it too (wiki
        // Design: Door Controller: "No setting, command or bug may hold a relay on longer").
        let ms = ms.min(door_controller::LOCK_PULSE_MAX_MS);
        let on = ALL_OFF & !(1u8 << lock);
        let started = self.bus.borrow_mut().write(RELAYS_ADDR, &[on], TIMEOUT);
        if started.is_ok() {
            std::thread::sleep(Duration::from_millis(u64::from(ms)));
        }
        // Always switch off, whatever the first write did.
        let off = self
            .bus
            .borrow_mut()
            .write(RELAYS_ADDR, &[ALL_OFF], TIMEOUT);
        started.map_err(|_| HwError)?;
        off.map_err(|_| HwError)
    }
}

pub struct BoardInputs {
    pub bus: Bus,
}

impl Inputs for BoardInputs {
    fn read_door(&mut self, lock: u32) -> Result<DoorState, HwError> {
        if lock >= CHANNELS {
            return Err(HwError);
        }
        let mut byte = [0u8; 1];
        self.bus
            .borrow_mut()
            .read(INPUTS_ADDR, &mut byte, TIMEOUT)
            .map_err(|_| HwError)?;
        Ok(if byte[0] & (1 << lock) == 0 {
            DoorState::Closed
        } else {
            DoorState::Open
        })
    }
}
