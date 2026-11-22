//! Driving the Dexcom CGM driver: which sensor, which secrets, which engine.
//!
//! The pendant of [`crate::ypso`] for the CGM side. Nothing here talks to a
//! radio; it turns what the user typed into the three things the engine needs
//! — a variant, an address and the secrets a `Pair` carries — and refuses
//! early, with a message that says what to type instead, whatever it cannot
//! turn into one of them.

pub mod scenarios;
pub mod session;
pub mod store;

use core::fmt;

use cgm_dexcom::DRIVER;
use cgm_shared::{Address, Advertisement, CgmDriver, CgmEngine, Config, Secret, UtcTime, Variant};

/// The address a `Pair` carries.
///
/// The radio scans by advertised name and ignores it: macOS does not hand a
/// central the peripheral's MAC at all, so no address this tool could print
/// would be the device's. The engine keeps it in the session and passes it
/// back on every later connect, which costs nothing as long as it is stable.
/// `--address` overrides it for a host (BlueZ) where it means something.
pub const ADDRESS: [u8; 6] = [0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02];

/// How many digits a G7-generation pairing code has.
const CODE_LEN: usize = 4;

/// How many characters a G6 transmitter ID has.
const ID_LEN: usize = 6;

/// How many bytes the G7 generation's AES key is.
const KEY_LEN: usize = 16;

/// The sensor the user is aiming at.
///
/// A model, not a generation: the G7 generation's models speak one protocol
/// and differ only in the name they advertise, which is exactly what has to be
/// known before scanning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Model {
    G6,
    G7,
    OnePlus,
    Flex,
}

impl Model {
    /// Every model this build can drive, in the order `--model` lists them.
    pub const ALL: [Model; 4] = [Model::G6, Model::G7, Model::OnePlus, Model::Flex];

    /// The word `--model` spells it with, the one the board console uses too.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::G6 => "g6",
            Self::G7 => "g7",
            Self::OnePlus => "one-plus",
            Self::Flex => "flex",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|model| model.as_str() == word)
    }

    /// The driver's variant id for this model.
    pub fn variant_id(self) -> &'static str {
        match self {
            Self::G6 => "g6",
            Self::G7 => "g7",
            Self::OnePlus => "g7-one-plus",
            Self::Flex => "g7-flex",
        }
    }

    /// The prefix this model advertises.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::G6 => "Dexcom",
            Self::G7 => "DXCM",
            Self::OnePlus => "DX02",
            Self::Flex => "DX04",
        }
    }

    /// Whether this model belongs to the G6 generation, which decides what
    /// every secret on the command line means.
    pub fn is_g6(self) -> bool {
        self == Self::G6
    }

    /// The name this model advertises when its printed secret is `secret`.
    ///
    /// A G6 advertises `Dexcom` plus the last two characters of its
    /// transmitter ID, a G7-generation sensor its prefix plus the last two
    /// digits of its pairing code. Which is why `--name` can be left out when
    /// a secret was given, and must be there when it was not.
    pub fn advertised_name(self, secret: &str) -> Option<String> {
        let tail = secret.get(secret.len().checked_sub(2)?..)?;
        tail.is_ascii().then(|| format!("{}{tail}", self.prefix()))
    }
}

impl fmt::Display for Model {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The credential kept between runs: the sensor's lifelong key.
///
/// The two generations keep different things under that one word, so the two
/// are read differently and neither can be mistaken for the other. A G6's
/// credential *is* the six characters printed on the transmitter, which are
/// not a secret and are typed as they are printed. A G7's is the sixteen-byte
/// AES key its pairing agreed, which is a real secret and has no printed form
/// at all, so it is written as hex.
pub fn credential(model: Model, text: &str) -> Result<Secret, String> {
    if model.is_g6() {
        return transmitter_id(text).map(Secret::new);
    }
    let bytes = hex(text).ok_or_else(|| {
        format!(
            "--credential for a {model} is the {KEY_LEN}-byte key its pairing agreed, written as {} hex characters",
            KEY_LEN * 2
        )
    })?;
    if bytes.len() == KEY_LEN {
        Ok(Secret::new(bytes))
    } else {
        Err(format!(
            "--credential for a {model} is {KEY_LEN} bytes ({} hex characters), not {}",
            KEY_LEN * 2,
            bytes.len()
        ))
    }
}

/// The secret a person reads off the device: a transmitter ID or a pairing
/// code, never a key.
pub fn pairing_secret(model: Model, text: &str) -> Result<Secret, String> {
    if model.is_g6() {
        return transmitter_id(text).map(Secret::new);
    }
    let digits = text.as_bytes();
    if digits.len() == CODE_LEN && digits.iter().all(u8::is_ascii_digit) {
        Ok(Secret::new(digits.to_vec()))
    } else {
        Err(format!(
            "--pairing-secret for a {model} is the {CODE_LEN} digits printed on the applicator, not `{text}`"
        ))
    }
}

fn transmitter_id(text: &str) -> Result<Vec<u8>, String> {
    let bytes = text.as_bytes();
    if bytes.len() == ID_LEN && bytes.iter().all(u8::is_ascii_alphanumeric) {
        Ok(bytes.to_vec())
    } else {
        Err(format!(
            "a G6's transmitter ID is the {ID_LEN} characters printed on it, not `{text}`"
        ))
    }
}

fn hex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok())
        .collect()
}

/// A BLE address written `aa:bb:cc:dd:ee:ff`.
pub fn address(text: &str) -> Result<Address, String> {
    let mut bytes = [0u8; 6];
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() == bytes.len() {
        for (byte, part) in bytes.iter_mut().zip(parts) {
            *byte = u8::from_str_radix(part, 16)
                .map_err(|_| format!("`{text}` is not an address: expected aa:bb:cc:dd:ee:ff"))?;
        }
        return Ok(Address::new(bytes));
    }
    Err(format!(
        "`{text}` is not an address: expected aa:bb:cc:dd:ee:ff"
    ))
}

