use core::fmt;
use std::path::PathBuf;

use cgm_shared::{Address, Secret};
use tester_ble::{NameError, SensorName, SimulatorName};

use crate::cgm::scenarios;
use crate::cgm::{self, Model};

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

/// A run against a CGM sensor on a board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CgmRun {
    pub model: Model,
    /// The name the board advertises, given or worked out from the secret.
    pub name: SensorName,
    pub console: Option<PathBuf>,
    /// The key kept from an earlier pairing.
    pub credential: Option<Secret>,
    /// The code printed on the device, for a pairing that has to agree one.
    pub pairing_secret: Option<Secret>,
    pub expected_serial: Option<String>,
    pub address: Address,
    pub scenario: String,
    pub minutes: u64,
    /// Where the slots the engine stores are kept between runs.
    pub store: PathBuf,
}

/// What the command line asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    Pump(Run),
    Cgm(Box<CgmRun>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageError {
    Usage,
    Name(NameError),
    /// Something the line said that cannot be used, and what to type instead.
    Said(String),
}

const USAGE: &str = "usage: tester run --target sim [--mode opn|encrypted] | tester run --target ble --name <exact simulator name> [--console <serial port>] [--mode opn|encrypted] | tester cgm --model g6|g7|one-plus|flex [--name <advertised name>] [--credential <key>] [--pairing-secret <code>] [--serial <printed serial>] [--address aa:bb:cc:dd:ee:ff] [--console <serial port>] [--scenario <name>] [--minutes <n>] [--store <directory>]";

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage => f.write_str(USAGE),
            Self::Name(error) => error.fmt(f),
            Self::Said(what) => f.write_str(what),
        }
    }
}

impl std::error::Error for UsageError {}

impl From<NameError> for UsageError {
    fn from(error: NameError) -> Self {
        Self::Name(error)
    }
}

impl From<String> for UsageError {
    fn from(what: String) -> Self {
        Self::Said(what)
    }
}

/// The flags of one invocation, each given at most once.
#[derive(Default)]
struct Flags(Vec<(String, String)>);

impl Flags {
    /// Reads `--flag value` pairs, refusing an unknown flag, a flag with no
    /// value and a flag given twice.
    fn read(args: &mut impl Iterator<Item = String>, known: &[&str]) -> Result<Self, UsageError> {
        let mut flags = Self::default();
        while let Some(flag) = args.next() {
            let value = args.next().ok_or(UsageError::Usage)?;
            let name = flag.strip_prefix("--").ok_or(UsageError::Usage)?;
            if !known.contains(&name) || flags.get(name).is_some() {
                return Err(UsageError::Usage);
            }
            flags.0.push((name.to_owned(), value));
        }
        Ok(flags)
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(flag, _)| flag == name)
            .map(|(_, value)| value.as_str())
    }
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Invocation, UsageError> {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        Some("run") => pump(&mut args).map(Invocation::Pump),
        Some("cgm") => cgm(&mut args).map(|run| Invocation::Cgm(Box::new(run))),
        _ => Err(UsageError::Usage),
    }
}

fn pump(args: &mut impl Iterator<Item = String>) -> Result<Run, UsageError> {
    let flags = Flags::read(args, &["target", "name", "console", "mode"])?;
    let mode = match flags.get("mode") {
        None | Some("opn") => Mode::Opn,
        Some("encrypted") => Mode::Encrypted,
        _ => return Err(UsageError::Usage),
    };
    let target = match (flags.get("target"), flags.get("name"), flags.get("console")) {
        (Some("sim"), None, None) => Target::Sim,
        (Some("ble"), Some(name), console) => Target::Ble {
            name: name.parse()?,
            console: console.map(PathBuf::from),
        },
        _ => return Err(UsageError::Usage),
    };
    Ok(Run { target, mode })
}

