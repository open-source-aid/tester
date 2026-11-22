//! What a run does to the sensor, and what it expects back.
//!
//! The pendant of [`crate::scenarios`] for the CGM side. Each one says in its
//! name what it is after; when it does not get it, the message names what it
//! wanted and what it got, and when it cannot even try — no console on the
//! board, no secret on the command line, a pairing the simulator cannot run —
//! it is skipped with the reason rather than reported as a driver fault.

use core::time::Duration;

use cgm_shared::{
    CgmCommand, CgmFailure, CgmOutcome, CgmReport, EngineState, GlucoseReading, PairReport,
    Refusal, SensorState,
};

use crate::cgm::session::{CgmBoard, CgmSession};

pub type CgmScenario<D> = (&'static str, fn(&mut CgmSession<D>));

/// One reading every five minutes, on both generations.
const CYCLE: Duration = Duration::from_secs(300);

/// How long the gap the backfill scenario cuts into the stream is.
const GAP: Duration = Duration::from_secs(1_800);

const NO_SECRET: &str =
    "no credential and no pairing secret: pass --credential for a sensor already paired, or --pairing-secret for one in hand";
const NO_CREDENTIAL: &str =
    "no credential to reconnect with: pass --credential, or run the pairing scenario first";
const NO_CONSOLE: &str =
    "needs the board's console to move its clock and cut the stream: pass --console <serial port>";

/// Why a fresh G7 pairing cannot finish against the simulator.
///
/// The sensor side of the EC-J-PAKE staircase is a **frozen fixture**: the
/// simulator replays bytes computed once with fixed scalars, because running
/// the exchange for real would mean either calling the driver's own
/// implementation — which guideline 11 forbids, a round trip proving only
/// that one implementation agrees with itself — or writing a second one. A
/// host draws fresh scalars on every attempt, so the two cannot agree, and
/// the fixture answers a round the driver never asked.
///
/// What this does exercise, and what the run checks, is the framing: the
/// order of the steps, the fragment reassembly on the certificate
/// characteristic, and that the driver gives up cleanly instead of hanging.
/// A real sensor, or a simulator that runs the exchange, is the only thing
/// that can prove the arithmetic.
const FROZEN_PAIRING: &str =
    "a fresh G7 pairing cannot finish against the simulator: its EC-J-PAKE packets are frozen bytes computed with fixed scalars, and a real host draws its own — the framing was driven, the agreement cannot be. Known simulator limit, not a driver fault";

/// Every scenario, in the order a full run does them.
///
/// Pairing first, because everything else needs a session; the fresh pairing
/// last, because it is the one that deliberately throws the session away.
pub fn all<D: CgmBoard>() -> Vec<CgmScenario<D>> {
    vec![
        ("pair", pair),
        ("status", status),
        ("live readings", live),
        ("backfill after a gap", backfill),
        ("full cycle", cycle),
        ("fresh pairing", fresh),
    ]
}

/// The scenarios `--scenario <name>` selects, `None` for a name nobody wrote.
pub fn selected<D: CgmBoard>(name: &str) -> Option<Vec<CgmScenario<D>>> {
    if name == "all" {
        return Some(all());
    }
    all()
        .into_iter()
        .find(|(scenario, _)| *scenario == name)
        .map(|scenario| vec![scenario])
}

/// Every scenario name, for the usage line.
pub fn names() -> Vec<&'static str> {
    let mut names = vec!["all"];
    names.extend(all::<Never>().into_iter().map(|(name, _)| name));
    names
}

/// A board that exists only so a scenario list can be asked for without
/// naming a transport: it has no values, so none of its methods can run.
pub enum Never {}

impl cgm_shared::SimDevice for Never {
    fn handle(
        &mut self,
        _op: &cgm_shared::BleOp,
        _now: cgm_shared::Timestamp,
    ) -> cgm_shared::BleResult {
        match *self {}
    }
}

impl CgmBoard for Never {
    fn console(&mut self, _line: &str, _now: cgm_shared::Timestamp) -> Result<(), String> {
        match *self {}
    }

    fn has_console(&self) -> bool {
        match *self {}
    }
}

/// Binds to the sensor with the credential kept from an earlier pairing.
fn pair<D: CgmBoard>(session: &mut CgmSession<D>) {
    if session.setup.credential.is_none() {
        if session.engine.state() == EngineState::Paired {
            session.note("already paired from the stored session".into());
            return;
        }
        return session.skip(NO_CREDENTIAL);
    }
    let command = session.setup.pair();
    let outcome = session.command(command);
    paired(session, outcome);
}