/// The variant of the driver this name belongs to, checked against `--model`.
///
/// The driver is asked rather than told: it owns the name table, and a name
/// it does not recognise is a name the scanner should not be given either.
pub fn variant(model: Model, name: &str) -> Result<&'static Variant, String> {
    let adv = Advertisement {
        address: Address::new(ADDRESS),
        name: Some(name),
        service_uuids: &[cgm_dexcom::protocol::uuids::ADVERTISED_SERVICE],
        manufacturer: None,
        rssi: 0,
    };
    let found = DRIVER.matches(&adv).ok_or_else(|| {
        format!("the Dexcom driver matches no sensor advertising `{name}`: a {model} advertises `{}` and two characters", model.prefix())
    })?;
    if found.id == model.variant_id() {
        Ok(found)
    } else {
        Err(format!(
            "`{name}` is a {} name, not a {model}: pass --model for the sensor you are aiming at",
            found.id
        ))
    }
}

/// A fresh engine for one sensor.
pub fn engine(variant: &'static Variant, address: Address) -> Box<dyn CgmEngine> {
    DRIVER.new_engine(
        variant,
        Config {
            address: Some(address),
            utc_offset_secs: 0,
        },
    )
}

/// The wall clock, as the engine wants it.
pub fn wall_clock_now() -> UtcTime {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    UtcTime::from_unix_millis(u64::try_from(millis).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_is_a_variant_the_driver_knows() {
        for model in Model::ALL {
            assert_eq!(Model::parse(model.as_str()), Some(model));
            let name = model
                .advertised_name(if model.is_g6() { "ABCDEF" } else { "1234" })
                .expect("a name");
            let variant = variant(model, &name).unwrap_or_else(|e| panic!("{model}: {e}"));
            assert_eq!(variant.id, model.variant_id());
        }
        assert_eq!(Model::parse("stelo"), None, "the stelo feature is off");
        assert_eq!(Model::G6.to_string(), "g6");
    }

    #[test]
    fn a_name_is_built_from_the_tail_of_the_printed_secret() {
        assert_eq!(
            Model::G6.advertised_name("ABCDEF").as_deref(),
            Some("DexcomEF")
        );
        assert_eq!(Model::G7.advertised_name("1234").as_deref(), Some("DXCM34"));
        assert_eq!(
            Model::OnePlus.advertised_name("9876").as_deref(),
            Some("DX0276")
        );
        assert_eq!(
            Model::Flex.advertised_name("0042").as_deref(),
            Some("DX0442")
        );
        assert_eq!(Model::G7.advertised_name("1"), None);
    }

    #[test]
    fn a_name_of_the_wrong_model_is_refused_before_scanning() {
        let error = variant(Model::G6, "DXCM34").unwrap_err();
        assert_eq!(
            error,
            "`DXCM34` is a g7 name, not a g6: pass --model for the sensor you are aiming at"
        );
        let error = variant(Model::G7, "Libre3").unwrap_err();
        assert!(
            error.starts_with("the Dexcom driver matches no sensor"),
            "{error}"
        );
    }

    #[test]
    fn the_two_generations_read_their_secrets_differently() {
        assert_eq!(credential(Model::G6, "8GH42Z").unwrap().expose(), b"8GH42Z");
        assert_eq!(
            pairing_secret(Model::G6, "8GH42Z").unwrap().expose(),
            b"8GH42Z"
        );
        assert_eq!(
            credential(Model::G7, "00112233445566778899aabbccddeeff")
                .unwrap()
                .expose(),
            &[
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD,
                0xEE, 0xFF
            ]
        );
        assert_eq!(pairing_secret(Model::G7, "1234").unwrap().expose(), b"1234");
    }

    #[test]
    fn a_secret_of_the_wrong_shape_says_what_to_type() {
        assert_eq!(
            credential(Model::G6, "ABCDE").unwrap_err(),
            "a G6's transmitter ID is the 6 characters printed on it, not `ABCDE`"
        );
        assert!(credential(Model::G6, "ABCDE_").is_err());
        assert_eq!(
            credential(Model::G7, "00112233").unwrap_err(),
            "--credential for a g7 is 16 bytes (32 hex characters), not 4"
        );
        assert_eq!(
            credential(Model::G7, "zz").unwrap_err(),
            "--credential for a g7 is the 16-byte key its pairing agreed, written as 32 hex characters"
        );
        assert!(credential(Model::G7, "abc").is_err(), "odd length");
        assert_eq!(
            pairing_secret(Model::OnePlus, "12a4").unwrap_err(),
            "--pairing-secret for a one-plus is the 4 digits printed on the applicator, not `12a4`"
        );
        assert!(pairing_secret(Model::G7, "12345").is_err());
    }

    #[test]
    fn an_address_is_read_and_refused_as_written() {
        assert_eq!(address("de:ad:be:ef:01:02"), Ok(Address::new(ADDRESS)));
        for refused in [
            "",
            "de:ad:be:ef:01",
            "de:ad:be:ef:01:02:03",
            "de:ad:be:ef:01:zz",
        ] {
            assert!(address(refused).is_err(), "{refused}");
        }
        assert_eq!(
            address("x").unwrap_err(),
            "`x` is not an address: expected aa:bb:cc:dd:ee:ff"
        );
    }

    #[test]
    fn the_wall_clock_is_the_host_clock() {
        assert!(wall_clock_now().as_unix_millis() > 1_700_000_000_000);
    }
}
