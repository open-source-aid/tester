use core::fmt;
use core::str::FromStr;

const MARKER: &str = "3735928559";

/// The exact local name the scanner looks for.
///
/// The radio knows nothing about devices: it connects to the one peripheral
/// advertising this name. Every way of obtaining one is a checked conversion
/// ([`SimulatorName`], [`SensorName`]), so a name can only reach the scanner
/// through a rule someone wrote down.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Advertised(String);

impl Advertised {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is the name a peripheral advertised.
    pub fn is(&self, advertised: Option<&str>) -> bool {
        advertised == Some(self.as_str())
    }
}

impl From<SimulatorName> for Advertised {
    fn from(name: SimulatorName) -> Self {
        Self(name.0)
    }
}

impl From<SensorName> for Advertised {
    fn from(name: SensorName) -> Self {
        Self(name.0)
    }
}

/// The advertised-name prefixes of the Dexcom sensors a board can impersonate,
/// with the total length of a name that carries each.
///
/// A G6 advertises `Dexcom` plus the last two characters of its transmitter
/// ID; every G7-generation model advertises a four-character model prefix plus
/// the last two characters of its four-digit pairing code. Taken from
/// `cgm-dexcom`'s variant table, which is where a name is matched for real.
const SENSOR_PREFIXES: [(&str, usize); 5] = [
    ("Dexcom", 8),
    ("DXCM", 6),
    ("DX02", 6),
    ("DX04", 6),
    ("DX01", 6),
];

/// The local name of a Dexcom sensor, real or simulated.
///
/// Unlike [`SimulatorName`], this carries **no simulator marker**: a Dexcom
/// name is a model prefix and two characters of a secret, and a simulator has
/// no room to say it is one. The shape is all that is checked, so the board
/// must be the only thing in range answering to the name given — a real
/// sensor whose name matches would be driven just the same.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SensorName(String);

impl SensorName {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The model prefix this name carries, as the driver spells it.
    pub fn prefix(&self) -> &'static str {
        SENSOR_PREFIXES
            .iter()
            .find(|(prefix, len)| matches(&self.0, prefix, *len))
            .map_or("", |(prefix, _)| *prefix)
    }
}

fn matches(name: &str, prefix: &str, len: usize) -> bool {
    name.len() == len
        && name
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

impl FromStr for SensorName {
    type Err = NameError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        let known = SENSOR_PREFIXES
            .iter()
            .any(|(prefix, len)| matches(name, prefix, *len));
        if known && name.is_ascii() {
            Ok(Self(name.to_owned()))
        } else {
            Err(NameError::sensor(name))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SimulatorName(String);

impl SimulatorName {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is(&self, advertised: Option<&str>) -> bool {
        advertised == Some(self.as_str())
    }
}

impl FromStr for SimulatorName {
    type Err = NameError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name.strip_suffix(MARKER) {
            Some(prefix)
                if !prefix.is_empty() && !prefix.ends_with(|c: char| c.is_ascii_digit()) =>
            {
                Ok(Self(name.to_owned()))
            }
            _ => Err(NameError::simulator(name)),
        }
    }
}

/// A name no rule here accepts, and what the rule was.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NameError {
    name: String,
    expected: &'static str,
}

impl NameError {
    fn simulator(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            expected: "only a simulator whose name ends with serial 3735928559 may be connected",
        }
    }

    fn sensor(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            expected: "a Dexcom sensor advertises `Dexcom` and two characters, or DXCM, DX02, DX04 or DX01 and two characters",
        }
    }
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "refusing `{}`: {}", self.name, self.expected)
    }
}

impl std::error::Error for NameError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_names_ending_with_the_simulator_serial() {
        let name: SimulatorName = "YpsoPump_3735928559".parse().unwrap();
        assert_eq!(name.as_str(), "YpsoPump_3735928559");
        for refused in [
            "",
            "3735928559",
            "YpsoPump_10155555",
            "YpsoPump_",
            "YpsoPump",
            "YpsoPump_13735928559",
            "YpsoPump_3735928559 ",
            "YpsoPump_37359285590",
        ] {
            assert_eq!(
                refused.parse::<SimulatorName>(),
                Err(NameError::simulator(refused))
            );
        }
        assert_eq!(
            "YpsoPump_10155555".parse::<SimulatorName>().unwrap_err().to_string(),
            "refusing `YpsoPump_10155555`: only a simulator whose name ends with serial 3735928559 may be connected"
        );
    }

    #[test]
    fn matches_an_advertised_name_exactly() {
        let name: SimulatorName = "YpsoPump_3735928559".parse().unwrap();
        assert!(name.is(Some("YpsoPump_3735928559")));
        for other in [
            None,
            Some("YpsoPump_10155555"),
            Some("YpsoPump_"),
            Some("YpsoPump_37359285"),
            Some("YpsoPump_3735928559X"),
            Some("ypsopump_3735928559"),
        ] {
            assert!(!name.is(other));
        }
    }

    #[test]
    fn accepts_a_name_shaped_like_a_dexcom_sensor() {
        for (name, prefix) in [
            ("DexcomEF", "Dexcom"),
            ("dexcom2Z", "Dexcom"),
            ("DXCM34", "DXCM"),
            ("DX0234", "DX02"),
            ("DX04lW", "DX04"),
            ("DX0176", "DX01"),
        ] {
            let parsed: SensorName = name.parse().expect(name);
            assert_eq!(parsed.as_str(), name);
            assert_eq!(parsed.prefix(), prefix, "{name}");
        }
    }

    #[test]
    fn refuses_a_name_no_dexcom_advertises() {
        for refused in [
            "",
            "Dexcom",
            "Dexcom123",
            "DXCM",
            "DXCM345",
            "DX0534",
            "YpsoPump_3735928559",
            "DXCMé",
        ] {
            assert_eq!(
                refused.parse::<SensorName>(),
                Err(NameError::sensor(refused)),
                "{refused}"
            );
        }
        assert_eq!(
            "Libre3".parse::<SensorName>().unwrap_err().to_string(),
            "refusing `Libre3`: a Dexcom sensor advertises `Dexcom` and two characters, or DXCM, DX02, DX04 or DX01 and two characters"
        );
    }

    #[test]
    fn the_scanner_matches_either_kind_of_name_exactly() {
        let pump: Advertised = "YpsoPump_3735928559"
            .parse::<SimulatorName>()
            .unwrap()
            .into();
        let sensor: Advertised = "DXCM34".parse::<SensorName>().unwrap().into();
        assert_eq!(pump.as_str(), "YpsoPump_3735928559");
        assert!(pump.is(Some("YpsoPump_3735928559")));
        assert!(sensor.is(Some("DXCM34")));
        for other in [None, Some("dxcm34"), Some("DXCM3"), Some("DXCM344")] {
            assert!(!sensor.is(other));
        }
    }
}