/// Reports what the sensor says about itself.
fn status<D: CgmBoard>(session: &mut CgmSession<D>) {
    if !ready(session) {
        return;
    }
    let Some(status) = session.status() else {
        return;
    };
    session.note(format!(
        "state {:?}, activation {:?}, latest sequence {:?}, battery {:?}",
        status.state, status.activation, status.latest_sequence, status.battery_percent
    ));
    session.check(
        status.state != SensorState::Unknown,
        "state Unknown: the driver could not work out where the sensor is in its life".into(),
    );
}

/// Watches the live stream for the time `--minutes` asked for.
fn live<D: CgmBoard>(session: &mut CgmSession<D>) {
    if !ready(session) {
        return;
    }
    // A command the sensor answers puts the link up; readings follow on their
    // own, pushed, with nothing asked for (E3).
    if session.status().is_none() {
        return;
    }
    let before = session.readings().len();
    let minutes = session.setup.minutes.max(1);
    for _ in 0..minutes.div_ceil(CYCLE.as_secs() / 60) {
        advance(session, CYCLE);
    }
    let readings = session.readings();
    let new = readings.len() - before.min(readings.len());
    let newest = readings
        .last()
        .map(|reading| describe(reading))
        .unwrap_or_else(|| "none".into());
    session.note(format!(
        "{new} reading(s) over {minutes} minute(s), newest {newest}"
    ));
    session.check(
        new > 0,
        format!(
            "no reading in {minutes} minute(s): the sensor pushes one every {} s",
            CYCLE.as_secs()
        ),
    );
}

/// Cuts a gap into the stream and fetches what was missed.
fn backfill<D: CgmBoard>(session: &mut CgmSession<D>) {
    if !ready(session) {
        return;
    }
    if !session.has_console() {
        return session.skip(NO_CONSOLE);
    }
    if session.status().is_none() {
        return;
    }
    // Silence, then one cycle of signal again, which is what tells the engine
    // it missed something.
    session.console(&format!("signal-loss {}", GAP.as_secs()));
    advance(session, GAP);
    if session.status().is_none() {
        return;
    }
    advance(session, CYCLE);
    advance(session, CYCLE);
    let Some(after) = session.gaps().last().copied() else {
        return session.check(
            false,
            format!(
                "no gap announced after {} minutes of silence: the engine should report BackfillAvailable",
                GAP.as_secs() / 60
            ),
        );
    };
    let outcome = session.command(CgmCommand::ReadReadings { after });
    let CgmOutcome::Completed(CgmReport::Readings(readings)) = outcome else {
        return session.check(
            false,
            format!(
                "backfill after {}: {outcome:?}, wanted readings",
                after.get()
            ),
        );
    };
    session.note(format!(
        "{} record(s) after sequence {}",
        readings.len(),
        after.get()
    ));
    session.check(
        !readings.is_empty(),
        format!(
            "the gap after {} was announced and came back empty",
            after.get()
        ),
    );
    session.check(
        readings.iter().all(|reading| reading.historical),
        "a record read from history is not marked historical".into(),
    );
    if let Some(newest) = readings.iter().map(|reading| reading.sequence).max() {
        session.seen = newest;
    }
}

/// Everything in order, from a clean start to an unpair.
fn cycle<D: CgmBoard>(session: &mut CgmSession<D>) {
    if !ready(session) {
        return;
    }
    if session.status().is_none() {
        return;
    }
    advance(session, CYCLE);
    // What was stored is what a new process comes up with: the point of
    // keeping the slots on disk at all.
    if let Err(error) = session.restart() {
        return session.check(false, error);
    }
    session.check(
        session.engine.state() == EngineState::Paired,
        format!(
            "an engine rebuilt from {} came up {:?}, wanted Paired",
            session.store.dir().display(),
            session.engine.state()
        ),
    );
    if session.status().is_none() {
        return;
    }
    let outcome = session.command(CgmCommand::Unpair);
    session.check(
        outcome == CgmOutcome::Completed(CgmReport::Done),
        format!("unpair: {outcome:?}, wanted Done"),
    );
    let after = session.command(CgmCommand::Status);
    session.check(
        after == CgmOutcome::Refused(Refusal::NotPaired),
        format!("status after an unpair: {after:?}, wanted NotPaired"),
    );
    // And the next run must not find a session on disk either.
    match session.store.load() {
        Ok(slots) => session.check(
            !slots.contains_key(&cgm_shared::Slot::Session),
            format!(
                "the unpaired session is still in {}",
                session.store.dir().display()
            ),
        ),
        Err(error) => session.check(false, error),
    }
    // Leave the sensor as it was found, so a run can be repeated.
    pair(session);
}

