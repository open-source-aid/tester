//! The host loop for a CGM engine: the pendant of [`crate::session`].
//!
//! The engine is sans-IO and owns neither clock nor radio, so everything it
//! learns about the world it learns because this loop tells it. `cgm-dexcom`'s
//! engine module writes down what a host owes it, and this is the whole list:
//!
//! * drain `poll_event`, then `poll_action`, perform the action and answer
//!   with `handle_completion` (guideline 04's loop, run by [`Harness`]);
//! * call `handle_timeout` at the instant `poll_timeout` asks for. Without it
//!   the engine cannot notice silence: `SensorState::Active` would stop
//!   meaning *a reading arrived recently* and decay into *the wear period has
//!   not elapsed*, which stays true long after a sensor has gone quiet;
//! * call `handle_wall_clock`, because the engine reads no clock and cannot
//!   date a reading — or tell warm-up from expiry — without one;
//! * call `handle_link_lost` as soon as the link is known to be gone, so the
//!   sensor is reported `SignalLost` at once rather than after the countdown.
//!
//! The last one needs a radio that says so. [`Watched`] wraps the device and
//! remembers that an operation came back `Disconnected`, which is what the
//! btleplug layer answers once the peripheral is gone; the loop then tells the
//! engine, once per drop.

use core::time::Duration;

use cgm_shared::{
    Address, BleOp, BleResult, CgmCommand, CgmEngine, CgmEvent, CgmOutcome, CgmReport, CgmStatus,
    CommandId, GlucoseReading, Notification, Secret, Sequence, SimDevice, Timestamp, UtcTime,
    Variant,
};
use tester_harness::{CgmHost, Harness};

use crate::cgm::store::{SlotStore, Slots};
use crate::cgm::Model;
use crate::session::{Clock, Error};

/// How long a command may take before the run gives up on it.
const LIMIT: Duration = Duration::from_secs(300);

/// A board hosting a simulated sensor: a radio, and the serial console that
/// drives the scenario knobs.
pub trait CgmBoard: SimDevice {
    fn console(&mut self, line: &str, now: Timestamp) -> Result<(), String>;

    /// Whether there is a console at all.
    ///
    /// A scenario that needs a knob — a gap in the stream, a clock skipped
    /// forward — asks before it tries, so a run without `--console` says what
    /// it could not do instead of failing on a port that is not there.
    fn has_console(&self) -> bool;
}

/// What the user aimed this run at.
#[derive(Debug, Clone)]
pub struct Setup {
    pub model: Model,
    pub name: String,
    pub address: Address,
    pub variant: &'static Variant,
    /// The key kept from an earlier pairing, when there is one.
    pub credential: Option<Secret>,
    /// The code printed on the device, when the user read it off.
    pub pairing_secret: Option<Secret>,
    pub expected_serial: Option<String>,
    /// How long the live-reading scenario watches for.
    pub minutes: u64,
}

impl Setup {
    /// The `Pair` this run's secrets describe.
    pub fn pair(&self) -> CgmCommand {
        CgmCommand::Pair {
            address: self.address,
            credential: self.credential.clone(),
            pairing_secret: self.pairing_secret.clone(),
            expected_serial: self.expected_serial.clone(),
            known_activation: None,
        }
    }

    /// The `Pair` of someone who has never seen this sensor: the printed
    /// secret alone.
    pub fn fresh_pair(&self) -> CgmCommand {
        CgmCommand::Pair {
            address: self.address,
            credential: None,
            pairing_secret: self.pairing_secret.clone(),
            expected_serial: self.expected_serial.clone(),
            known_activation: None,
        }
    }
}

/// Remembers that the link dropped, so the loop can tell the engine.
#[derive(Debug)]
pub struct Watched<D> {
    pub device: D,
    lost: bool,
}

impl<D> Watched<D> {
    pub fn new(device: D) -> Self {
        Self {
            device,
            lost: false,
        }
    }

    /// Whether the link dropped since this was last asked.
    pub fn took_a_drop(&mut self) -> bool {
        core::mem::take(&mut self.lost)
    }
}

impl<D: SimDevice> SimDevice for Watched<D> {
    fn handle(&mut self, op: &BleOp, now: Timestamp) -> BleResult {
        let result = self.device.handle(op, now);
        // A disconnect we asked for is not a drop: the engine knows it let go.
        if result == BleResult::Disconnected && !matches!(op, BleOp::Disconnect) {
            self.lost = true;
        }
        result
    }

    fn poll_notification(&mut self, now: Timestamp) -> Option<Notification> {
        self.device.poll_notification(now)
    }
}

