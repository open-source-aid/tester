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
pub use scenarios::{all, smoke, Scenario};
pub use session::{Board, Clock, Error, Session};

pub fn run<D: Board>(
    mut session: Session<D>,
    scenarios: Vec<Scenario<D>>,
    out: &mut dyn Write,
) -> bool {
    let mut passed = true;
    for (name, scenario) in scenarios {
        scenario(&mut session);
        let result = session.result();
        passed &= matches!(result, Ok(()) | Err(Error::Skipped(_)));
        let _ = match result {
            Ok(()) => writeln!(out, "pass  {name}"),
            Err(skipped @ Error::Skipped(_)) => writeln!(out, "skip  {name}: {skipped}"),
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
            Session::new(ypso::board(mode), mode, Clock::Virtual),
            all(),
            out,
        )),
        #[cfg(feature = "radio")]
        Ok(Run {
            target: Target::Ble { name, console },
            mode,
        }) => hardware::session(name, console.as_deref(), mode)
            .map(|session| run(session, smoke(), out)),
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
        Session::new(board, mode, Clock::Virtual)
    }

    fn report(board: YpsoSim, mode: Mode, scenarios: Vec<Scenario<YpsoSim>>) -> (bool, String) {
        let mut out = Vec::new();
        let passed = run(session(board, mode), scenarios, &mut out);
        (passed, String::from_utf8(out).unwrap())
    }

    #[test]
    fn every_scenario_passes_against_the_board_double_in_both_modes() {
        for mode in [Mode::Opn, Mode::Encrypted] {
            let (passed, out) = report(ypso::board(mode), mode, all());
            assert!(passed, "{mode:?}\n{out}");
            assert_eq!(out.lines().count(), all::<YpsoSim>().len());
        }
    }

    #[test]
    fn the_smoke_subset_passes_against_the_board_double_in_both_modes() {
        for mode in [Mode::Opn, Mode::Encrypted] {
            let (passed, out) = report(ypso::board(mode), mode, smoke());
            assert!(passed, "{mode:?}\n{out}");
            assert_eq!(out.lines().count(), smoke::<YpsoSim>().len());
            assert!(out.contains("one timeout recovered"));
        }
    }

    #[test]
    fn the_smoke_subset_reports_a_fault_that_never_recovers() {
        let (passed, out) = report(YpsoSim::new([0; 32], [0x5A; 16]), Mode::Opn, smoke());
        assert!(!passed);
        assert!(out.contains("FAIL  one timeout recovered"));
    }

    #[test]
    fn reports_each_failing_scenario() {
        let (passed, out) = report(YpsoSim::new([0; 32], [0x5A; 16]), Mode::Opn, all());
        assert!(!passed);
        let count = |kind: &str| out.lines().filter(|l| l.starts_with(kind)).count();
        assert_eq!((count("FAIL"), count("skip"), count("pass")), (33, 7, 1));
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