/// Pairs from the printed secret alone, as someone holding a new sensor does.
fn fresh<D: CgmBoard>(session: &mut CgmSession<D>) {
    if session.setup.pairing_secret.is_none() {
        return session.skip(NO_SECRET);
    }
    // The credential this run came with is put back afterwards, so throwing
    // the session away to try a fresh pairing cannot cost the user the one
    // they had.
    let kept = session.harness.store.clone();
    if let Err(error) = session.forget() {
        return session.check(false, error);
    }
    let command = session.setup.fresh_pair();
    let outcome = session.command(command);
    let fresh_g7 = !session.setup.model.is_g6();
    match (&outcome, fresh_g7) {
        (CgmOutcome::Completed(CgmReport::Paired(report)), _) => {
            let report = report.clone();
            session.note(credential_note(&report));
            session.note(format!("started a new sensor: {}", report.started_sensor));
        }
        // The one failure this run is content with, and why.
        (CgmOutcome::Failed(failure), true) => {
            session.note(format!("the exchange ended {failure:?}"));
            session.skip(FROZEN_PAIRING);
            restore(session, kept);
            return;
        }
        _ => session.check(
            false,
            format!("fresh pairing: {outcome:?}, wanted a PairReport"),
        ),
    }
    restore(session, kept);
}

/// Puts the slots a scenario borrowed back where they were.
fn restore<D: CgmBoard>(session: &mut CgmSession<D>, kept: crate::cgm::store::Slots) {
    if kept.is_empty() {
        return;
    }
    session.harness.store = kept;
    let put_back = session.persist().and_then(|()| session.restart());
    if let Err(error) = put_back {
        session.check(false, error);
    }
}

/// Makes sure there is a session to work with, skipping with the reason when
/// there cannot be one.
fn ready<D: CgmBoard>(session: &mut CgmSession<D>) -> bool {
    if session.engine.state() == EngineState::Paired {
        return true;
    }
    if session.setup.credential.is_none() {
        session.skip(NO_SECRET);
        return false;
    }
    let command = session.setup.pair();
    let outcome = session.command(command);
    paired(session, outcome)
}

/// Checks a `Pair` outcome and says what came back.
fn paired<D: CgmBoard>(session: &mut CgmSession<D>, outcome: CgmOutcome) -> bool {
    match outcome {
        CgmOutcome::Completed(CgmReport::Paired(report)) => {
            session.note(format!(
                "paired: activation {:?}, serial {:?}, started the sensor: {}",
                report.activation, report.product_serial, report.started_sensor
            ));
            true
        }
        CgmOutcome::Failed(CgmFailure::KeyRejected) => {
            session.check(
                false,
                "the sensor refused the credential: is --credential the key this sensor was paired with?".into(),
            );
            false
        }
        other => {
            session.check(false, format!("pair: {other:?}, wanted a PairReport"));
            false
        }
    }
}

/// The agreed credential, in the form `--credential` takes it back.
fn credential_note(report: &PairReport) -> String {
    let hex: String = report
        .credential
        .expose()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("keep this credential: --credential {hex}")
}

/// Moves time on: the board's clock where there is a console, the real one
/// otherwise, because nothing else can make a sensor transmit sooner.
fn advance<D: CgmBoard>(session: &mut CgmSession<D>, by: Duration) {
    if session.has_console() {
        session.skip_ahead(by);
    } else {
        session.wait(by);
    }
}

