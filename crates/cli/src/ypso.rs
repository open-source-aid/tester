use pump_shared::{Address, Advertisement, Config, Driver, Engine, SimDevice, Slot, Timestamp};
use pump_ypso::protocol::{setting, uuids, KEY_LEN};
use pump_ypso::sim::{YpsoSim, PASSWORD_LEN};
use pump_ypso::DRIVER;
use simulator_bridge::{Line, Simulator};
use tester_harness::Harness;

use crate::args::Mode;
use crate::session::{Board, Store};

pub const SERIAL: u32 = 0xDEAD_BEEF;
const PASSWORD: [u8; PASSWORD_LEN] = [0x5A; PASSWORD_LEN];
pub const KEY: [u8; KEY_LEN] = [0xA5; KEY_LEN];
pub const CODE: &[u8; 2] = b"O2";
const SERVER_KEY: [u8; KEY_LEN] = [
    0xCA, 0xFE, 0xCA, 0xFE, 0xCA, 0xFE, 0xCA, 0xFE, 0xCA, 0xFE, 0xCA, 0xFE, 0xCA, 0xFE, 0xCA, 0xFE,
    0xCA, 0xFE, 0xCA, 0xFE, 0xCA, 0xFE, 0xCA, 0xFE, 0xCA, 0xFE, 0xCA, 0xFE, 0xCA, 0xFE, 0xCA, 0xFE,
];
const MAC: [u8; 6] = [0; 6];

pub fn board(mode: Mode) -> YpsoSim {
    let mut sim = match mode {
        Mode::Opn => YpsoSim::opn(PASSWORD),
        Mode::Encrypted => YpsoSim::new(KEY, PASSWORD),
    };
    sim.server_secret = SERVER_KEY;
    sim.settings
        .insert(setting::SERIAL_NUMBER_UPPER, (SERIAL >> 16) as i32);
    sim.settings
        .insert(setting::SERIAL_NUMBER_LOWER, (SERIAL & 0xFFFF) as i32);
    sim
}

impl Board for YpsoSim {
    fn console(&mut self, line: &str, now: Timestamp) -> Result<(), String> {
        match line.parse().map_err(|e| format!("{e}"))? {
            // The console grammar covers both device kinds; a pump takes the
            // pump half and says so rather than ignoring the other one.
            Line::Knob(knob) => self.apply(knob.try_into().map_err(|e| format!("{e}"))?, now),
            Line::Advance(_) | Line::Reset | Line::Model(_) => {}
        }
        Ok(())
    }

    fn pump(&mut self) -> Option<&mut YpsoSim> {
        Some(self)
    }
}

pub fn engine() -> Box<dyn Engine> {
    let adv = Advertisement {
        address: Address::new(MAC),
        name: Some(&format!("YpsoPump_{SERIAL}")),
        service_uuids: &[],
        manufacturer: None,
        rssi: 0,
    };
    let variant = DRIVER.matches(&adv).expect("a YpsoPump name");
    DRIVER.new_engine(variant, Config::default())
}

pub fn harness<D: SimDevice>(device: D, mode: Mode) -> Harness<D> {
    let mut harness = Harness::new(device);
    harness.announced = Some(DRIVER.info());
    harness.resendable.insert(uuids::AUTHORIZATION_PASSWORD);
    harness.resendable.insert(uuids::SETTING_ID);
    harness.resendable.insert(uuids::EVENT_ENTRY_INDEX);
    harness.store = store(mode, KEY, CODE);
    harness
}

pub fn store(mode: Mode, key: [u8; KEY_LEN], code: &[u8; 2]) -> Store {
    Store::from([(Slot::Session, blob(mode, key, code))])
}

fn blob(mode: Mode, key: [u8; KEY_LEN], code: &[u8; 2]) -> Vec<u8> {
    let (version, flags) = match mode {
        Mode::Opn => (4, 1),
        Mode::Encrypted => (1, 0),
    };
    let mut out = b"YB".to_vec();
    out.extend([version, flags]);
    out.extend(MAC);
    out.extend(SERIAL.to_le_bytes());
    out.extend(key);
    out.extend((-1i64).to_le_bytes());
    out.extend((-1i64).to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(0i64.to_le_bytes());
    out.push(0);
    out.extend([7; 16]);
    out.extend([1, 2]);
    out.extend(code);
    out.extend(PASSWORD);
    let crc = crc32(&out);
    out.extend(crc.to_le_bytes());
    out
}

fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(!0u32, |crc, &byte| {
        (0..8).fold(crc ^ u32::from(byte), |crc, _| {
            (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_board_double_is_named_like_the_board() {
        for mode in [Mode::Opn, Mode::Encrypted] {
            assert_eq!(board(mode).name(), "YpsoPump_3735928559");
            assert_eq!(board(mode).server_secret[..2], [0xCA, 0xFE]);
        }
    }

    #[test]
    fn the_console_takes_the_board_commands() {
        let mut sim = board(Mode::Opn);
        assert_eq!(sim.console("battery 1", Timestamp::default()), Ok(()));
        assert_eq!(sim.battery, 1);
        assert!(sim
            .console("explode", Timestamp::default())
            .unwrap_err()
            .starts_with("unknown command `explode`"));
    }

    #[test]
    fn checksums_like_the_driver() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
