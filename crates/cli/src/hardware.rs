use std::path::Path;
use std::time::Instant;

use pump_shared::{BleOp, BleResult, Notification, SimDevice, Timestamp};
use pump_ypso::sim::YpsoSim;
use serialport::SerialPort;
use tester_ble::{open_console, os_random, Console, Radio, SimulatorName};

use crate::args::Mode;
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

pub fn session(
    name: SimulatorName,
    console: Option<&Path>,
    mode: Mode,
) -> Result<Session<Hardware>, String> {
    let console = console
        .map(open_console)
        .transpose()
        .map_err(|e| format!("console: {e}"))?;
    let radio = Radio::new(name).map_err(|e| format!("bluetooth: {e}"))?;
    let mut session = Session::new(
        Hardware { radio, console },
        mode,
        Clock::Real(Instant::now()),
    );
    session.harness.random = os_random;
    Ok(session)
}
