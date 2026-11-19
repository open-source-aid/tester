use core::time::Duration;

use pump_shared::{
    Basal, BasalProfile, BasalRate, BasalSegment, Bolus, BolusKind, Command, CommandId,
    EngineState, Event, Extended, Failure, HistoryEntry, HistoryEvent, InsulinAmount, Notification,
    Outcome, Percent, PumpMode, PumpStatus, Refusal, Report, Sequence, Slot, TempBasalPhase,
    TempBasalRate, UtcTime,
};
use pump_ypso::protocol::{setting, uuids, KEY_LEN};
use tester_harness::Fault;

use crate::args::Mode;
use crate::session::{Board, Session};
use crate::ypso::{self, SERIAL};

pub type Scenario<D> = (&'static str, fn(&mut Session<D>));

const NO_COUNTERS: &str =
    "an OPN pump is driven in clear: it has no session key, counters or epoch";
const OLDER_CONTROL: &str =
    "an OPN pump's control service is already older than 1.3: the timeout sweep covers it";
const HALF_HOUR: Duration = Duration::from_secs(1800);
const HOUR: Duration = Duration::from_secs(3600);
const QUARTER: Duration = Duration::from_secs(900);
const RESERVOIR: &str = "reservoir 15000";

fn nominal<D: Board>() -> Vec<Scenario<D>> {
    vec![
        ("connect and status", connect_and_status),
        ("clock read", clock_read),
        ("clock sync", clock_sync),
        ("basal profile read and write", basal_profile),
        ("unsupported basal profile refused", unsupported_profile),
        ("temp basal set, replace and cancel", temp_basal),
        ("temp basal expiry", temp_basal_expiry),
        ("bolus delivered with progress", bolus),
        ("bolus with progress and stop", bolus_and_stop),
        ("timed-out bolus unknown and never resent", timed_out_bolus),
        ("extended bolus", extended_bolus),
        ("refusals on a stopped pump", stopped_refusals),
        ("commands the pump cannot do", unsupported_commands),
        ("battery from the console", battery),
        ("reservoir from the console", reservoir),
        ("battery alarm from the console", battery_alarm),
        ("reservoir alarm from the console", reservoir_alarm),
        ("occlusion alarm from the console", occlusion),
        ("stop and run from the console", stop_and_run),
        ("reboot from the console", reboot),
        ("pump reboot changes the epoch", epoch_change),
        ("learns the epoch before jumping", learns_epoch),
        ("counter moved by another app", other_app),
        ("gives up after the largest jump", largest_jump),
        ("restarts from stored slots", restart),
        ("restores old storage", old_storage),
        ("counters kept only for an encrypted pump", counters_kept),
        ("never drives an encrypted pump in clear", never_in_clear),
        ("wrong key rejected", wrong_key),
        ("unpairs", unpairs),
        ("ignores notifications", notifications),
    ]
}

pub fn all<D: Board>() -> Vec<Scenario<D>> {
    let mut scenarios = nominal();
    scenarios.extend([
        (
            "connect survives a timeout at every step",
            every_step_timeout as fn(&mut Session<D>),
        ),
        (
            "connect survives a disconnect at every step",
            every_step_disconnect,
        ),
        (
            "connect survives a host failure at every step",
            every_step_host,
        ),
        (
            "connect survives a counter error at every step",
            every_step_counter,
        ),
        ("every command survives timeouts", sweep_timeout),
        ("every command survives disconnects", sweep_disconnect),
        ("every command survives counter errors", sweep_counter),
        ("every command survives refusals", sweep_refusal),
        ("every command survives host failures", sweep_host),
        (
            "every command survives timeouts on an older control service",
            older_control,
        ),
    ]);
    scenarios
}

pub fn smoke<D: Board>() -> Vec<Scenario<D>> {
    let mut scenarios = nominal();
    scenarios.extend([
        (
            "one timeout recovered",
            one_fault_timeout as fn(&mut Session<D>),
        ),
        ("one disconnect recovered", one_fault_disconnect),
        ("one host failure recovered", one_fault_host),
    ]);
    scenarios
}

fn every_step_timeout<D: Board>(session: &mut Session<D>) {
    every_step(session, Fault::BleTimeout);
}

fn every_step_disconnect<D: Board>(session: &mut Session<D>) {
    every_step(session, Fault::BleDisconnected);
}

fn every_step_host<D: Board>(session: &mut Session<D>) {
    every_step(session, Fault::HostFails);
}

fn every_step_counter<D: Board>(session: &mut Session<D>) {
    every_step(session, Fault::BleStatus(0x8B));
}

fn sweep_timeout<D: Board>(session: &mut Session<D>) {
    sweep(session, Fault::BleTimeout);
}

fn sweep_disconnect<D: Board>(session: &mut Session<D>) {
    sweep(session, Fault::BleDisconnected);
}

fn sweep_counter<D: Board>(session: &mut Session<D>) {
    sweep(session, Fault::BleStatus(0x8B));
}

fn sweep_refusal<D: Board>(session: &mut Session<D>) {
    sweep(session, Fault::BleStatus(0x87));
}

fn sweep_host<D: Board>(session: &mut Session<D>) {
    sweep(session, Fault::HostFails);
}

fn one_fault_timeout<D: Board>(session: &mut Session<D>) {
    one_fault(session, Fault::BleTimeout);
}

fn one_fault_disconnect<D: Board>(session: &mut Session<D>) {
    one_fault(session, Fault::BleDisconnected);
}

fn one_fault_host<D: Board>(session: &mut Session<D>) {
    one_fault(session, Fault::HostFails);
}

fn one_fault<D: Board>(session: &mut Session<D>, fault: Fault) {
    session.fresh();
    let before = session.harness.transcript.len();
    session.harness.faults.insert(before + 1, fault);
    let faulted = session.command(Command::Status);
    let recovered = session.command(Command::Status);
    session.check(
        matches!(recovered, Outcome::Completed(_)),
        format!("after {faulted:?}: {recovered:?}"),
    );
}

fn units(milli: u32) -> InsulinAmount {
    InsulinAmount::from_milliunits(milli)
}

fn temp(percent: u16, duration: Duration) -> Command {
    Command::TempBasal {
        rate: TempBasalRate::Percent(Percent::new(percent)),
        duration,
    }
}

fn running(percent: u16, duration: Duration) -> Basal {
    Basal::Temporary {
        rate: TempBasalRate::Percent(Percent::new(percent)),
        duration,
    }
}

fn extended() -> Command {
    Command::ExtendedBolus {
        amount: units(2_000),
        duration: HOUR,
    }
}

fn expect<D: Board>(session: &mut Session<D>, outcome: Outcome, wanted: Outcome) {
    session.check(outcome == wanted, format!("{outcome:?}, wanted {wanted:?}"));
}

fn done<D: Board>(session: &mut Session<D>, outcome: Outcome) {
    expect(session, outcome, Outcome::Completed(Report::Done));
}

fn completed<D: Board>(session: &mut Session<D>, outcome: Outcome) {
    session.check(
        matches!(outcome, Outcome::Completed(_)),
        format!("outcome {outcome:?}"),
    );
}

fn status<D: Board>(session: &mut Session<D>) -> PumpStatus {
    let outcome = session.command(Command::Status);
    let status = match &outcome {
        Outcome::Completed(Report::Status(status)) => Some(status.clone()),
        _ => None,
    };
    session.check(status.is_some(), format!("status {outcome:?}"));
    status.unwrap_or_default()
}

fn mode<D: Board>(session: &mut Session<D>, wanted: PumpMode) -> PumpStatus {
    let status = status(session);
    let mode = status.therapy.mode;
    session.check(mode == wanted, format!("mode {mode:?}, wanted {wanted:?}"));
    status
}

fn basal<D: Board>(session: &mut Session<D>, wanted: Basal) {
    let basal = mode(session, PumpMode::Running).therapy.basal;
    session.check(
        basal == wanted,
        format!("basal {basal:?}, wanted {wanted:?}"),
    );
}

fn clock<D: Board>(session: &mut Session<D>) -> u64 {
    let outcome = session.command(Command::ReadClock);
    let seconds = match &outcome {
        Outcome::Completed(Report::Clock(time)) => Some(time.local_seconds()),
        _ => None,
    };
    session.check(seconds.is_some(), format!("clock {outcome:?}"));
    seconds.unwrap_or_default()
}

fn wall<D: Board>(session: &mut Session<D>, seconds: u64) {
    session
        .engine
        .handle_wall_clock(UtcTime::from_unix_millis(seconds * 1000));
}

fn profile<D: Board>(session: &mut Session<D>) -> Option<BasalProfile> {
    let outcome = session.command(Command::ReadBasalProfile);
    let profile = match &outcome {
        Outcome::Completed(Report::Profile(profile)) => Some(profile.clone()),
        _ => None,
    };
    session.check(profile.is_some(), format!("profile {outcome:?}"));
    profile
}

fn history<D: Board>(session: &mut Session<D>) -> Vec<HistoryEntry> {
    let outcome = session.command(Command::ReadHistory {
        after: session.seen,
    });
    let entries = match outcome {
        Outcome::Completed(Report::History(entries)) => entries,
        other => {
            session.check(false, format!("history {other:?}"));
            Vec::new()
        }
    };
    if let Some(newest) = entries.iter().map(|entry| entry.sequence).max() {
        session.seen = newest;
    }
    entries
}

fn logged<D: Board>(session: &mut Session<D>, wanted: impl Fn(&HistoryEvent) -> bool) {
    let events: Vec<_> = history(session).into_iter().map(|e| e.event).collect();
    session.check(
        events.iter().any(wanted),
        format!("history without the event: {events:?}"),
    );
}

fn stored<D: Board>(session: &Session<D>) -> Option<(i64, u32)> {
    let counters = session.harness.store.get(&Slot::Counters)?;
    let (mut write, mut epoch) = ([0; 8], [0; 4]);
    write.copy_from_slice(&counters[..8]);
    epoch.copy_from_slice(&counters[16..20]);
    Some((i64::from_le_bytes(write), u32::from_le_bytes(epoch)))
}

fn encrypted<D: Board>(session: &mut Session<D>) -> bool {
    if session.mode == Mode::Opn {
        session.skip(NO_COUNTERS);
    }
    session.mode == Mode::Encrypted
}

fn ready<D: Board>(session: &mut Session<D>) {
    for _ in 0..12 {
        if !matches!(session.command(Command::Status), Outcome::Failed(_)) {
            break;
        }
        session.wait(Duration::from_secs(5));
    }
}

fn halted_by<D: Board>(session: &mut Session<D>, line: &str) {
    history(session);
    session.console(line);
    mode(session, PumpMode::Suspended);
    logged(session, |event| *event == HistoryEvent::Suspended);
}

fn resumed<D: Board>(session: &mut Session<D>) {
    session.console("run");
    mode(session, PumpMode::Running);
    logged(session, |event| *event == HistoryEvent::Resumed);
}

fn connect_and_status<D: Board>(session: &mut Session<D>) {
    let status = mode(session, PumpMode::Running);
    session.check(
        status.battery.is_some() && status.reservoir.is_some(),
        format!("status {status:?}"),
    );
    let serial = session.events().iter().find_map(|event| match event {
        Event::DeviceInfo(info) => Some(info.serial.clone()),
        _ => None,
    });
    session.check(
        serial == Some(SERIAL.to_string()),
        format!("serial {serial:?}"),
    );
    let state = session.engine.state();
    session.check(state == EngineState::Paired, format!("state {state:?}"));
}

fn clock_read<D: Board>(session: &mut Session<D>) {
    clock(session);
}

fn clock_sync<D: Board>(session: &mut Session<D>) {
    session.restart();
    let unset = session.command(Command::SyncClock);
    expect(session, unset, Outcome::Failed(Failure::Host));
    let before = clock(session);
    wall(session, before + 3600);
    let synced = session.command(Command::SyncClock);
    done(session, synced);
    let after = clock(session);
    session.check(
        after.abs_diff(before + 3600) < 300,
        format!("clock {after} after syncing to {}", before + 3600),
    );
    wall(session, after.saturating_sub(3600));
    let back = session.command(Command::SyncClock);
    done(session, back);
}

fn basal_profile<D: Board>(session: &mut Session<D>) {
    history(session);
    let Some(original) = profile(session) else {
        return;
    };
    let rate = BasalRate::from_milliunits_per_hour;
    let segment = |hours: u64, milli: u32| BasalSegment {
        start: Duration::from_secs(hours * 3600),
        rate: rate(milli),
    };
    let wanted =
        BasalProfile::new(vec![segment(0, 800), segment(6, 1_220), segment(22, 0)]).unwrap();
    let set = session.command(Command::SetBasalProfile {
        profile: wanted.clone(),
    });
    done(session, set);
    let seven = Duration::from_secs(7 * 3600);
    let read = profile(session).map(|read| read.rate_at(seven));
    session.check(read == Some(rate(1_220)), format!("rate {read:?} at 7:00"));
    logged(session, |event| *event == HistoryEvent::BasalProfileChanged);
    let off_grid = BasalProfile::new(vec![segment(0, 1_010)]).unwrap();
    let half_hour = BasalProfile::new(vec![
        segment(0, 800),
        BasalSegment {
            start: HALF_HOUR,
            rate: rate(900),
        },
    ])
    .unwrap();
    for profile in [off_grid, half_hour] {
        let refused = session.command(Command::SetBasalProfile { profile });
        expect(session, refused, Outcome::Refused(Refusal::OutOfLimits));
    }
    let restored = session.command(Command::SetBasalProfile { profile: original });
    done(session, restored);
}

fn unsupported_profile<D: Board>(session: &mut Session<D>) {
    session.tamper(&mut |pump| {
        pump.settings.insert(setting::BASAL_PROFILE_SELECTION, 4);
    });
    let read = session.command(Command::ReadBasalProfile);
    expect(session, read, Outcome::Refused(Refusal::Unsupported));
    let profile = BasalProfile::new(vec![BasalSegment {
        start: Duration::ZERO,
        rate: BasalRate::from_milliunits_per_hour(800),
    }])
    .unwrap();
    let set = session.command(Command::SetBasalProfile { profile });
    expect(session, set, Outcome::Refused(Refusal::Unsupported));
    session.tamper(&mut |pump| {
        pump.settings.insert(setting::BASAL_PROFILE_SELECTION, 3);
    });
}

fn temp_basal<D: Board>(session: &mut Session<D>) {
    history(session);
    for (percent, wanted) in [
        (150, running(150, HALF_HOUR)),
        (80, running(80, HALF_HOUR)),
        (100, Basal::Scheduled),
        (80, running(80, HALF_HOUR)),
    ] {
        let set = session.command(temp(percent, HALF_HOUR));
        done(session, set);
        basal(session, wanted);
    }
    let cancel = session.command(Command::CancelTempBasal);
    done(session, cancel);
    basal(session, Basal::Scheduled);
    logged(session, |event| {
        matches!(event, HistoryEvent::TempBasal { rate, .. }
            if *rate == TempBasalRate::Percent(Percent::new(150)))
    });
    for refused in [
        temp(260, HALF_HOUR),
        temp(220, Duration::from_secs(150 * 60)),
        temp(150, Duration::from_secs(20 * 60)),
    ] {
        let outcome = session.command(refused);
        expect(session, outcome, Outcome::Refused(Refusal::OutOfLimits));
    }
    let absolute = session.command(Command::TempBasal {
        rate: TempBasalRate::Absolute(BasalRate::from_milliunits_per_hour(1_000)),
        duration: HALF_HOUR,
    });
    expect(session, absolute, Outcome::Refused(Refusal::Unsupported));
}

fn temp_basal_expiry<D: Board>(session: &mut Session<D>) {
    history(session);
    let set = session.command(temp(150, QUARTER));
    done(session, set);
    basal(session, running(150, QUARTER));
    session.wait(QUARTER + Duration::from_secs(30));
    basal(session, Basal::Scheduled);
    logged(session, |event| {
        matches!(
            event,
            HistoryEvent::TempBasal { rate, duration, phase: TempBasalPhase::Ended }
                if *rate == TempBasalRate::Percent(Percent::new(150)) && *duration == QUARTER
        )
    });
}

fn bolus<D: Board>(session: &mut Session<D>) {
    history(session);
    let delivered = session.command(Command::Bolus {
        amount: units(1_500),
    });
    expect(
        session,
        delivered,
        Outcome::Completed(Report::Delivered(units(1_500))),
    );
    let progress: Vec<_> = session
        .events()
        .iter()
        .filter_map(|event| match event {
            Event::Progress { delivered, .. } => Some(*delivered),
            _ => None,
        })
        .collect();
    session.check(
        progress.len() > 2 && progress.last() == Some(&units(1_500)),
        format!("progress {progress:?}"),
    );
    let idle = session.engine.therapy().bolus;
    session.check(idle == Bolus::Idle, format!("bolus {idle:?}"));
    logged(session, |event| {
        matches!(event, HistoryEvent::Bolus { delivered, in_progress: false, .. }
            if *delivered == units(1_500))
    });
    for amount in [units(1_550), units(30_000)] {
        let refused = session.command(Command::Bolus { amount });
        expect(session, refused, Outcome::Refused(Refusal::OutOfLimits));
    }
}

fn bolus_and_stop<D: Board>(session: &mut Session<D>) {
    history(session);
    let bolus = session.submit(Command::Bolus {
        amount: units(5_000),
    });
    let progressed = |session: &Session<D>| {
        session.events().iter().any(|event| {
            matches!(event, Event::Progress { id, delivered, .. }
                if Some(*id) == bolus && *delivered > InsulinAmount::ZERO)
        })
    };
    for _ in 0..30 {
        if progressed(session) {
            break;
        }
        session.wait(Duration::from_secs(1));
    }
    let stop = session.stop();
    let stopped = session.finish(Some(stop));
    done(session, stopped);
    let outcome = session.finish(bolus);
    expect(session, outcome, Outcome::Unknown);
    let progressed = progressed(session);
    session.check(progressed, "bolus without progress".into());
    logged(session, |event| {
        matches!(event, HistoryEvent::Bolus { delivered, in_progress: false, .. }
            if *delivered > units(0) && *delivered < units(5_000))
    });
}

fn timed_out_bolus<D: Board>(session: &mut Session<D>) {
    history(session);
    session
        .harness
        .write_faults
        .insert(uuids::START_STOP_BOLUS, Fault::BleTimeout);
    let outcome = session.command(Command::Bolus {
        amount: units(1_000),
    });
    expect(session, outcome, Outcome::Unknown);
    let bolus = mode(session, PumpMode::Running).therapy.bolus;
    session.check(bolus == Bolus::Idle, format!("bolus {bolus:?}"));
    let events = history(session);
    session.check(
        events.is_empty(),
        format!("logged after a timed-out bolus: {events:?}"),
    );
}

fn extended_bolus<D: Board>(session: &mut Session<D>) {
    history(session);
    let started = session.command(extended());
    if session.mode == Mode::Opn {
        expect(session, started, Outcome::Refused(Refusal::Unsupported));
        return;
    }
    done(session, started);
    let therapy = session.engine.therapy().extended;
    session.check(
        therapy
            == Extended::Running {
                amount: units(2_000),
                duration: HOUR,
            },
        format!("extended {therapy:?}"),
    );
    for _ in 0..2 {
        let cancel = session.command(Command::CancelExtendedBolus);
        done(session, cancel);
    }
    let idle = session.engine.therapy().extended;
    session.check(idle == Extended::Idle, format!("extended {idle:?}"));
    logged(session, |event| {
        *event
            == HistoryEvent::Bolus {
                kind: BolusKind::Extended,
                requested: units(0),
                delivered: units(0),
                duration: Duration::ZERO,
                automatic: false,
                in_progress: false,
            }
    });
    let refused = session.command(Command::ExtendedBolus {
        amount: units(50),
        duration: HOUR,
    });
    expect(session, refused, Outcome::Refused(Refusal::OutOfLimits));
}

fn stopped_refusals<D: Board>(session: &mut Session<D>) {
    halted_by(session, "stop");
    for command in [
        Command::Bolus {
            amount: units(1_000),
        },
        temp(150, HALF_HOUR),
        extended(),
    ] {
        let refused = session.command(command);
        expect(session, refused, Outcome::Refused(Refusal::NotRunning));
    }
    resumed(session);
}

fn unsupported_commands<D: Board>(session: &mut Session<D>) {
    let before = session.harness.transcript.len();
    for command in [
        Command::Suspend,
        Command::Resume,
        Command::SetDeliveryLimits {
            max_bolus: units(10_000),
            max_basal: BasalRate::from_milliunits_per_hour(2_000),
        },
        Command::Pair {
            address: pump_shared::Address::new([0; 6]),
            server_key: None,
        },
    ] {
        let refused = session.command(command);
        expect(session, refused, Outcome::Refused(Refusal::Unsupported));
    }
    let after = session.harness.transcript.len();
    session.check(after == before, format!("{} actions", after - before));
}

fn battery<D: Board>(session: &mut Session<D>) {
    for (line, wanted) in [("battery 1", 20), ("battery 4", 80)] {
        session.console(line);
        let battery = status(session).battery;
        session.check(
            battery == Some(Percent::new(wanted)),
            format!("battery {battery:?} after {line}"),
        );
    }
}

fn reservoir<D: Board>(session: &mut Session<D>) {
    for (line, wanted) in [("reservoir 5000", 50_000), (RESERVOIR, 150_000)] {
        session.console(line);
        let reservoir = status(session).reservoir;
        session.check(
            reservoir == Some(units(wanted)),
            format!("reservoir {reservoir:?} after {line}"),
        );
    }
}

fn alarm<D: Board>(session: &mut Session<D>, line: &str) {
    halted_by(session, line);
    let refused = session.command(Command::Bolus {
        amount: units(1_000),
    });
    expect(session, refused, Outcome::Refused(Refusal::NotRunning));
    session.console("acknowledge");
    resumed(session);
}

fn battery_alarm<D: Board>(session: &mut Session<D>) {
    alarm(session, "alarm battery");
}

fn reservoir_alarm<D: Board>(session: &mut Session<D>) {
    alarm(session, "alarm reservoir");
}

fn occlusion<D: Board>(session: &mut Session<D>) {
    alarm(session, "alarm occlusion");
}

fn stop_and_run<D: Board>(session: &mut Session<D>) {
    halted_by(session, "stop");
    resumed(session);
}

fn reboot<D: Board>(session: &mut Session<D>) {
    status(session);
    session.console("reboot");
    let lost = session.command(Command::Status);
    expect(session, lost, Outcome::Failed(Failure::Disconnected));
    ready(session);
    mode(session, PumpMode::Running);
}

fn epoch_change<D: Board>(session: &mut Session<D>) {
    if !encrypted(session) {
        return;
    }
    status(session);
    let before = stored(session);
    session.console("reboot");
    ready(session);
    let read = session.command(Command::ReadBasalProfile);
    completed(session, read);
    let after = stored(session);
    session.check(
        after.map(|(_, epoch)| epoch) == before.map(|(_, epoch)| epoch + 1),
        format!("counters {before:?} then {after:?}"),
    );
}

fn learns_epoch<D: Board>(session: &mut Session<D>) {
    if !encrypted(session) {
        return;
    }
    status(session);
    let before = stored(session);
    session.tamper(&mut |pump| pump.reboot += 1);
    let read = session.command(Command::ReadBasalProfile);
    completed(session, read);
    let after = stored(session);
    session.check(
        after.map(|(_, epoch)| epoch) == before.map(|(_, epoch)| epoch + 1),
        format!("counters {before:?} then {after:?}"),
    );
}

fn other_app<D: Board>(session: &mut Session<D>) {
    if !encrypted(session) {
        return;
    }
    status(session);
    let other = ypso::store(session.mode, ypso::KEY, ypso::CODE);
    let (ours, store) = session.switch(ypso::engine(), other);
    for _ in 0..3 {
        let read = session.command(Command::ReadBasalProfile);
        completed(session, read);
    }
    let theirs = stored(session);
    session.switch(ours, store);
    let read = session.command(Command::ReadBasalProfile);
    completed(session, read);
    let jumped = stored(session);
    session.check(
        jumped.map(|(write, _)| write) > theirs.map(|(write, _)| write),
        format!("counters {jumped:?} after the other app's {theirs:?}"),
    );
}

fn largest_jump<D: Board>(session: &mut Session<D>) {
    if !encrypted(session) {
        return;
    }
    status(session);
    let mut counter = 0;
    session.tamper(&mut |pump| {
        counter = pump.app_counter;
        pump.app_counter = i64::MAX as u64 - 1;
    });
    let read = session.command(Command::ReadBasalProfile);
    expect(session, read, Outcome::Failed(Failure::Status(0x8B)));
    session.tamper(&mut |pump| pump.app_counter = counter);
    session.fresh();
    status(session);
}

fn restart<D: Board>(session: &mut Session<D>) {
    status(session);
    let before = stored(session);
    session.restart();
    status(session);
    let state = session.engine.state();
    session.check(state == EngineState::Paired, format!("state {state:?}"));
    let after = stored(session);
    session.check(
        after >= before,
        format!("counters {before:?} then {after:?}"),
    );
}

fn old_storage<D: Board>(session: &mut Session<D>) {
    status(session);
    let old = session.harness.store.clone();
    let read = session.command(Command::ReadBasalProfile);
    completed(session, read);
    session.switch(ypso::engine(), old);
    let read = session.command(Command::ReadBasalProfile);
    completed(session, read);
}

fn counters_kept<D: Board>(session: &mut Session<D>) {
    session.fresh();
    status(session);
    let read = session.command(Command::ReadBasalProfile);
    completed(session, read);
    let kept = session.harness.store.contains_key(&Slot::Counters);
    session.check(
        kept == (session.mode == Mode::Encrypted),
        format!("counters stored: {kept}"),
    );
}

fn never_in_clear<D: Board>(session: &mut Session<D>) {
    if !encrypted(session) {
        return;
    }
    let before = session.harness.transcript.len();
    let (engine, store) = session.switch(
        ypso::engine(),
        ypso::store(Mode::Opn, ypso::KEY, ypso::CODE),
    );
    let refused = session.command(Command::Status);
    expect(session, refused, Outcome::Failed(Failure::Status(0x8D)));
    let allowed =
        [uuids::AUTHORIZATION_PASSWORD, uuids::SETTING_ID].map(|uuid| format!("{uuid:?}"));
    let driven = session.harness.transcript[before..]
        .iter()
        .any(|line| line.contains("Write") && !allowed.iter().any(|uuid| line.contains(uuid)));
    session.check(
        !driven,
        "wrote more than the connect's clear-text writes".into(),
    );
    session.switch(engine, store);
}

fn wrong_key<D: Board>(session: &mut Session<D>) {
    if !encrypted(session) {
        return;
    }
    let (engine, store) = session.switch(
        ypso::engine(),
        ypso::store(Mode::Encrypted, [1; KEY_LEN], b"X1"),
    );
    for _ in 0..2 {
        let rejected = session.command(Command::Status);
        expect(session, rejected, Outcome::Failed(Failure::KeyRejected));
    }
    let state = session.engine.state();
    session.check(
        state == EngineState::KeyRejected,
        format!("state {state:?}"),
    );
    session.switch(engine, store);
}

fn unpairs<D: Board>(session: &mut Session<D>) {
    status(session);
    let unpaired = session.command(Command::Unpair);
    done(session, unpaired);
    let state = session.engine.state();
    let empty = session.harness.store.is_empty();
    session.check(
        state == EngineState::Unpaired && empty,
        format!("state {state:?}, store empty {empty}"),
    );
    let refused = session.command(Command::Status);
    expect(session, refused, Outcome::Refused(Refusal::NotPaired));
    let stop = session.stop();
    let stopped = session.finish(Some(stop));
    expect(session, stopped, Outcome::Refused(Refusal::NotPaired));
    session.fresh();
    status(session);
}

fn notifications<D: Board>(session: &mut Session<D>) {
    session.engine.handle_notification(Notification {
        service: uuids::CONTROL_SERVICE,
        characteristic: uuids::CONTROL_NOTIFY,
        data: vec![1, 2, 3],
    });
    let idle = session.engine.poll_action().is_none();
    session.check(idle, "an action after a notification".into());
    status(session);
}

fn every_step<D: Board>(session: &mut Session<D>, fault: Fault) {
    for step in 1.. {
        session.fresh();
        let before = session.harness.transcript.len();
        session.harness.faults.insert(before + step, fault);
        let outcome = session.command(Command::Status);
        if session.harness.transcript.len() < before + step {
            completed(session, outcome);
            break;
        }
        let again = session.command(Command::Status);
        session.check(
            matches!(again, Outcome::Completed(_)),
            format!("step {step} ({outcome:?}): {again:?}"),
        );
    }
}

type Prepare<D> = fn(&mut Session<D>);
type Act<D> = fn(&mut Session<D>) -> Option<CommandId>;

fn nothing<D: Board>(_: &mut Session<D>) {}

fn read_status<D: Board>(session: &mut Session<D>) {
    status(session);
}

fn status_and_history<D: Board>(session: &mut Session<D>) {
    status(session);
    history(session);
}

fn start_extended<D: Board>(session: &mut Session<D>) {
    status(session);
    session.command(extended());
}

fn start_temp_basal<D: Board>(session: &mut Session<D>) {
    session.command(temp(120, HALF_HOUR));
}

fn set_wall<D: Board>(session: &mut Session<D>) {
    let now = clock(session);
    wall(session, now + 3600);
}

fn acts<D: Board>() -> Vec<(Prepare<D>, Act<D>)> {
    vec![
        (nothing, |s| s.submit(Command::Status)),
        (nothing, |s| s.submit(Command::ReadClock)),
        (set_wall, |s| s.submit(Command::SyncClock)),
        (status_and_history, |s| {
            let after = Sequence::new(s.seen.get().saturating_sub(1));
            s.submit(Command::ReadHistory { after })
        }),
        (nothing, |s| s.submit(Command::ReadBasalProfile)),
        (nothing, |s| {
            s.submit(Command::SetBasalProfile {
                profile: BasalProfile::new(vec![BasalSegment {
                    start: Duration::ZERO,
                    rate: BasalRate::from_milliunits_per_hour(900),
                }])
                .unwrap(),
            })
        }),
        (read_status, |s| s.submit(temp(150, HALF_HOUR))),
        (start_temp_basal, |s| s.submit(temp(150, HALF_HOUR))),
        (nothing, |s| s.submit(Command::CancelTempBasal)),
        (status_and_history, |s| {
            s.submit(Command::Bolus { amount: units(500) })
        }),
        (read_status, |s| s.submit(extended())),
        (start_extended, |s| s.submit(Command::CancelExtendedBolus)),
        (start_extended, |s| Some(s.stop())),
        (nothing, |s| Some(s.stop())),
        (nothing, |s| s.submit(Command::Unpair)),
    ]
}

fn sweep<D: Board>(session: &mut Session<D>, fault: Fault) {
    for (index, (prepare, act)) in acts::<D>().into_iter().enumerate() {
        for step in 1.. {
            session.fresh();
            for line in ["stop", "run", RESERVOIR] {
                session.console(line);
            }
            prepare(session);
            let before = session.harness.transcript.len();
            session.harness.faults.insert(before + step, fault);
            let id = act(session);
            let outcome = session.finish(id);
            if session.harness.transcript.len() < before + step {
                break;
            }
            let again = session.command(Command::Status);
            session.check(
                matches!(
                    again,
                    Outcome::Completed(_) | Outcome::Refused(Refusal::NotPaired)
                ),
                format!("act {index} step {step} ({outcome:?}): {again:?}"),
            );
        }
    }
    session.fresh();
}

fn older_control<D: Board>(session: &mut Session<D>) {
    if session.mode == Mode::Opn {
        session.skip(OLDER_CONTROL);
        return;
    }
    session.tamper(&mut |pump| pump.versions.control = "1.2".into());
    sweep(session, Fault::BleTimeout);
    session.tamper(&mut |pump| pump.versions.control = "1.4".into());
}