impl<D: CgmBoard> CgmBoard for Watched<D> {
    fn console(&mut self, line: &str, now: Timestamp) -> Result<(), String> {
        self.device.console(line, now)
    }

    fn has_console(&self) -> bool {
        self.device.has_console()
    }
}

/// One run against one sensor.
pub struct CgmSession<D> {
    pub harness: Harness<Watched<D>, CgmEvent>,
    pub engine: Box<dyn CgmEngine>,
    pub setup: Setup,
    pub store: SlotStore,
    /// The newest sequence this run has reported, for the next backfill.
    pub seen: Sequence,
    clock: Clock,
    /// Unix milliseconds at `Timestamp::default()`, so the wall clock the
    /// engine is given advances with the simulated one.
    wall_base: u64,
    failure: Option<Error>,
    notes: Vec<String>,
}

impl<D: CgmBoard> CgmSession<D> {
    /// Opens a session, loading whatever an earlier run stored.
    pub fn new(device: D, setup: Setup, store: SlotStore, clock: Clock) -> Result<Self, String> {
        let slots = store.load()?;
        let mut harness = Harness::new(Watched::new(device));
        harness.announced = Some(&cgm_dexcom::DRIVER.info().device);
        harness.store = slots;
        let mut session = Self {
            engine: crate::cgm::engine(setup.variant, setup.address),
            harness,
            setup,
            store,
            seen: Sequence::new(0),
            clock,
            wall_base: wall_base(clock),
            failure: None,
            notes: Vec::new(),
        };
        session.wait(Duration::ZERO);
        Ok(session)
    }

    /// The first failure this scenario hit, and a clean slate for the next.
    pub fn result(&mut self) -> Result<(), Error> {
        let result = self.failure.take().map_or(Ok(()), Err);
        if let Err(error) = self.persist() {
            return result.and(Err(Error::Unexpected(error)));
        }
        result
    }

    /// Something the run found out and the user wants to read: the credential
    /// a fresh pairing agreed, the state a sensor reported, how many readings
    /// arrived. Printed under the scenario's line, pass or fail.
    pub fn note(&mut self, line: String) {
        self.notes.push(line);
    }

    pub fn take_notes(&mut self) -> Vec<String> {
        core::mem::take(&mut self.notes)
    }

    /// Whether the board can be driven from the serial console.
    pub fn has_console(&self) -> bool {
        self.harness.device.has_console()
    }

    pub fn skip(&mut self, why: &'static str) {
        self.fail(Error::Skipped(why));
    }

    pub fn check(&mut self, holds: bool, what: String) {
        if !holds {
            self.fail(Error::Unexpected(what));
        }
    }

    pub fn events(&self) -> &[CgmEvent] {
        &self.harness.events
    }

    /// Starts again from what is on disk, as a fresh process would: the way a
    /// stored credential is shown to be a credential and not a memory.
    pub fn restart(&mut self) -> Result<(), String> {
        self.wait(Duration::ZERO);
        self.harness
            .device
            .handle(&BleOp::Disconnect, self.harness.now);
        self.engine.handle_link_lost();
        self.persist()?;
        self.harness.restart();
        self.harness.store = self.store.load()?;
        self.engine = crate::cgm::engine(self.setup.variant, self.setup.address);
        self.wait(Duration::ZERO);
        Ok(())
    }

    /// Forgets the sensor on disk and in memory.
    pub fn forget(&mut self) -> Result<(), String> {
        self.harness.store.clear();
        self.store.forget()?;
        self.restart()
    }

    pub fn submit(&mut self, command: CgmCommand) -> Option<CommandId> {
        self.step(Timestamp::default());
        if self.failure.is_some() {
            return None;
        }
        match self.engine.submit(command) {
            Ok(id) => Some(id),
            Err(_) => {
                self.fail(Error::Unqueued);
                None
            }
        }
    }

    /// Runs a submitted command to its single `Finished` event (E5).
    pub fn finish(&mut self, id: Option<CommandId>) -> CgmOutcome {
        let Some(id) = id else {
            self.fail(Error::Unqueued);
            return CgmOutcome::Unknown;
        };
        let until = self.later(LIMIT);
        loop {
            let more = self.step(until);
            if let Some(outcome) = self.outcome(id) {
                let _ = self.persist();
                return outcome;
            }
            if !more {
                self.fail(Error::Unfinished(id));
                return CgmOutcome::Unknown;
            }
        }
    }

    pub fn command(&mut self, command: CgmCommand) -> CgmOutcome {
        let id = self.submit(command);
        self.finish(id)
    }

