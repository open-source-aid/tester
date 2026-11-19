use core::fmt;
use std::path::PathBuf;

use tester_ble::{NameError, SimulatorName};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Sim,
    Ble {
        name: SimulatorName,
        console: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Opn,
    Encrypted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub target: Target,
    pub mode: Mode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageError {
    Usage,
    Name(NameError),
}

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage => f.write_str(
                "usage: tester run --target sim [--mode opn|encrypted] | tester run --target ble --name <exact simulator name> [--console <serial port>] [--mode opn|encrypted]",
            ),
            Self::Name(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for UsageError {}

impl From<NameError> for UsageError {
    fn from(error: NameError) -> Self {
        Self::Name(error)
    }
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Run, UsageError> {
    let mut args = args.into_iter();
    if args.next().as_deref() != Some("run") {
        return Err(UsageError::Usage);
    }
    let (mut target, mut name, mut console, mut mode) = (None, None, None, None);
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(UsageError::Usage)?;
        let slot = match flag.as_str() {
            "--target" => &mut target,
            "--name" => &mut name,
            "--console" => &mut console,
            "--mode" => &mut mode,
            _ => return Err(UsageError::Usage),
        };
        if slot.replace(value).is_some() {
            return Err(UsageError::Usage);
        }
    }
    let mode = match mode.as_deref() {
        None | Some("opn") => Mode::Opn,
        Some("encrypted") => Mode::Encrypted,
        _ => return Err(UsageError::Usage),
    };
    let target = match (target.as_deref(), name, console) {
        (Some("sim"), None, None) => Target::Sim,
        (Some("ble"), Some(name), console) => Target::Ble {
            name: name.parse()?,
            console: console.map(PathBuf::from),
        },
        _ => return Err(UsageError::Usage),
    };
    Ok(Run { target, mode })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_line(line: &str) -> Result<Target, UsageError> {
        parse(line.split_whitespace().map(String::from)).map(|run| run.target)
    }

    fn mode(line: &str) -> Mode {
        parse(line.split_whitespace().map(String::from))
            .unwrap()
            .mode
    }

    #[test]
    fn parses_both_modes_and_defaults_to_opn() {
        assert_eq!(mode("run --target sim"), Mode::Opn);
        assert_eq!(mode("run --target sim --mode opn"), Mode::Opn);
        assert_eq!(mode("run --mode encrypted --target sim"), Mode::Encrypted);
        assert_eq!(
            mode("run --target ble --name YpsoPump_3735928559 --mode encrypted"),
            Mode::Encrypted
        );
    }

    #[test]
    fn parses_both_targets() {
        assert_eq!(parse_line("run --target sim"), Ok(Target::Sim));
        assert_eq!(
            parse_line("run --target ble --name YpsoPump_3735928559 --console /dev/cu.usbmodem1"),
            Ok(Target::Ble {
                name: "YpsoPump_3735928559".parse().unwrap(),
                console: Some(PathBuf::from("/dev/cu.usbmodem1")),
            })
        );
        assert_eq!(
            parse_line("run --name YpsoPump_3735928559 --target ble"),
            Ok(Target::Ble {
                name: "YpsoPump_3735928559".parse().unwrap(),
                console: None,
            })
        );
    }

    #[test]
    fn refuses_any_name_but_a_simulator() {
        for name in ["YpsoPump_10155555", "YpsoPump_", "YpsoPump", "3735928559"] {
            assert_eq!(
                parse_line(&format!("run --target ble --name {name}")),
                Err(UsageError::Name(name.parse::<SimulatorName>().unwrap_err()))
            );
        }
        assert_eq!(
            parse_line("run --target ble"),
            Err(UsageError::Usage),
            "no default name"
        );
        assert!(parse_line("run --target ble --name YpsoPump_10155555")
            .unwrap_err()
            .to_string()
            .starts_with("refusing `YpsoPump_10155555`"));
    }

    #[test]
    fn refuses_malformed_lines() {
        for line in [
            "",
            "test --target sim",
            "run",
            "run --target",
            "run --target usb",
            "run --target sim --name YpsoPump_3735928559",
            "run --target sim --console /dev/null",
            "run --target sim --target sim",
            "run --target sim --verbose yes",
            "run --target sim --mode",
            "run --target sim --mode clear",
            "run --target sim --mode opn --mode opn",
        ] {
            assert_eq!(parse_line(line), Err(UsageError::Usage), "{line}");
        }
        assert!(UsageError::Usage
            .to_string()
            .starts_with("usage: tester run"));
    }
}
