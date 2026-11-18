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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageError {
    Usage,
    Name(NameError),
}

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage => f.write_str(
                "usage: tester run --target sim | tester run --target ble --name <exact simulator name> [--console <serial port>]",
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

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Target, UsageError> {
    let mut args = args.into_iter();
    if args.next().as_deref() != Some("run") {
        return Err(UsageError::Usage);
    }
    let (mut target, mut name, mut console) = (None, None, None);
    while let (Some(flag), Some(value)) = (args.next(), args.next()) {
        let slot = match flag.as_str() {
            "--target" => &mut target,
            "--name" => &mut name,
            "--console" => &mut console,
            _ => return Err(UsageError::Usage),
        };
        if slot.replace(value).is_some() {
            return Err(UsageError::Usage);
        }
    }
    match (target.as_deref(), name, console) {
        (Some("sim"), None, None) => Ok(Target::Sim),
        (Some("ble"), Some(name), console) => Ok(Target::Ble {
            name: name.parse()?,
            console: console.map(PathBuf::from),
        }),
        _ => Err(UsageError::Usage),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_line(line: &str) -> Result<Target, UsageError> {
        parse(line.split_whitespace().map(String::from))
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
        ] {
            assert_eq!(parse_line(line), Err(UsageError::Usage), "{line}");
        }
        assert!(UsageError::Usage
            .to_string()
            .starts_with("usage: tester run"));
    }
}