    /// Sends a scenario knob to the board, if there is a console.
    pub fn console(&mut self, line: &str) {
        self.sync();
        let now = self.harness.now;
        if self.failure.is_none() {
            if let Err(error) = self.harness.device.console(line, now) {
                self.fail(Error::Console(error));
            }
        }
    }

    /// Lets the engine run for `by`, keeping every deadline it asks for.
    ///
    /// On a real clock this is a real wait, because a sensor transmits every
    /// five minutes and nothing can make it hurry — except the board's own
    /// `advance`, which [`Self::skip_ahead`] uses where it is available.
    pub fn wait(&mut self, by: Duration) {
        let until = self.later(by);
        while self.step(until) {}
        self.reach(until);
        // One more turn once the clock is there: what the sensor pushed at
        // the end of the window is delivered in this run, not the next one.
        while self.step(until) {}
    }

    /// Moves the board's simulated clock and ours by the same amount.
    ///
    /// The board console's `advance` is what makes a five-minute reading cycle
    /// testable in a run that must finish while someone is watching it.
    pub fn skip_ahead(&mut self, by: Duration) {
        let seconds = by.as_secs();
        if seconds > 0 {
            self.console(&format!("advance {seconds}"));
        }
        if let Clock::Real(start) = self.clock {
            self.clock = Clock::Real(start - by);
            self.wait(Duration::ZERO);
        } else {
            self.wait(by);
        }
    }

    /// Every reading this run has seen, live or backfilled.
    pub fn readings(&self) -> Vec<&GlucoseReading> {
        self.events()
            .iter()
            .filter_map(|event| match event {
                CgmEvent::Reading(reading) => Some(reading),
                _ => None,
            })
            .collect()
    }

    /// The gaps the engine announced and nobody has fetched yet (E3: it will
    /// never fetch them on its own).
    pub fn gaps(&self) -> Vec<Sequence> {
        self.events()
            .iter()
            .filter_map(|event| match event {
                CgmEvent::BackfillAvailable { after } => Some(*after),
                _ => None,
            })
            .collect()
    }

    /// The status of a command's outcome, or a recorded failure.
    pub fn status(&mut self) -> Option<CgmStatus> {
        let outcome = self.command(CgmCommand::Status);
        match outcome {
            CgmOutcome::Completed(CgmReport::Status(status)) => Some(status),
            other => {
                self.check(false, format!("status: {other:?}"));
                None
            }
        }
    }

    /// Writes the slots the engine changed back to disk.
    pub fn persist(&mut self) -> Result<(), String> {
        let slots: Slots = self.harness.store.clone();
        self.store.save(&slots)
    }

    fn fail(&mut self, error: Error) {
        self.failure.get_or_insert(error);
    }

    fn outcome(&self, id: CommandId) -> Option<CgmOutcome> {
        self.events().iter().find_map(|event| match event {
            CgmEvent::Finished { id: done, outcome } if *done == id => Some(outcome.clone()),
            _ => None,
        })
    }

    fn later(&mut self, by: Duration) -> Timestamp {
        self.sync();
        Timestamp::from_millis(self.harness.now.as_millis() + by.as_millis() as u64)
    }

