use std::path::Path;
use std::time::Instant;

use pump_shared::{BleOp, BleResult, Notification, SimDevice, Timestamp};
use pump_ypso::sim::YpsoSim;
use serialport::SerialPort;
use tester_ble::{open_console, os_random, Console, Radio, SimulatorName};

use crate::args::{CgmRun, Mode};
use crate::cgm::session::{CgmBoard, CgmSession, Setup};
use crate::cgm::store::SlotStore;
use crate::session::{Board, Clock, Session};

pub struct Hardware {
    radio: Radio,
    console: Option<Console<Box<dyn SerialPort>>>,
}

impl SimDevice for Hardware {
    fn handle(&mut self, op: &BleOp, now: Timestamp) -> BleResult {
        self.radio.handle(op, now)
    }

    fn poll_notification(&mut self, now: Timestamp) -> Option<Notification> {
        self.radio.poll_notification(now)
    }
}

impl Board for Hardware {
    fn console(&mut self, line: &str, _now: Timestamp) -> Result<(), String> {
        let console = self.console.as_mut().ok_or("no --console given")?;
        console.send(line).map_err(|e| e.to_string())
    }

    fn pump(&mut self) -> Option<&mut YpsoSim> {
        None
    }
}

/// The same board, hosting a sensor instead of a pump.
///
/// The radio layer knows nothing about either: it speaks `BleOp` and
/// `Notification` and is reused as it is. All that differs is what is on the
/// other end of the console and which engine the session drives.
pub struct CgmHardware {
    radio: Radio,
    console: Option<Console<Box<dyn SerialPort>>>,
}

impl SimDevice for CgmHardware {
    fn handle(&mut self, op: &BleOp, now: Timestamp) -> BleResult {
        self.radio.handle(op, now)
    }

    fn poll_notification(&mut self, now: Timestamp) -> Option<Notification> {
        self.radio.poll_notification(now)
    }
}

impl CgmBoard for CgmHardware {
    fn console(&mut self, line: &str, _now: Timestamp) -> Result<(), String> {
        let console = self
            .console
            .as_mut()
            .ok_or("no --console given: the board's scenario knobs are on its serial port")?;
        console.send(line).map_err(|e| e.to_string())
    }

    fn has_console(&self) -> bool {
        self.console.is_some()
    }
}

/// A session against the sensor the board is impersonating.
pub fn cgm_session(run: CgmRun) -> Result<CgmSession<CgmHardware>, String> {
    let console = run
        .console
        .as_deref()
        .map(open_console)
        .transpose()
        .map_err(|e| format!("console: {e}"))?;
    let radio = Radio::new(run.name.clone().into()).map_err(|e| format!("bluetooth: {e}"))?;
    let variant = crate::cgm::variant(run.model, run.name.as_str())?;
    let store = SlotStore::open(run.store.clone())?;
    let clock = Clock::Real(Instant::now());
    let setup = Setup {
        model: run.model,
        name: run.name.as_str().to_owned(),
        address: run.address,
        variant,
        credential: run.credential,
        pairing_secret: run.pairing_secret,
        expected_serial: run.expected_serial,
        minutes: run.minutes,
    };
    let mut session = CgmSession::new(CgmHardware { radio, console }, setup, store, clock)?;
    // The engine's `Random` must be real randomness: its challenge token is
    // what proves the sensor holds the key, and a predictable one proves
    // nothing. The in-process harness counts instead, which is fine for a
    // test and not for a radio.
    session.harness.random = os_random;
    Ok(session)
}

pub fn session(
    name: SimulatorName,
    console: Option<&Path>,
    mode: Mode,
) -> Result<Session<Hardware>, String> {
    let console = console
        .map(open_console)
        .transpose()
        .map_err(|e| format!("console: {e}"))?;
    let radio = Radio::new(name.into()).map_err(|e| format!("bluetooth: {e}"))?;
    let mut session = Session::new(
        Hardware { radio, console },
        mode,
        Clock::Real(Instant::now()),
    );
    session.harness.random = os_random;
    Ok(session)
}
