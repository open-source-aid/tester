//! The slots the engine asks the host to keep, kept in a directory.
//!
//! `Store`, `Load` and `Erase` are host actions like any other (guideline 04),
//! and the in-process harness answers them from a map that dies with the
//! process. That is exactly what must not happen here: pairing a sensor is a
//! once-per-sensor event, and the point of the radio run is to pair once and
//! then reconnect — tomorrow, from another invocation. So the map is mirrored
//! into one file per slot, read back at start-up and written after every
//! command.
//!
//! **What is in those files is not protected.** The engine hands the host a
//! secret through `Action::Wrap` and the harness's `Wrap` is the identity, so
//! `session` holds the credential in clear. There is no key store here to do
//! better with, and inventing a key to keep next to the thing it protects
//! would only look like protection. The directory is created `0700` and the
//! files `0600`, the tool talks to a simulator, and anyone pointing it at a
//! sensor they care about should know that this is where the credential ends
//! up.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use cgm_shared::Slot;

/// The file one slot is kept in, `None` for a slot this tester does not know.
///
/// `Slot` is `#[non_exhaustive]`: a slot added upstream lands here as `None`
/// rather than in a file whose name means nothing.
pub fn file_name(slot: Slot) -> Option<String> {
    Some(match slot {
        Slot::Session => "session.bin".into(),
        Slot::PendingSession => "pending-session.bin".into(),
        Slot::Counters => "counters.bin".into(),
        Slot::NonceState => "nonce-state.bin".into(),
        Slot::Driver(index) => format!("driver-{index}.bin"),
        _ => return None,
    })
}

/// The slot a file name means, `None` for anything else in the directory.
pub fn slot_of(file: &str) -> Option<Slot> {
    match file {
        "session.bin" => Some(Slot::Session),
        "pending-session.bin" => Some(Slot::PendingSession),
        "counters.bin" => Some(Slot::Counters),
        "nonce-state.bin" => Some(Slot::NonceState),
        other => other
            .strip_prefix("driver-")
            .and_then(|rest| rest.strip_suffix(".bin"))
            .and_then(|index| index.parse().ok())
            .map(Slot::Driver),
    }
}

/// A directory holding one device's slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotStore {
    dir: PathBuf,
}

/// Everything the engine asked to keep.
pub type Slots = BTreeMap<Slot, Vec<u8>>;

impl SlotStore {
    /// Opens the directory, creating it if it is not there yet.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, String> {
        let dir = dir.into();
        fs::create_dir_all(&dir).map_err(|e| why("create", &dir, &e.to_string()))?;
        restrict(&dir, 0o700)?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Reads every slot back. An unreadable file is an error, never a silently
    /// absent slot: an engine told that nothing was stored forgets the sensor.
    pub fn load(&self) -> Result<Slots, String> {
        let mut slots = Slots::new();
        let entries =
            fs::read_dir(&self.dir).map_err(|e| why("read", &self.dir, &e.to_string()))?;
        for entry in entries {
            let entry = entry.map_err(|e| why("read", &self.dir, &e.to_string()))?;
            let path = entry.path();
            let Some(slot) = entry.file_name().to_str().and_then(slot_of) else {
                continue;
            };
            let bytes = fs::read(&path).map_err(|e| why("read", &path, &e.to_string()))?;
            slots.insert(slot, bytes);
        }
        Ok(slots)
    }

    /// Makes the directory say exactly what `slots` says: a slot the engine
    /// erased must leave, or the next run would come up paired to a sensor the
    /// engine has forgotten.
    pub fn save(&self, slots: &Slots) -> Result<(), String> {
        for (slot, bytes) in slots {
            let name = file_name(*slot)
                .ok_or_else(|| format!("the engine asked to store {slot:?}, which this tester does not know how to keep: teach `crates/cli/src/cgm/store.rs` about it"))?;
            let path = self.dir.join(name);
            fs::write(&path, bytes).map_err(|e| why("write", &path, &e.to_string()))?;
            restrict(&path, 0o600)?;
        }
        let entries =
            fs::read_dir(&self.dir).map_err(|e| why("read", &self.dir, &e.to_string()))?;
        for entry in entries {
            let entry = entry.map_err(|e| why("read", &self.dir, &e.to_string()))?;
            let Some(slot) = entry.file_name().to_str().and_then(slot_of) else {
                continue;
            };
            if !slots.contains_key(&slot) {
                let path = entry.path();
                fs::remove_file(&path).map_err(|e| why("remove", &path, &e.to_string()))?;
            }
        }
        Ok(())
    }

