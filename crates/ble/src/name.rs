use core::fmt;
use core::str::FromStr;

const MARKER: &str = "3735928559";

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
            _ => Err(NameError(name.to_owned())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NameError(String);

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "refusing `{}`: only a simulator whose name ends with serial {MARKER} may be connected",
            self.0
        )
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
                Err(NameError(refused.to_owned()))
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
}