fn cgm(args: &mut impl Iterator<Item = String>) -> Result<CgmRun, UsageError> {
    let flags = Flags::read(
        args,
        &[
            "model",
            "name",
            "console",
            "credential",
            "pairing-secret",
            "serial",
            "address",
            "scenario",
            "minutes",
            "store",
        ],
    )?;
    let model = flags
        .get("model")
        .and_then(Model::parse)
        .ok_or_else(|| said("--model is one of g6, g7, one-plus or flex"))?;
    let credential = flags
        .get("credential")
        .map(|text| cgm::credential(model, text))
        .transpose()?;
    let pairing_secret = flags
        .get("pairing-secret")
        .map(|text| cgm::pairing_secret(model, text))
        .transpose()?;
    let name = name(model, &flags)?;
    // The driver owns the name table: a name it does not match is a name the
    // scanner must not be handed either.
    cgm::variant(model, name.as_str())?;
    let address = flags
        .get("address")
        .map_or_else(|| Ok(Address::new(cgm::ADDRESS)), cgm::address)?;
    let scenario = flags.get("scenario").unwrap_or("all").to_owned();
    if scenarios::selected::<scenarios::Never>(&scenario).is_none() {
        return Err(said(&format!(
            "`{scenario}` is no scenario: --scenario is one of {}",
            scenarios::names().join(", ")
        )));
    }
    let minutes = match flags.get("minutes") {
        None => 11,
        Some(text) => text
            .parse()
            .map_err(|_| said("--minutes is a whole number of minutes"))?,
    };
    let store = match flags.get("store") {
        Some(dir) => PathBuf::from(dir),
        None => default_store(name.as_str())?,
    };
    Ok(CgmRun {
        model,
        name,
        console: flags.get("console").map(PathBuf::from),
        credential,
        pairing_secret,
        expected_serial: flags.get("serial").map(str::to_owned),
        address,
        scenario,
        minutes,
        store,
    })
}

/// The name to scan for: the one given, or the one the printed secret implies.
fn name(model: Model, flags: &Flags) -> Result<SensorName, UsageError> {
    if let Some(given) = flags.get("name") {
        return Ok(given.parse()?);
    }
    let printed = flags
        .get("pairing-secret")
        .or_else(|| flags.get("credential").filter(|_| model.is_g6()))
        .ok_or_else(|| {
            said(&format!(
                "--name is the name the board advertises; a {model} that was handed its printed secret can be found without it, but this run was not"
            ))
        })?;
    let derived = model.advertised_name(printed).ok_or_else(|| {
        said(&format!(
            "`{printed}` is too short to end a {model}'s advertised name: pass --name"
        ))
    })?;
    Ok(derived.parse()?)
}

/// Where a sensor's slots live when `--store` says nothing.
///
/// One directory per advertised name, under the user's state directory, so
/// two sensors cannot overwrite each other's session and a pairing survives
/// the run that made it.
fn default_store(name: &str) -> Result<PathBuf, UsageError> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })
        .ok_or_else(|| {
            said("neither HOME nor XDG_STATE_HOME is set, so there is nowhere to keep the sensor's session: pass --store <directory>")
        })?;
    Ok(state.join("tester-cgm").join(name))
}