fn describe(reading: &GlucoseReading) -> String {
    format!(
        "sequence {} value {:?} trend {:?} quality {:#06b}",
        reading.sequence.get(),
        reading.value.map(|value| value.get()),
        reading.trend,
        reading.quality.bits()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cgm::session::tests::{session, CODE, TRANSMITTER};
    use crate::cgm::{credential, Model};
    use crate::session::Error;

    fn run(model: Model, name: &str, with_credential: bool) -> (Vec<String>, Result<(), Error>) {
        let mut run = session(model, name);
        if with_credential {
            let key: String = run
                .harness
                .device
                .device
                .sim
                .key()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let text = if model.is_g6() { TRANSMITTER } else { &key };
            run.setup.credential = Some(credential(model, text).expect("a credential"));
        }
        let (_, scenario) = all()
            .into_iter()
            .find(|(scenario, _)| *scenario == name)
            .expect("a scenario");
        scenario(&mut run);
        let result = run.result();
        (run.take_notes(), result)
    }

    #[test]
    fn every_scenario_is_selectable_by_name_and_nothing_else_is() {
        assert_eq!(
            selected::<Never>("all").map(|s| s.len()),
            Some(all::<Never>().len())
        );
        for (name, _) in all::<Never>() {
            let one = selected::<Never>(name).expect(name);
            assert_eq!(one.len(), 1);
            assert_eq!(one[0].0, name);
        }
        assert!(selected::<Never>("backfil").is_none());
        assert!(selected::<Never>("").is_none());
        assert_eq!(names()[0], "all");
        assert_eq!(names().len(), all::<Never>().len() + 1);
    }

    #[test]
    fn pairing_and_status_pass_against_the_simulated_sensor() {
        for model in [Model::G6, Model::G7] {
            let (notes, result) = run(model, "pair", true);
            assert_eq!(result, Ok(()), "{model}");
            assert!(notes[0].starts_with("paired:"), "{notes:?}");
            let (notes, result) = run(model, "status", true);
            assert_eq!(result, Ok(()), "{model}: {notes:?}");
            assert!(
                notes.iter().any(|note| note.contains("state ")),
                "{notes:?}"
            );
        }
    }

    #[test]
    fn the_live_and_backfill_scenarios_see_what_the_sensor_sends() {
        let (notes, result) = run(Model::G7, "live readings", true);
        assert_eq!(result, Ok(()), "{notes:?}");
        assert!(
            notes.iter().any(|note| note.contains("reading(s) over")),
            "{notes:?}"
        );
        let (notes, result) = run(Model::G7, "backfill after a gap", true);
        assert_eq!(result, Ok(()), "{notes:?}");
        assert!(
            notes
                .iter()
                .any(|note| note.contains("record(s) after sequence")),
            "{notes:?}"
        );
    }

    #[test]
    fn the_full_cycle_unpairs_and_pairs_again() {
        let (notes, result) = run(Model::G7, "full cycle", true);
        assert_eq!(result, Ok(()), "{notes:?}");
    }

    #[test]
    fn a_fresh_g7_pairing_is_skipped_with_the_reason_rather_than_failing() {
        let (notes, result) = run(Model::G7, "fresh pairing", true);
        assert_eq!(result, Err(Error::Skipped(FROZEN_PAIRING)), "{notes:?}");
        assert!(
            FROZEN_PAIRING.contains("frozen bytes"),
            "the reason names the limit"
        );
    }

    #[test]
    fn a_fresh_g6_pairing_works_from_the_transmitter_id() {
        let (notes, result) = run(Model::G6, "fresh pairing", false);
        assert_eq!(result, Ok(()), "{notes:?}");
        assert!(
            notes
                .iter()
                .any(|note| note.starts_with("keep this credential: --credential ")),
            "{notes:?}"
        );
        assert_eq!(
            notes.iter().filter(|note| note.contains(CODE)).count(),
            0,
            "a G6 has no pairing code"
        );
    }

    #[test]
    fn a_run_with_no_secret_at_all_says_so_rather_than_failing() {
        for name in [
            "pair",
            "status",
            "live readings",
            "backfill after a gap",
            "full cycle",
        ] {
            let (_, result) = run(Model::G7, name, false);
            assert!(
                matches!(result, Err(Error::Skipped(_))),
                "{name}: {result:?}"
            );
        }
    }

    #[test]
    fn a_backfill_without_a_console_is_skipped_with_what_to_pass() {
        let mut run = session(Model::G7, "no-console");
        let key: String = run
            .harness
            .device
            .device
            .sim
            .key()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        run.setup.credential = Some(credential(Model::G7, &key).expect("a credential"));
        run.harness.device.device.deaf = true;
        backfill(&mut run);
        assert_eq!(run.result(), Err(Error::Skipped(NO_CONSOLE)));
    }
}
