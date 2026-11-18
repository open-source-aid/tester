use core::time::Duration;

use pump_shared::{
    Basal, Command, Event, Failure, HistoryEvent, InsulinAmount, Outcome, Percent, PumpMode,
    PumpStatus, Report, Sequence, TempBasalRate,
};

use crate::session::{Board, Session};
use crate::ypso::SERIAL;

pub type Scenario<D> = (&'static str, fn(&mut Session<D>));

pub fn all<D: Board>() -> [Scenario<D>; 8] {
    [
        ("connect and status", connect_and_status),
        ("clock read", clock_read),
        ("temp basal set and cancel", temp_basal),
        ("bolus with progress and stop", bolus_and_stop),
        ("history read", history_read),
        ("occlusion alarm from the console", occlusion),
        ("stop and run from the console", stop_and_run),
        ("reboot from the console", reboot),
    ]
}

fn done<D: Board>(session: &mut Session<D>, outcome: Outcome) {
    session.check(
        outcome == Outcome::Completed(Report::Done),
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

fn halted_by<D: Board>(session: &mut Session<D>, line: &str) {
    session.console(line);
    let mode = status(session).therapy.mode;
    session.check(
        mode != PumpMode::Running,
        format!("mode {mode:?} after {line}"),
    );
}

fn connect_and_status<D: Board>(session: &mut Session<D>) {
    mode(session, PumpMode::Running);
    let serial = session.events().iter().find_map(|event| match event {
        Event::DeviceInfo(info) => Some(info.serial.clone()),
        _ => None,
    });
    session.check(
        serial == Some(SERIAL.to_string()),
        format!("serial {serial:?}"),
    );
}

fn clock_read<D: Board>(session: &mut Session<D>) {
    let clock = session.command(Command::ReadClock);
    session.check(
        matches!(clock, Outcome::Completed(Report::Clock(_))),
        format!("clock {clock:?}"),
    );
}

fn temp_basal<D: Board>(session: &mut Session<D>) {
    let rate = TempBasalRate::Percent(Percent::new(150));
    let duration = Duration::from_secs(1800);
    let set = session.command(Command::TempBasal { rate, duration });
    done(session, set);
    let basal = mode(session, PumpMode::Running).therapy.basal;
    session.check(
        basal == Basal::Temporary { rate, duration },
        format!("basal {basal:?} after set"),
    );
    let cancel = session.command(Command::CancelTempBasal);
    done(session, cancel);
    let basal = mode(session, PumpMode::Running).therapy.basal;
    session.check(
        basal == Basal::Scheduled,
        format!("basal {basal:?} after cancel"),
    );
}

fn bolus_and_stop<D: Board>(session: &mut Session<D>) {
    let bolus = session.submit(Command::Bolus {
        amount: InsulinAmount::from_milliunits(5_000),
    });
    session.wait(Duration::from_secs(6));
    let stop = session.stop();
    let stopped = session.finish(Some(stop));
    done(session, stopped);
    let outcome = session.finish(bolus);
    session.check(outcome == Outcome::Unknown, format!("bolus {outcome:?}"));
    let progressed = session.events().iter().any(|event| {
        matches!(event, Event::Progress { id, delivered, .. }
            if Some(*id) == bolus && *delivered > InsulinAmount::ZERO)
    });
    session.check(progressed, "bolus without progress".into());
}

fn history_read<D: Board>(session: &mut Session<D>) {
    let history = session.command(Command::ReadHistory {
        after: Sequence::new(0),
    });
    let bolused = matches!(&history, Outcome::Completed(Report::History(entries))
        if entries.iter().any(|entry| matches!(entry.event, HistoryEvent::Bolus { .. })));
    session.check(bolused, format!("history {history:?}"));
}

fn occlusion<D: Board>(session: &mut Session<D>) {
    halted_by(session, "alarm occlusion");
    session.console("acknowledge");
    session.console("run");
    mode(session, PumpMode::Running);
}

fn stop_and_run<D: Board>(session: &mut Session<D>) {
    halted_by(session, "stop");
    session.console("run");
    mode(session, PumpMode::Running);
}

fn reboot<D: Board>(session: &mut Session<D>) {
    session.console("reboot");
    let lost = session.command(Command::Status);
    session.check(
        lost == Outcome::Failed(Failure::Disconnected),
        format!("first status after reboot {lost:?}"),
    );
    mode(session, PumpMode::Running);
}
