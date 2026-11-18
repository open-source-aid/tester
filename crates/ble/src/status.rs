use pump_shared::BleResult;

const DESCRIPTIONS: [(&str, u8); 17] = [
    ("The handle is invalid.", 0x01),
    ("Reading is not permitted.", 0x02),
    ("Writing is not permitted.", 0x03),
    ("The command is improperly formatted.", 0x04),
    ("Authentication is insufficient.", 0x05),
    ("The request is not supported.", 0x06),
    ("The offset is invalid.", 0x07),
    ("Authorization is insufficient.", 0x08),
    ("The prepare queue is full.", 0x09),
    ("The attribute could not be found.", 0x0A),
    ("The attribute is not long.", 0x0B),
    ("The encryption key size is insufficient.", 0x0C),
    ("The value's length is invalid.", 0x0D),
    ("Unlikely error.", 0x0E),
    ("Encryption is insufficient.", 0x0F),
    ("The group type is not supported.", 0x10),
    ("Resources are insufficient.", 0x11),
];

pub fn status(message: &str) -> BleResult {
    let hex = message
        .split_once("ATT error: 0x")
        .and_then(|(_, code)| u8::from_str_radix(code.get(..2)?, 16).ok());
    let decimal = || {
        let (_, tail) = message.split_once("error ")?;
        tail.strip_suffix(".)")?.parse().ok()
    };
    let described = || {
        DESCRIPTIONS
            .iter()
            .find(|(text, _)| *text == message)
            .map(|(_, code)| *code)
    };
    hex.or_else(decimal)
        .or_else(described)
        .map_or(BleResult::Failed, BleResult::Status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_att_codes_from_bluez_and_corebluetooth_messages() {
        assert_eq!(
            status("org.bluez.Error.Failed: Operation failed with ATT error: 0x8b"),
            BleResult::Status(0x8B)
        );
        assert_eq!(
            status("The operation couldn’t be completed. (CBATTErrorDomain error 135.)"),
            BleResult::Status(0x87)
        );
        assert_eq!(
            status("Encryption is insufficient."),
            BleResult::Status(0x0F)
        );
        for (text, code) in DESCRIPTIONS {
            assert_eq!(status(text), BleResult::Status(code));
        }
    }

    #[test]
    fn anything_else_is_a_plain_failure() {
        for message in [
            "",
            "Channel closed",
            "Operation failed with ATT error: 0xZZ",
            "Operation failed with ATT error: 0x",
            "an error 300.)",
            "an error 12",
        ] {
            assert_eq!(status(message), BleResult::Failed);
        }
    }
}
