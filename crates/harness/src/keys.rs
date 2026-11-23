use std::collections::{BTreeMap, BTreeSet};

use curve25519_dalek::montgomery::MontgomeryPoint;
use pump_shared::KeyRef;

/// The X25519 keypairs the harness holds on the engine's behalf, as a host does.
///
/// Test-shaped, and no secrecy is claimed: a key no test seeded is filled in
/// from its own name, so a transcript reads the same on every run, the way
/// [`Harness::random`](crate::Harness::random) counts bytes by default. A key
/// is created on first use and then stays put for the harness's life, so the
/// public half an engine reads and the agreement it later asks for describe
/// one keypair.
///
/// Both knobs are plain fields, like the harness's own: seed [`secrets`] to
/// pin a keypair, and name a key in [`failing`] to make the keystore refuse
/// it, so an engine's failure path is reachable.
///
/// A [`SimDevice`](pump_shared::SimDevice) may answer key agreement itself.
/// The keystore is asked first, and a name it does not hold is left to the
/// device; once the device has answered for a name, the keystore refuses its
/// public half rather than serve the half of some other keypair.
///
/// [`secrets`]: Self::secrets
/// [`failing`]: Self::failing
#[derive(Debug, Default)]
pub struct Keystore {
    /// The secret halves, by name. A name absent here is filled in on first use.
    pub secrets: BTreeMap<KeyRef, [u8; 32]>,
    /// Names the keystore refuses to serve at all.
    pub failing: BTreeSet<KeyRef>,
    device: BTreeSet<KeyRef>,
}

impl Keystore {
    /// The public half of the key of that name, creating the key on first use.
    pub fn public(&mut self, key: &KeyRef) -> Option<[u8; 32]> {
        let secret = self.secret(key)?;
        Some(MontgomeryPoint::mul_base_clamped(secret).to_bytes())
    }

    /// The X25519 shared secret between the key of that name and `peer_public`,
    /// creating the key on first use.
    pub fn agree(&mut self, key: &KeyRef, peer_public: &[u8; 32]) -> Option<[u8; 32]> {
        let secret = self.secret(key)?;
        Some(MontgomeryPoint(*peer_public).mul_clamped(secret).to_bytes())
    }

    /// Whether the keystore refuses both halves of this name outright.
    pub(crate) fn fails(&self, key: &KeyRef) -> bool {
        self.failing.contains(key)
    }

    /// Whether the keystore serves no key of this name: it fails, or the
    /// simulated device answered for it.
    fn refuses(&self, key: &KeyRef) -> bool {
        self.fails(key) || self.device.contains(key)
    }

    /// Whether a key of that name has been seeded or already created.
    pub(crate) fn holds(&self, key: &KeyRef) -> bool {
        self.secrets.contains_key(key) && !self.refuses(key)
    }

    /// Record that the simulated device answered for this name, so the
    /// keystore stops serving a public half that would not match it.
    pub(crate) fn left_to_device(&mut self, key: KeyRef) {
        self.device.insert(key);
    }

    fn secret(&mut self, key: &KeyRef) -> Option<[u8; 32]> {
        if self.refuses(key) {
            return None;
        }
        Some(
            *self
                .secrets
                .entry(key.clone())
                .or_insert_with(|| from_name(key)),
        )
    }
}

/// The secret half of an unseeded key: the name's bytes, offset by position so
/// that a short name does not simply repeat. No key derivation and no secret,
/// only stable, distinct filler.
fn from_name(key: &KeyRef) -> [u8; 32] {
    let name = key.as_str().as_bytes();
    core::array::from_fn(|i| match name.len() {
        0 => i as u8,
        len => name[i % len].wrapping_add(i as u8),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> KeyRef {
        KeyRef::new(name)
    }

    #[test]
    fn a_key_is_created_once_and_then_stays_put() {
        let mut keys = Keystore::default();
        let first = keys.public(&named("server")).unwrap();
        assert_eq!(keys.public(&named("server")), Some(first));
        assert_eq!(keys.secrets[&named("server")], from_name(&named("server")));
        assert!(keys.holds(&named("server")));
        assert_ne!(keys.public(&named("other")), Some(first));
    }

    #[test]
    fn an_unseeded_secret_follows_the_name_even_when_it_is_empty() {
        assert_eq!(from_name(&named(""))[..4], [0, 1, 2, 3]);
        let server = from_name(&named("ab"));
        assert_eq!(server[..4], [b'a', b'b' + 1, b'a' + 2, b'b' + 3]);
    }

    #[test]
    fn a_seeded_keypair_is_the_one_served() {
        let mut keys = Keystore::default();
        keys.secrets.insert(named("server"), [7; 32]);
        assert_eq!(
            keys.public(&named("server")),
            Some(MontgomeryPoint::mul_base_clamped([7; 32]).to_bytes())
        );
    }

    #[test]
    fn both_halves_agree_on_the_same_shared_secret() {
        let mut keys = Keystore::default();
        let peer_secret = [9u8; 32];
        let peer_public = MontgomeryPoint::mul_base_clamped(peer_secret).to_bytes();
        let host_public = keys.public(&named("server")).unwrap();
        let shared = keys.agree(&named("server"), &peer_public).unwrap();
        // What the peer computes from its own secret and the host's public half.
        let mine = MontgomeryPoint(host_public)
            .mul_clamped(peer_secret)
            .to_bytes();
        assert_eq!(shared, mine);
    }

    #[test]
    fn a_failing_name_is_served_neither_half() {
        let mut keys = Keystore::default();
        keys.failing.insert(named("server"));
        assert_eq!(keys.public(&named("server")), None);
        assert_eq!(keys.agree(&named("server"), &[1; 32]), None);
        assert!(keys.fails(&named("server")));
        assert!(!keys.holds(&named("server")));
    }

    #[test]
    fn a_name_the_device_answered_for_is_no_longer_served() {
        let mut keys = Keystore::default();
        keys.left_to_device(named("server"));
        assert_eq!(keys.public(&named("server")), None);
        assert!(!keys.fails(&named("server")));
        assert!(!keys.holds(&named("server")));
    }
}