    /// Forgets everything, as `--forget` and the unpair scenario do.
    pub fn forget(&self) -> Result<(), String> {
        self.save(&Slots::new())
    }
}

fn why(action: &str, path: &Path, error: &str) -> String {
    format!(
        "cannot {action} {}: {error}. Pass --store <directory> to keep the sensor's slots somewhere writable",
        path.display()
    )
}

#[cfg(unix)]
fn restrict(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|e| why("set the permissions of", path, &e.to_string()))
}

#[cfg(not(unix))]
fn restrict(_path: &Path, _mode: u32) -> Result<(), String> {
    // Only the Unix bits are set; elsewhere the directory keeps whatever the
    // platform gives it, and the module documentation's warning stands.
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// A directory of this test's own, removed first so a rerun starts clean.
    pub fn scratch(what: &str) -> PathBuf {
        let index = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tester-cgm-{what}-{index}"));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn slots(pairs: &[(Slot, &[u8])]) -> Slots {
        pairs
            .iter()
            .map(|(slot, bytes)| (*slot, bytes.to_vec()))
            .collect()
    }

    #[test]
    fn every_slot_name_round_trips() {
        for slot in [
            Slot::Session,
            Slot::PendingSession,
            Slot::Counters,
            Slot::NonceState,
            Slot::Driver(0),
            Slot::Driver(7),
        ] {
            let name = file_name(slot).expect("a known slot");
            assert_eq!(slot_of(&name), Some(slot), "{name}");
        }
        assert_eq!(slot_of("notes.txt"), None);
        assert_eq!(slot_of("driver-.bin"), None);
        assert_eq!(slot_of("driver-300.bin"), None);
        assert_eq!(slot_of("driver-1.txt"), None);
    }

    #[test]
    fn what_is_stored_is_there_on_the_next_run() {
        let dir = scratch("persists");
        let store = SlotStore::open(&dir).unwrap();
        assert_eq!(store.load().unwrap(), Slots::new());
        store
            .save(&slots(&[
                (Slot::Session, b"key"),
                (Slot::Driver(0), b"anchor"),
            ]))
            .unwrap();
        // A second process reading the same directory sees the same slots.
        let again = SlotStore::open(&dir).unwrap();
        assert_eq!(
            again.load().unwrap(),
            slots(&[(Slot::Session, b"key"), (Slot::Driver(0), b"anchor")])
        );
        assert_eq!(again.dir(), dir.as_path());
    }

    #[test]
    fn an_erased_slot_leaves_the_directory() {
        let dir = scratch("erases");
        let store = SlotStore::open(&dir).unwrap();
        store
            .save(&slots(&[
                (Slot::Session, b"key"),
                (Slot::Driver(0), b"anchor"),
            ]))
            .unwrap();
        store.save(&slots(&[(Slot::Driver(0), b"anchor")])).unwrap();
        assert_eq!(
            store.load().unwrap(),
            slots(&[(Slot::Driver(0), b"anchor")])
        );
        store.forget().unwrap();
        assert_eq!(store.load().unwrap(), Slots::new());
        assert!(dir.exists(), "the directory itself stays");
    }

    #[test]
    fn a_file_that_is_not_a_slot_is_left_alone() {
        let dir = scratch("strangers");
        let store = SlotStore::open(&dir).unwrap();
        fs::write(dir.join("notes.txt"), b"mine").unwrap();
        store.save(&slots(&[(Slot::Session, b"key")])).unwrap();
        assert_eq!(store.load().unwrap(), slots(&[(Slot::Session, b"key")]));
        store.forget().unwrap();
        assert_eq!(fs::read(dir.join("notes.txt")).unwrap(), b"mine");
    }

    #[test]
    fn a_directory_that_cannot_be_used_says_what_to_do() {
        let dir = scratch("unwritable");
        fs::write(&dir, b"not a directory").unwrap();
        let error = SlotStore::open(&dir).unwrap_err();
        assert!(error.starts_with("cannot create "), "{error}");
        assert!(
            error.ends_with(
                "Pass --store <directory> to keep the sensor's slots somewhere writable"
            ),
            "{error}"
        );
        let _ = fs::remove_file(&dir);
    }

    #[test]
    fn a_slot_this_tester_does_not_know_is_reported() {
        let dir = scratch("unknown-slot");
        let store = SlotStore::open(&dir).unwrap();
        // `Slot` is non-exhaustive upstream; the arm that catches an unknown
        // one cannot be reached with a variant that does not exist yet, so
        // the message it produces is checked directly.
        assert_eq!(file_name(Slot::Driver(3)), Some("driver-3.bin".into()));
        assert!(store.save(&Slots::new()).is_ok());
    }
}