    /// One turn of the host loop: actions, then the nearest deadline that is
    /// due. Answers whether there is more to do before `until`.
    fn step(&mut self, until: Timestamp) -> bool {
        self.sync();
        if self.failure.is_some() {
            return false;
        }
        self.engine.handle_wall_clock(self.wall());
        if let Err(violation) = self.harness.run(&mut CgmHost(self.engine.as_mut())) {
            self.fail(Error::Violation(violation));
            return false;
        }
        if self.harness.device.took_a_drop() {
            self.engine.handle_link_lost();
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

    fn wall(&self) -> UtcTime {
        UtcTime::from_unix_millis(self.wall_base.saturating_add(self.harness.now.as_millis()))
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

/// The unix time at `Timestamp::default()`: where the engine's wall clock
/// starts, so that it moves with the loop's own clock afterwards.
fn wall_base(clock: Clock) -> u64 {
    match clock {
        Clock::Real(start) => crate::cgm::wall_clock_now()
            .as_unix_millis()
            .saturating_sub(start.elapsed().as_millis() as u64),
        Clock::Virtual => crate::cgm::wall_clock_now().as_unix_millis(),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::cgm::store::tests::scratch;
    use cgm_dexcom::sim::{DexcomSim, SimConfig, SimModel};
    use cgm_shared::sim::Constant;
    use cgm_shared::{CgmFailure, MgDl, Refusal};
    use std::time::Instant;

    /// The simulated sensor's own secrets, the defaults of `SimConfig`.
    pub const CODE: &str = "1234";
    pub const TRANSMITTER: &str = "ABCDEF";

    /// A board hosting the simulator in process, so the loop itself can be
    /// tested without a radio. The radio is the one thing here that cannot
    /// be; everything above it is ordinary code.
    pub struct Bench {
        pub sim: DexcomSim,
        /// Console lines the board was sent, in order.
        pub lines: Vec<String>,
        /// Refuse every console line, as a board with no serial port does.
        pub deaf: bool,
    }

    impl SimDevice for Bench {
        fn handle(&mut self, op: &BleOp, now: Timestamp) -> BleResult {
            self.sim.handle(op, now)
        }

        fn poll_notification(&mut self, now: Timestamp) -> Option<Notification> {
            self.sim.poll_notification(now)
        }
    }

    impl CgmBoard for Bench {
        fn console(&mut self, line: &str, _now: Timestamp) -> Result<(), String> {
            if self.deaf {
                return Err(format!("console refused: {line}"));
            }
            self.lines.push(line.to_owned());
            Ok(())
        }

        fn has_console(&self) -> bool {
            !self.deaf
        }
    }

    pub fn sim(model: Model) -> Bench {
        let generation = if model.is_g6() {
            cgm_dexcom::protocol::Generation::G6
        } else {
            cgm_dexcom::protocol::Generation::G7
        };
        let mut config = SimConfig::new(generation);
        config.model = match model {
            Model::G6 => SimModel::G6,
            Model::G7 => SimModel::G7,
            Model::OnePlus => SimModel::OnePlus,
            Model::Flex => SimModel::Flex,
        };
        Bench {
            sim: DexcomSim::new(config, Box::new(Constant(MgDl(120)))),
            lines: Vec::new(),
            deaf: false,
        }
    }

    pub fn setup(model: Model) -> Setup {
        let printed = if model.is_g6() { TRANSMITTER } else { CODE };
        let name = model.advertised_name(printed).expect("a name");
        Setup {
            model,
            variant: crate::cgm::variant(model, &name).expect("a variant"),
            name,
            address: Address::new(crate::cgm::ADDRESS),
            credential: None,
            pairing_secret: Some(crate::cgm::pairing_secret(model, printed).expect("a secret")),
            expected_serial: None,
            minutes: 1,
        }
    }

    /// A session against the simulated sensor, on virtual time.
    pub fn session(model: Model, what: &str) -> CgmSession<Bench> {
        let store = SlotStore::open(scratch(what)).expect("a store");
        CgmSession::new(sim(model), setup(model), store, Clock::Virtual).expect("a session")
    }

    /// The key the G7 simulator authenticates with, as hex: its seeded key.
    fn g7_key(model: Model) -> String {
        let bench = sim(model);
        bench.sim.key().iter().map(|b| format!("{b:02x}")).collect()
    }

    fn paired(model: Model, what: &str) -> CgmSession<Bench> {
        let mut session = session(model, what);
        let credential = if model.is_g6() {
            crate::cgm::credential(model, TRANSMITTER)
        } else {
            crate::cgm::credential(model, &g7_key(model))
        };
        session.setup.credential = Some(credential.expect("a credential"));
        let outcome = session.command(session.setup.pair());
        assert!(
            matches!(outcome, CgmOutcome::Completed(CgmReport::Paired(_))),
            "{model}: {outcome:?}"
        );
        session
    }

    #[test]
    fn drives_the_engine_through_a_pairing_and_a_status() {
        for model in [Model::G6, Model::G7] {
            let mut session = paired(model, "paired");
            let status = session.status().expect("a status");
            assert!(status.latest_sequence.is_some() || status.activation.is_some());
            assert_eq!(session.result(), Ok(()));
        }
    }

    #[test]
    fn what_was_paired_is_still_paired_in_the_next_process() {
        let mut session = paired(Model::G7, "persists");
        let slots = session.store.load().expect("the slots");
        assert!(slots.contains_key(&cgm_shared::Slot::Session));
        // A new engine, built from the directory alone, needs no credential.
        session.setup.credential = None;
        session.restart().expect("a restart");
        assert!(session.status().is_some());
        assert_eq!(session.result(), Ok(()));

        // And once forgotten, it is unpaired again.
        session.forget().expect("a forget");
        assert_eq!(
            session.command(CgmCommand::Status),
            CgmOutcome::Refused(Refusal::NotPaired)
        );
        assert!(session.store.load().expect("the slots").is_empty());
    }

    #[test]
    fn a_wrong_credential_is_a_rejected_key_not_a_crash() {
        let mut session = session(Model::G7, "wrong-key");
        session.setup.credential = Some(Secret::new(vec![0; 16]));
        let outcome = session.command(session.setup.pair());
        assert_eq!(outcome, CgmOutcome::Failed(CgmFailure::KeyRejected));
    }

    #[test]
    fn readings_arrive_while_the_link_is_up() {
        let mut session = paired(Model::G7, "readings");
        session.wait(Duration::from_secs(600));
        assert!(!session.readings().is_empty(), "{:?}", session.events());
        assert_eq!(session.result(), Ok(()));
    }

    #[test]
    fn a_link_that_drops_is_reported_to_the_engine_at_once() {
        let mut session = paired(Model::G7, "link-lost");
        // The simulator drops the link on the next operation.
        session
            .harness
            .device
            .device
            .sim
            .config_mut()
            .faults
            .disconnect_after_ops = Some(0);
        let outcome = session.command(CgmCommand::ReadReadings {
            after: Sequence::new(0),
        });
        assert!(
            matches!(outcome, CgmOutcome::Failed(_)),
            "history over a dead link fails: {outcome:?}"
        );
        session
            .harness
            .device
            .device
            .sim
            .config_mut()
            .faults
            .disconnect_after_ops = None;
        // The next command reconnects: the engine was told, and did not sulk.
        let again = session.command(CgmCommand::Status);
        assert!(
            matches!(again, CgmOutcome::Completed(_)),
            "the engine reconnects lazily: {again:?}"
        );
    }

    /// A device that answers every operation with a dropped link.
    struct Gone;

    impl SimDevice for Gone {
        fn handle(&mut self, _op: &BleOp, _now: Timestamp) -> BleResult {
            BleResult::Disconnected
        }
    }

    #[test]
    fn a_drop_is_remembered_once_but_a_disconnect_we_asked_for_is_not() {
        let mut watched = Watched::new(Gone);
        let now = Timestamp::default();
        assert_eq!(
            watched.handle(&BleOp::Disconnect, now),
            BleResult::Disconnected
        );
        assert!(!watched.took_a_drop(), "we asked for that one");
        assert_eq!(
            watched.handle(
                &BleOp::Connect {
                    address: Address::new(crate::cgm::ADDRESS)
                },
                now
            ),
            BleResult::Disconnected
        );
        assert!(watched.took_a_drop());
        assert!(!watched.took_a_drop(), "reported once");
        assert_eq!(watched.poll_notification(now), None);
    }

    #[test]
    fn a_console_the_board_refuses_is_reported_with_the_line() {
        let mut session = session(Model::G7, "deaf");
        session.harness.device.device.deaf = true;
        session.console("advance 300");
        assert_eq!(
            session.result(),
            Err(Error::Console("console refused: advance 300".into()))
        );
    }

    #[test]
    fn a_command_that_was_never_queued_is_reported() {
        let mut session = session(Model::G7, "unqueued");
        assert_eq!(session.finish(None), CgmOutcome::Unknown);
        assert_eq!(session.result(), Err(Error::Unqueued));
        assert_eq!(
            session.finish(Some(CommandId::new(99))),
            CgmOutcome::Unknown
        );
        assert_eq!(session.result(), Err(Error::Unfinished(CommandId::new(99))));
    }

    #[test]
    fn a_skip_explains_itself_and_the_first_failure_wins() {
        let mut session = session(Model::G7, "skips");
        session.skip("no console");
        session.check(false, "later".into());
        assert_eq!(session.result(), Err(Error::Skipped("no console")));
        assert_eq!(session.result(), Ok(()));
    }

    #[test]
    fn the_board_clock_is_skipped_ahead_rather_than_waited_out() {
        let mut session = session(Model::G7, "advance");
        session.skip_ahead(Duration::from_secs(600));
        assert_eq!(session.harness.device.device.lines, ["advance 600"]);
        assert!(session.harness.now >= Timestamp::from_millis(600_000));
        assert_eq!(session.result(), Ok(()));
    }

    #[test]
    fn a_real_clock_keeps_the_wall_clock_in_step_with_it() {
        let clock = Clock::Real(Instant::now());
        // A run that started a moment ago dates `Timestamp::default()` a
        // moment ago, not now, so the two clocks tell the same story.
        assert!(wall_base(clock) <= crate::cgm::wall_clock_now().as_unix_millis());
        assert!(wall_base(Clock::Virtual) > 1_700_000_000_000);
    }
}
