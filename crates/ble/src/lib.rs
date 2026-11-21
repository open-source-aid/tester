#![forbid(unsafe_code)]

mod console;
mod name;
#[cfg(feature = "radio")]
mod radio;
mod status;

pub use console::{Console, ConsoleError};
pub use name::{Advertised, NameError, SensorName, SimulatorName};
#[cfg(feature = "radio")]
pub use radio::{open_console, Radio};
pub use status::status;

pub fn os_random(len: usize) -> Vec<u8> {
    let mut bytes = vec![0; len];
    getrandom::fill(&mut bytes).expect("the OS random source");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_fresh_bytes_from_the_os() {
        assert_eq!(os_random(32).len(), 32);
        assert_ne!(os_random(32), os_random(32));
    }
}