fn said(what: &str) -> UsageError {
    UsageError::Said(what.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_line(line: &str) -> Result<Target, UsageError> {
        pump_run(line).map(|run| run.target)
    }

    fn pump_run(line: &str) -> Result<Run, UsageError> {
        match parse(line.split_whitespace().map(String::from))? {
            Invocation::Pump(run) => Ok(run),
            Invocation::Cgm(_) => panic!("a pump line"),
        }
    }

    fn cgm_run(line: &str) -> Result<CgmRun, UsageError> {
        match parse(line.split_whitespace().map(String::from))? {
            Invocation::Cgm(run) => Ok(*run),
            Invocation::Pump(_) => panic!("a cgm line"),
        }
    }

    fn mode(line: &str) -> Mode {
        pump_run(line).unwrap().mode
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
            "run --target sim -mode opn",
        ] {
            assert_eq!(parse_line(line), Err(UsageError::Usage), "{line}");
        }
        assert!(UsageError::Usage
            .to_string()
            .starts_with("usage: tester run"));
    }

    #[test]
    fn a_cgm_run_knows_the_sensor_from_the_printed_secret_alone() {
        let run = cgm_run("cgm --model g7 --pairing-secret 1234 --store /tmp/x").unwrap();
        assert_eq!(run.model, Model::G7);
        assert_eq!(run.name.as_str(), "DXCM34");
        assert_eq!(
            run.pairing_secret.map(|s| s.expose().to_vec()),
            Some(b"1234".to_vec())
        );
        assert_eq!(run.credential, None);
        assert_eq!(run.scenario, "all");
        assert_eq!(run.minutes, 11);
        assert_eq!(run.store, PathBuf::from("/tmp/x"));
        assert_eq!(run.address, Address::new(cgm::ADDRESS));

        let run = cgm_run("cgm --model g6 --credential 8GH42Z --store /tmp/x").unwrap();
        assert_eq!(run.name.as_str(), "Dexcom2Z");
        assert_eq!(
            run.credential.map(|s| s.expose().to_vec()),
            Some(b"8GH42Z".to_vec())
        );
    }

    #[test]
    fn a_cgm_run_takes_every_flag() {
        let run = cgm_run(
            "cgm --model one-plus --name DX0299 --console /dev/cu.usbmodem1 --credential 000102030405060708090a0b0c0d0e0f --pairing-secret 9999 --serial 123456789 --address de:ad:be:ef:01:02 --scenario status --minutes 30 --store /tmp/one-plus",
        )
        .unwrap();
        assert_eq!(run.model, Model::OnePlus);
        assert_eq!(run.name.as_str(), "DX0299");
        assert_eq!(run.console, Some(PathBuf::from("/dev/cu.usbmodem1")));
        assert_eq!(run.credential.map(|s| s.expose().len()), Some(16));
        assert_eq!(run.expected_serial.as_deref(), Some("123456789"));
        assert_eq!(run.scenario, "status");
        assert_eq!(run.minutes, 30);
        assert_eq!(run.store, PathBuf::from("/tmp/one-plus"));
    }

    #[test]
    fn a_cgm_run_says_what_it_cannot_use() {
        let wrong = [
            ("cgm", "--model is one of"),
            ("cgm --model g5 --name DXCM34", "--model is one of"),
            (
                "cgm --model g7 --store /tmp/x",
                "--name is the name the board advertises",
            ),
            (
                "cgm --model g7 --name Dexcom12 --store /tmp/x",
                "is a g6 name, not a g7",
            ),
            (
                "cgm --model g7 --name Libre3 --store /tmp/x",
                "refusing `Libre3`",
            ),
            (
                "cgm --model g7 --pairing-secret 12a4 --store /tmp/x",
                "--pairing-secret for a g7 is the 4 digits",
            ),
            (
                "cgm --model g7 --credential zzzz --store /tmp/x",
                "--credential for a g7 is the 16-byte key",
            ),
            (
                "cgm --model g7 --pairing-secret 1234 --scenario pairing --store /tmp/x",
                "`pairing` is no scenario",
            ),
            (
                "cgm --model g7 --pairing-secret 1234 --minutes soon --store /tmp/x",
                "--minutes is a whole number",
            ),
            (
                "cgm --model g7 --pairing-secret 1234 --address nope --store /tmp/x",
                "is not an address",
            ),
        ];
        for (line, expected) in wrong {
            let error = cgm_run(line).unwrap_err().to_string();
            assert!(error.contains(expected), "{line}: {error}");
        }
        for line in [
            "cgm --model g7 --model g7",
            "cgm --model g7 --pairing-secret",
            "cgm --model g7 --colour red",
        ] {
            assert_eq!(cgm_run(line), Err(UsageError::Usage), "{line}");
        }
    }

    #[test]
    fn a_cgm_run_keeps_its_slots_under_the_state_directory() {
        let run = cgm_run("cgm --model g7 --pairing-secret 1234").unwrap();
        assert!(
            run.store.ends_with("tester-cgm/DXCM34"),
            "{}",
            run.store.display()
        );
        assert!(run.store.is_absolute(), "{}", run.store.display());
    }

    #[test]
    fn the_usage_line_names_both_devices() {
        let usage = UsageError::Usage.to_string();
        assert!(usage.contains("tester run --target sim"));
        assert!(usage.contains("tester cgm --model"));
    }
}
