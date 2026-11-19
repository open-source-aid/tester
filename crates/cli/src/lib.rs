#![forbid(unsafe_code)]

mod args;
#[cfg(feature = "radio")]
mod hardware;
mod scenarios;
mod session;
mod ypso;

use std::io::Write;
use std::process::ExitCode;

pub use args::{parse, Mode, Run, Target, UsageError};
pub use scenarios::{all, Scenario};
pub use session::{Board, Clock, Error, Session};

pub fn run<D: Board>(mut session: Session<D>, out: &mut dyn Write) -> bool {
    let mut passed = true;
    for (name, scenario) in all() {
        scenario(&mut session);
        let result = session.result();
        passed &= result.is_ok();
        let _ = match result {
            Ok(()) => writeln!(out, "pass  {name}"),
            Err(error) => writeln!(out, "FAIL  {name}: {error}"),
        };
    }
    passed
}

pub fn main(
    args: impl IntoIterator<Item = String>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> ExitCode {
    let passed = match parse(args) {
        Ok(Run {
            target: Target::Sim,
            mode,
        }) => Ok(run(
            Session::new(
                ypso::harness(ypso::board(mode), mode),
                ypso::engine(),
                Clock::Virtual,
            ),
            out,
        )),
        #[cfg(feature = "radio")]
        Ok(Run {
            target: Target::Ble { name, console },
            mode,
        }) => hardware::session(name, console.as_deref(), mode).map(|session| run(session, out)),
        #[cfg(not(feature = "radio"))]
        Ok(Run {
            target: Target::Ble { .. },
            ..
        }) => Err("built without BLE: rebuild with --features radio".into()),
        Err(error) => Err(error.to_string()),
    };
    match passed {
        Ok(passed) => ExitCode::from(u8::from(!passed)),
        Err(error) => {
            let _ = writeln!(err, "{error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pump_ypso::sim::YpsoSim;

    fn session(board: YpsoSim, mode: Mode) -> Session<YpsoSim> {
        Session::new(ypso::harness(board, mode), ypso::engine(), Clock::Virtual)
    }

    fn report(board: YpsoSim, mode: Mode) -> (bool, String) {
        let mut out = Vec::new();
        let passed = run(session(board, mode), &mut out);
        (passed, String::from_utf8(out).unwrap())
    }

    #[test]
    fn every_scenario_passes_against_the_board_double_in_both_modes() {
        for mode in [Mode::Opn, Mode::Encrypted] {
            let (passed, out) = report(ypso::board(mode), mode);
            assert!(passed, "{mode:?}\n{out}");
            assert_eq!(out.lines().count(), all::<YpsoSim>().len());
        }
    }

    #[test]
    fn reports_each_failing_scenario() {
        let (passed, out) = report(YpsoSim::new([0; 32], [0x5A; 16]), Mode::Opn);
        assert!(!passed);
        assert_eq!(out.lines().filter(|l| l.starts_with("FAIL")).count(), 8);
        assert!(out.starts_with("FAIL  connect and status: unexpected status Failed("));
    }

    #[test]
    fn main_reports_usage_errors() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = main(["run".to_string()], &mut out, &mut err);
        assert_eq!(code, ExitCode::from(2));
        assert!(String::from_utf8(err).unwrap().starts_with("usage:"));
    }
}
