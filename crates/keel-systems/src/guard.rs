//! Whether the boot partitions still hold the boot set the state names.
//!
//! The state is worth only what the partitions it describes are worth: an
//! update of the installed system, run while the other system is up, writes
//! the slot that does not start next — the installed system's own boot,
//! vendor_boot and dtbo — so the copy of that boot set in the store is the
//! boot set of a version that is no longer there. A switch back that wrote
//! it would start the phone with the old boot image and the new system and
//! vendor partitions: it does not come up, or it comes up part way and
//! crashes.
//!
//! This is kept apart from the parts of the store that only a phone can
//! reach, so that what decides it can be tried without a phone: the
//! partitions are read through `by_name`, which is a directory of files
//! standing in for them in the tests.

use crate::state::{Forgotten, Saved, State};
use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use keel_vab::Slot;
use std::fs::File;
use std::io::Read;
use std::path::Path;

const CHUNK: usize = 1 << 20;

/// The SHA-256 of all of a partition.
pub(crate) fn sum_of(path: &Path) -> Result<String> {
    let mut f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    keel_block::drop_cache(&f)?;
    let mut sum = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = f.read(&mut buf).with_context(|| format!("read {}", path.display()))?;
        if n == 0 {
            break;
        }
        sum.update(&buf[..n]);
    }
    Ok(sum.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Whether the boot partitions of `slot` still hold the boot set `saved`
/// names, as many images of it as the phone keeps.
pub(crate) fn holds(by_name: &Path, slot: Slot, saved: &Saved) -> Result<bool> {
    if sum_of(&crate::store::partition(by_name, crate::store::BOOT, slot))? != saved.boot {
        return Ok(false);
    }
    if saved.init_boot.is_empty() {
        return Ok(true);
    }
    Ok(sum_of(&crate::store::partition(by_name, crate::store::INIT_BOOT, slot))? == saved.init_boot)
}

/// Whether anything has been written over what the other system was
/// installed against: the installed system's boot set and the vendor_boot
/// and dtbo it starts with, in the slot the other system is in the other
/// one of; or the other system's own boot set, in the slot the state says
/// it is in.
///
/// An update of the installed system, run from the other system — which is
/// where the stock updater writes, the slot that does not start next —
/// changes exactly those partitions, and the state has to say so rather
/// than read as unchanged.
pub(crate) fn changed(by_name: &Path, state: &State, current: Slot) -> Result<bool> {
    let Some(system) = &state.system else { return Ok(false) };
    let occupied = match &state.occupied {
        Some(suffix) => Some(Slot::from_suffix(suffix).with_context(|| format!("the slot {suffix:?} the other system is in"))?),
        None => None,
    };
    // The installed system's slot: the other system runs from the one the
    // state says its boot set is in, so the installed system is in the
    // other one. With no switch made, it is the one the phone runs.
    let installed = match occupied {
        Some(occupied) => occupied.other(),
        None => current,
    };
    let mut changed = false;
    if let Some(saved) = &state.saved {
        changed |= !holds(by_name, installed, saved)?;
    }
    for (name, want) in [("vendor_boot", &system.vendor_boot), ("dtbo", &system.dtbo)] {
        changed |= sum_of(&crate::store::partition(by_name, name, installed))? != *want;
    }
    if let Some(occupied) = occupied {
        changed |= !holds(by_name, occupied, &system.set)?;
    }
    Ok(changed)
}

/// Refuse a switch back that would put a boot set saved before onto a
/// system that is no longer the one it was saved from; `slot` is the one
/// the other system's boot set is in, so the installed system is in the
/// other one.
///
/// So say so, with what can be done about it, instead of writing and
/// leaving a phone that has to be got out of with fastboot and no
/// explanation.
pub(crate) fn written_over(by_name: &Path, state: &State, slot: Slot) -> Result<()> {
    let (Some(saved), Some(system)) = (&state.saved, &state.system) else { return Ok(()) };
    ensure!(
        holds(by_name, slot, &system.set)?,
        "the other system's boot set is no longer in the partitions of slot {}, so something wrote over it; installing it again is the safe way back",
        slot.suffix()
    );
    let installed = slot.other();
    ensure!(
        holds(by_name, installed, saved)?,
        "the installed system's boot set in slot {} is not the one that was saved, so an update has replaced it; writing the saved one back would start the phone with an old boot image and a new system - delete the other system and install it again",
        installed.suffix()
    );
    for (name, want) in [("vendor_boot", &system.vendor_boot), ("dtbo", &system.dtbo)] {
        ensure!(
            sum_of(&crate::store::partition(by_name, name, installed))? == *want,
            "the installed system's {name} in slot {} was replaced after the other system was installed, so the other system is not what it was installed as - delete it and install it again",
            installed.suffix()
        );
    }
    Ok(())
}

/// The slot to start where the saved copy is not the boot set the
/// partitions hold any more: the installed system's, which is the other one
/// from `occupied`, the slot the other system's boot set is in.
///
/// Starting `occupied` instead would start the other system after its boot
/// set was let go of — a system the state no longer says is there, and the
/// one whose root [`crate::install::remove`] is about to delete.
pub(crate) fn start_from(occupied: Slot) -> Slot {
    occupied.other()
}

/// What is let go of when the saved copy is not the boot set the partitions
/// hold any more: the copy, the slot the other system's boot set was in, and
/// the other system's record.
///
/// The three go together because the state has to be one thing or the other.
/// An update written over the other system's slot leaves that system's boot
/// set gone from where it was, and nothing starts it any more. Keeping
/// `system` would say the other system is still installed while no slot holds
/// its boot set, and every reading of `occupied == None` would then be wrong:
/// a switch back finds nothing to switch, and the next switch to the other
/// system saves whatever is in the slot now as the installed system's copy,
/// which is how a phone ends up starting the other system every time.
///
/// The room it takes is not let go of, though: the area is still in the data
/// partition, taking the room it takes, and a reinstall of the same package
/// looks for the very file it is in. So it goes on `forgotten` — no longer
/// an installed system, still an area to be taken away — and `why` says what
/// wrote over it, for the page and for [`let_go_at`]'s kmsg. What was
/// forgotten before stays on the list beside it: it is still an area, and
/// nothing else would name it.
///
/// Only the state is touched here, so this half of starting a slot as it
/// stands can be tried without a phone: the boot control is the other half.
pub(crate) fn let_go(state: &mut State, why: &str) {
    state.occupied = None;
    state.saved = None;
    if let Some(system) = state.system.take() {
        // Pushed, not put in place of what is there: an entry left from an
        // earlier system is still an area taking room, and writing over it
        // would lose the name of that one for good.
        state.forgotten.push(Forgotten { id: system.id, name: system.name, root: system.root, why: why.to_string() });
    }
}

/// The state as it is left after [`let_go`], loaded, let go of and saved at
/// `path`; answers with what was forgotten, where anything was.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn let_go_at(path: &std::path::Path, why: &str) -> Result<Option<String>> {
    let mut state = State::load(path)?;
    let before = state.forgotten.len();
    let_go(&mut state, why);
    let id = state.forgotten.get(before).map(|forgotten| forgotten.id.clone());
    state.save(path)?;
    Ok(id)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::state::tests::area;
    use crate::state::{Saved, System};
    use crate::store;

    /// A phone's boot partitions as a directory of files: the installed
    /// system's boot set in slot `_a`, the other system's in slot `_b`, the
    /// installed system's set saved before the other one took the slot, and
    /// the other system installed against the installed one's vendor_boot
    /// and dtbo.
    pub(crate) fn phone(old: &[u8], new: &[u8]) -> (tempfile::TempDir, std::path::PathBuf, State) {
        let dir = tempfile::tempdir().unwrap();
        let at = dir.path().to_path_buf();
        let saved = Saved { entry: 0, boot: hex(old), init_boot: hex(old) };
        let set = Saved { entry: 1, boot: hex(new), init_boot: hex(new) };
        for (name, bytes) in [(store::BOOT, old), (store::INIT_BOOT, old)] {
            std::fs::write(store::partition(&at, name, Slot::A), bytes).unwrap();
        }
        for (name, bytes) in [(store::BOOT, new), (store::INIT_BOOT, new)] {
            std::fs::write(store::partition(&at, name, Slot::B), bytes).unwrap();
        }
        for name in ["vendor_boot", "dtbo"] {
            std::fs::write(store::partition(&at, name, Slot::A), b"stock").unwrap();
        }
        let state = State {
            saved: Some(saved),
            system: Some(System {
                id: "test".into(),
                name: "Test".into(),
                version: "1".into(),
                filesystem: "ext4".into(),
                init: "/sbin/init".into(),
                root: area("/data/skyward/systems/test", 1 << 20),
                set,
                own_ramdisk: None,
                vendor_boot: hex(b"stock"),
                dtbo: hex(b"stock"),
            }),
            occupied: Some(Slot::B.suffix().to_string()),
            ..State::default()
        };
        (dir, at, state)
    }

    fn hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn a_switch_back_over_an_updated_system_is_refused() {
        let (_dir, at, state) = phone(b"old boot", b"other system boot");
        assert!(written_over(&at, &state, Slot::B).is_ok(), "nothing was written over yet");
        // An update of the installed system, run from the other one, writes
        // the slot the installed system is in: its boot no longer holds what
        // was saved before the other system took the other slot.
        std::fs::write(store::partition(&at, store::BOOT, Slot::A), b"new boot").unwrap();
        let why = written_over(&at, &state, Slot::B).unwrap_err();
        assert!(format!("{why:#}").contains("would start the phone with an old boot image"), "says what the write would do: {why:#}");
        assert!(changed(&at, &state, Slot::A).unwrap(), "a written over boot set is a change the page sees");
        let mut unafraid = state.clone();
        unafraid.saved = None;
        assert!(written_over(&at, &unafraid, Slot::B).is_ok(), "with nothing saved there is nothing that could be put back");
    }

    #[test]
    fn an_updated_vendor_boot_or_dtbo_is_a_change_even_with_the_boot_set_untouched() {
        let (_dir, at, state) = phone(b"old boot", b"other system boot");
        assert!(!changed(&at, &state, Slot::A).unwrap(), "nothing was written over");
        std::fs::write(store::partition(&at, "dtbo", Slot::A), b"updated stock").unwrap();
        let why = written_over(&at, &state, Slot::B).unwrap_err();
        assert!(format!("{why:#}").contains("delete it and install it again"), "says what to do: {why:#}");
        assert!(changed(&at, &state, Slot::A).unwrap(), "an update of dtbo is seen");
    }

    #[test]
    fn the_other_systems_own_boot_set_going_away_is_a_change() {
        let (_dir, at, state) = phone(b"old boot", b"other system boot");
        std::fs::write(store::partition(&at, store::BOOT, Slot::B), b"something else").unwrap();
        let why = written_over(&at, &state, Slot::B).unwrap_err();
        assert!(format!("{why:#}").contains("installing it again is the safe way back"), "says what to do: {why:#}");
        assert!(changed(&at, &state, Slot::A).unwrap(), "the other system's boot set leaving slot B is a change");
    }

    #[test]
    fn a_slot_started_as_it_stands_is_the_installed_systems_not_the_other_ones() {
        // The boot control itself is only reachable on a phone; what is
        // decided here is which slot it is told to start.
        assert_eq!(start_from(Slot::B), Slot::A, "the other system is in B, so the updated installed system is in A");
        assert_eq!(start_from(Slot::A), Slot::B, "and the other way round");
    }

    #[test]
    fn letting_go_of_the_saved_set_leaves_no_other_system_installed_and_keeps_its_room_named() {
        let (_dir, _at, state) = phone(b"old boot", b"other system boot");
        let root = state.system.as_ref().unwrap().root.clone();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("systems.toml");
        state.save(&path).unwrap();
        let mut loaded = State::load(&path).unwrap();
        assert_eq!(loaded.occupied.as_deref(), Some("_b"));
        assert!(loaded.saved.is_some());
        let_go(&mut loaded, "an update wrote over it");
        loaded.save(&path).unwrap();
        let after = State::load(&path).unwrap();
        assert!(after.occupied.is_none(), "nothing is in a slot as its own boot set any more");
        assert!(after.saved.is_none(), "there is no copy left to write back");
        assert!(after.system.is_none(), "the other system's boot set is gone from where it was, so its record goes with it");
        assert_eq!(after.forgotten.len(), 1, "one system was let go of, so one room is named");
        let forgotten = after.forgotten.into_iter().next().expect("its root is still in the data partition, taking the room it takes");
        assert_eq!(forgotten.root, root, "the file a reinstall of the same package would find is the one named here");
        assert!(forgotten.why.contains("update"), "why it was let go of: {}", forgotten.why);
    }

    #[test]
    fn letting_go_of_another_system_keeps_what_was_let_go_of_before() {
        // Forgetting one and installing another is a way a second entry
        // is reached: the earlier room is still an area, and nothing else
        // names it, so a new one is added beside it rather than in its place.
        let (_dir, _at, mut state) = phone(b"old boot", b"other system boot");
        let earlier = state.system.as_ref().unwrap().root.clone();
        state.forgotten.push(Forgotten { id: "earlier".into(), name: "Earlier".into(), root: earlier.clone(), why: "an update wrote over it".into() });
        let now = state.system.as_ref().unwrap().root.clone();
        let_go(&mut state, "an update wrote over it");
        assert_eq!(state.forgotten.len(), 2, "both rooms are named");
        assert_eq!(state.forgotten[0].id, "earlier", "the one let go of before is still first");
        assert_eq!(state.forgotten[0].root, earlier, "and still names its own file");
        assert_eq!(state.forgotten[1].root, now, "the one let go of now names the one it was installed in");
    }

    #[test]
    fn letting_go_at_a_path_saves_what_was_let_go_of() {
        let (_dir, _at, state) = phone(b"old boot", b"other system boot");
        let root = state.system.as_ref().unwrap().root.clone();
        let id = state.system.as_ref().unwrap().id.clone();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("systems.toml");
        state.save(&path).unwrap();
        let said = let_go_at(&path, "an update wrote over it").unwrap();
        assert_eq!(said.as_deref(), Some(id.as_str()), "says whose room is now forgotten");
        let after = State::load(&path).unwrap();
        assert!(after.occupied.is_none() && after.saved.is_none() && after.system.is_none(), "nothing of it starts any more");
        assert_eq!(after.forgotten.into_iter().next().map(|forgotten| forgotten.root), Some(root), "the room it takes is still named after the save it was written in");
    }
}
