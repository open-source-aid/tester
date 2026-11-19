use core::fmt;
use core::time::Duration;
use std::collections::BTreeMap;
use std::time::Instant;

use pump_shared::{
    BleOp, Command, CommandId, Engine, Event, Outcome, Sequence, SimDevice, Slot, Timestamp,
};
use pump_ypso::sim::YpsoSim;
use tester_harness::{Harness, Violation};

use crate::args::Mode;
use crate::ypso;

pub type Store = BTreeMap<Slot, Vec<u8>>;

pub trait Board: SimDevice {
    fn console(&mut self, line: &str, now: Timestamp) -> Result<(), String>;
    fn pump(&mut self) -> Option<&mut YpsoSim>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clock {
    Virtual,
    Real(Instant),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Violation(Violation),
    Unqueued,
    Unfinished(CommandId),
    Console(String),
    Unexpected(String),
    Skipped(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Violation(violation) => write!(f, "invariant violated: {violation}"),
            Self::Unqueued => f.write_str("command not queued"),
            Self::Unfinished(id) => write!(f, "command {} never finished", id.get()),
            Self::Console(error) => write!(f, "console: {error}"),
            Self::Unexpected(what) => write!(f, "unexpected {what}"),
            Self::Skipped(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for Error {}

const LIMIT: Duration = Duration::from_secs(300);

const UNREACHABLE: &str = "needs the pump's internal state, which the board console cannot reach";

pub struct Session<D> {
    pub harness: Harness<D>,
    pub engine: Box<dyn Engine>,
    pub mode: Mode,
    pub seen: Sequence,
    counters: Option<Vec<u8>>,
    clock: Clock,
    failure: Option<Error>,
}

impl<D: Board> Session<D> {
    pub fn new(device: D, mode: Mode, clock: Clock) -> Self {
        Self {
            harness: ypso::harness(device, mode),
            engine: ypso::engine(),
            mode,
            seen: Sequence::new(0),
            counters: None,
            clock,
            failure: None,
        }
    }

    pub fn skip(&mut self, why: &'static str) {
        self.fail(Error::Skipped(why));
    }

    pub fn tamper(&mut self, change: &mut dyn FnMut(&mut YpsoSim)) {
        if self.failure.is_some() {
            return;
        }
        let blind = self.harness.device.pump().map(change).is_none();
        self.failure = blind.then_some(Error::Skipped(UNREACHABLE));
    }

    pub fn switch(&mut self, engine: Box<dyn Engine>, store: Store) -> (Box<dyn Engine>, Store) {
        self.sync();
        self.harness
            .device
            .handle(&BleOp::Disconnect, self.harness.now);
        self.engine.handle_link_lost();
        self.harness.restart();
        let store = core::mem::replace(&mut self.harness.store, store);
        let engine = core::mem::replace(&mut self.engine, engine);
        self.wait(Duration::ZERO);
        (engine, store)
    }

    pub fn restart(&mut self) {
        let store = self.harness.store.clone();
        self.switch(ypso::engine(), store);
    }

    pub fn fresh(&mut self) {
        if let Some(counters) = self.harness.store.get(&Slot::Counters) {
            self.counters = Some(counters.clone());
        }
        let mut store = ypso::store(self.mode, ypso::KEY, ypso::CODE);
        if let Some(counters) = &self.counters {
            store.insert(Slot::Counters, counters.clone());
        }
        self.switch(ypso::engine(), store);
    }

    pub fn result(&mut self) -> Result<(), Error> {
        self.failure.take().map_or(Ok(()), Err)
    }

    pub fn check(&mut self, holds: bool, what: String) {
        if !holds {
            self.fail(Error::Unexpected(what));
        }
    }

    pub fn events(&self) -> &[Event] {
        &self.harness.events
    }

    pub fn submit(&mut self, command: Command) -> Option<CommandId> {
        self.step(Timestamp::default());
        if self.failure.is_some() {
            return None;
        }
        self.engine.submit(command).ok()
    }

    pub fn stop(&mut self) -> CommandId {
        self.engine.stop()
    }

    pub fn console(&mut self, line: &str) {
        self.sync();
        let now = self.harness.now;
        if self.failure.is_none() {
            if let Err(error) = self.harness.device.console(line, now) {
                self.fail(Error::Console(error));
            }
        }
    }

    pub fn finish(&mut self, id: Option<CommandId>) -> Outcome {
        let Some(id) = id else {
            self.fail(Error::Unqueued);
            return Outcome::Unknown;
        };
        let until = self.later(LIMIT);
        loop {
            let more = self.step(until);
            if let Some(outcome) = self.outcome(id) {
                return outcome;
            }
            if !more {
                self.fail(Error::Unfinished(id));
                return Outcome::Unknown;
            }
        }
    }

    pub fn command(&mut self, command: Command) -> Outcome {
        let id = self.submit(command);
        self.finish(id)
    }

    pub fn wait(&mut self, by: Duration) {
        let until = self.later(by);
        while self.step(until) {}
        self.reach(until);
    }

    fn fail(&mut self, error: Error) {
        self.failure.get_or_insert(error);
    }

    fn outcome(&self, id: CommandId) -> Option<Outcome> {
        self.events().iter().find_map(|event| match event {
            Event::Finished { id: done, outcome } if *done == id => Some(outcome.clone()),
            _ => None,
        })
    }

    fn later(&mut self, by: Duration) -> Timestamp {
        self.sync();
        Timestamp::from_millis(self.harness.now.as_millis() + by.as_millis() as u64)
    }

    fn step(&mut self, until: Timestamp) -> bool {
        self.sync();
        if self.failure.is_some() {
            return false;
        }
        if let Err(violation) = self.harness.run(self.engine.as_mut()) {
            self.fail(Error::Violation(violation));
            return false;
        }
        match self.engine.poll_timeout() {
            Some(at) if at <= until => {
                self.reach(at);
                self.engine.handle_timeout(self.harness.now);
                true
            }
            _ => false,
        }
    }

    fn sync(&mut self) {
        if let Clock::Real(start) = self.clock {
            self.harness.now = Timestamp::from_millis(start.elapsed().as_millis() as u64);
        }
    }

    fn reach(&mut self, at: Timestamp) {
        if let Clock::Real(start) = self.clock {
            std::thread::sleep(
                Duration::from_millis(at.as_millis()).saturating_sub(start.elapsed()),
            );
        }
        self.harness.now = self.harness.now.max(at);
        self.sync();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tester_harness::Fault;

    fn session(clock: Clock) -> Session<YpsoSim> {
        Session::new(ypso::board(Mode::Opn), Mode::Opn, clock)
    }

    struct Blind(YpsoSim);

    impl SimDevice for Blind {
        fn handle(&mut self, op: &BleOp, now: Timestamp) -> pump_shared::BleResult {
            self.0.handle(op, now)
        }
    }

    impl Board for Blind {
        fn console(&mut self, line: &str, now: Timestamp) -> Result<(), String> {
            self.0.console(line, now)
        }

        fn pump(&mut self) -> Option<&mut YpsoSim> {
            None
        }
    }

    fn drain(pump: &mut YpsoSim) {
        pump.battery = 0;
    }

    #[test]
    fn tampers_with_the_pump_state_in_process() {
        let mut session = session(Clock::Virtual);
        session.tamper(&mut drain);
        assert_eq!(session.harness.device.battery, 0);
    }

    #[test]
    fn skips_what_needs_the_pump_state_without_it() {
        let mut session = Session::new(Blind(ypso::board(Mode::Opn)), Mode::Opn, Clock::Virtual);
        session.console("battery 1");
        session.command(Command::Status);
        session.wait(Duration::ZERO);
        session.tamper(&mut drain);
        session.tamper(&mut drain);
        assert_eq!(session.result(), Err(Error::Skipped(UNREACHABLE)));
        assert_eq!(Error::Skipped(UNREACHABLE).to_string(), UNREACHABLE);
    }

    #[test]
    fn records_the_first_failure_and_skips_the_rest() {
        let mut session = session(Clock::Virtual);
        session.harness.resendable.clear();
        session.harness.faults.insert(12, Fault::BleTimeout);
        session.command(Command::Status);
        session.wait(Duration::ZERO);
        assert_eq!(session.command(Command::Status), Outcome::Unknown);
        session.console("battery 1");
        session.check(false, "later".into());
        assert_eq!(
            session.harness.device.battery,
            ypso::board(Mode::Opn).battery
        );
        assert_eq!(
            session.result().unwrap_err().to_string(),
            "invariant violated: write [5a, 5a, 5a, 5a, 5a, 5a, 5a, 5a, 5a, 5a, 5a, 5a, 5a, 5a, 5a, 5a] resent after a timeout"
        );
        assert_eq!(session.result(), Ok(()));
    }

    #[test]
    fn reports_commands_that_never_finish_or_never_queue() {
        let mut session = session(Clock::Virtual);
        assert_eq!(session.finish(None), Outcome::Unknown);
        assert_eq!(
            session.result().unwrap_err().to_string(),
            "command not queued"
        );
        assert_eq!(session.finish(Some(CommandId::new(99))), Outcome::Unknown);
        assert_eq!(
            session.result().unwrap_err().to_string(),
            "command 99 never finished"
        );
    }

    #[test]
    fn reports_console_refusals() {
        let mut session = session(Clock::Virtual);
        session.console("explode");
        assert!(session
            .result()
            .unwrap_err()
            .to_string()
            .starts_with("console: unknown command `explode`"));
    }

    #[test]
    fn a_real_clock_waits_in_real_time() {
        let start = Instant::now();
        let mut session = session(Clock::Real(start));
        session.wait(Duration::from_millis(30));
        assert!(start.elapsed() >= Duration::from_millis(30));
        assert!(session.harness.now >= Timestamp::from_millis(30));
        assert_eq!(session.result(), Ok(()));
    }
}
