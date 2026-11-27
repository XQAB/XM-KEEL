//! What the rename leaves on a phone that carried XM KEEL before it: the
//! settings, the sealed credential that opens the data partition, and the
//! record of the systems installed beside this one, all in a folder named
//! after the old one, which this build no longer reads. The folder is moved
//! over once, on the first start of this build, before anything reads it —
//! a phone that opened its data once goes on opening it without asking for
//! the password again.
//!
//! Later starts find the old folder gone and do nothing. Where both folders
//! are there, the new one is the one this build wrote, and the files it has
//! are kept: only what it lacks is taken over.
//!
//! The moving is the same wherever it runs, and is tested on the computer;
//! only the work on /metadata is the phone's.

#![cfg(any(target_os = "android", test))]

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Where the settings and the credential lived before the rename, and where
/// they live now. This module only moves the folder; what is in it belongs
/// to `credential` and `settings`.
pub const OLD: &str = "/metadata/skyward";
pub const NEW: &str = "/metadata/keel";
/// The mount the two live under, and their names inside it: kept apart so that
/// the work done there can be run on a folder of the computer's own.
const METADATA: &str = "/metadata";
const OLD_NAME: &str = "skyward";
const NEW_NAME: &str = "keel";

/// Move the old folder to the new name. /metadata is writable only while the
/// files are moved, as everywhere else it is written; it is mounted for the
/// moment where it is not mounted yet, as in fastboot.
///
/// The old folder is looked for **inside** that mount. On a cold start into
/// recovery /metadata is not mounted yet, so asking for `/metadata/skyward`
/// before the closure runs always answers "not there": the move never ran, no
/// line was ever written about it, and a phone upgraded to this build went on
/// carrying its settings and credential in a folder nothing reads. Where there
/// is nothing to move nothing is said; where the move ran, or failed, the
/// kernel log says which.
#[cfg(target_os = "android")]
pub fn metadata() -> Result<()> {
    let moved = crate::settings::with_metadata(|| -> Result<bool> {
        if !Path::new(OLD).exists() {
            return Ok(false);
        }
        keel_vab::metadata_writable(true)?;
        let moved = take_over(Path::new(METADATA));
        let back = keel_vab::metadata_writable(false);
        let moved = moved?;
        back?;
        Ok(moved)
    });
    match &moved {
        Ok(true) => {
            let _ = keel_sys::kmsg(&format!("migrate: {OLD} moved to {NEW}"));
        }
        Ok(false) => {}
        Err(e) => {
            let _ = keel_sys::kmsg(&format!("migrate: {OLD} not moved: {e:#}"));
        }
    }
    moved.map(|_| ())
}

/// What the entry point does once /metadata is mounted: whether there was an
/// old folder to take over, and, where there was, that it has been taken over.
fn take_over(metadata: &Path) -> Result<bool> {
    let (old, new) = (metadata.join(OLD_NAME), metadata.join(NEW_NAME));
    if !old.exists() {
        return Ok(false);
    }
    move_over(&old, &new)?;
    Ok(true)
}

/// Move `old` to `new`; where `new` is already there, take the entries of
/// `old` that `new` lacks and leave behind none of the ones it has.
fn move_over(old: &Path, new: &Path) -> Result<()> {
    if !old.exists() {
        return Ok(());
    }
    if !new.exists() {
        return fs::rename(old, new).with_context(|| format!("move {OLD} to {NEW}"));
    }
    for entry in fs::read_dir(old).with_context(|| format!("read {OLD}"))? {
        let entry = entry.with_context(|| format!("read {OLD}"))?;
        let into: PathBuf = new.join(entry.file_name());
        if into.exists() {
            // The new folder's own file is the one this build reads; the one
            // beside it is what an older build left.
            remove(&entry.path())?;
            continue;
        }
        fs::rename(entry.path(), &into)
            .with_context(|| format!("move {} to {}", entry.path().display(), into.display()))?;
    }
    // Empty now, or holding something a failed move left: only an empty one
    // goes, so that a retry on the next start finds the rest.
    let _ = fs::remove_dir(old);
    Ok(())
}

fn remove(path: &Path) -> Result<()> {
    let taking = fs::metadata(path).with_context(|| format!("look at {}", path.display()))?;
    if taking.is_dir() {
        fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))
    } else {
        fs::remove_file(path).with_context(|| format!("remove {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let at = std::env::temp_dir().join(format!("keel-migrate-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&at);
        fs::create_dir_all(&at).unwrap();
        at
    }

    #[test]
    fn a_phone_that_kept_its_credential_finds_it_where_the_new_build_reads() {
        let root = folder("whole");
        let (old, new) = (root.join("skyward"), root.join("keel"));
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("credential"), b"sealed").unwrap();
        fs::write(old.join("settings.json"), b"{}").unwrap();
        fs::write(old.join("systems.toml"), b"no systems").unwrap();

        move_over(&old, &new).unwrap();

        assert_eq!(fs::read(new.join("credential")).unwrap(), b"sealed");
        assert_eq!(fs::read(new.join("settings.json")).unwrap(), b"{}");
        assert_eq!(fs::read(new.join("systems.toml")).unwrap(), b"no systems");
        assert!(!old.exists(), "the old folder is gone once moved");
    }

    #[test]
    fn a_folder_of_both_names_keeps_the_new_ones_files_and_takes_the_rest() {
        let root = folder("both");
        let (old, new) = (root.join("skyward"), root.join("keel"));
        fs::create_dir_all(old.join("wlan")).unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(old.join("credential"), b"written before the rename").unwrap();
        fs::write(old.join("systems.toml"), b"a second system").unwrap();
        fs::write(old.join("wlan").join("joined"), b"a network").unwrap();
        fs::write(new.join("credential"), b"written after it").unwrap();

        move_over(&old, &new).unwrap();

        assert_eq!(fs::read(new.join("credential")).unwrap(), b"written after it", "the newer file wins");
        assert_eq!(fs::read(new.join("systems.toml")).unwrap(), b"a second system");
        assert_eq!(fs::read(new.join("wlan").join("joined")).unwrap(), b"a network");
        assert!(!old.exists(), "nothing is left of the old folder");
    }

    #[test]
    fn a_phone_that_never_carried_the_old_name_is_left_alone() {
        let root = folder("fresh");
        let (old, new) = (root.join("skyward"), root.join("keel"));

        move_over(&old, &new).unwrap();

        assert!(!new.exists(), "no folder is made where there was nothing to move");
    }

    #[test]
    fn looking_before_the_partition_is_mounted_finds_nothing_to_move() {
        // The shape of the bug that hid for two months: on a cold start
        // /metadata is not mounted yet, so its child does not exist and
        // `move_over` answers "nothing to move" without a word. The question
        // is asked inside the mount for exactly this reason.
        let root = folder("unmounted");
        let (old, new) = (root.join("unmounted-metadata").join("skyward"), root.join("keel"));

        move_over(&old, &new).unwrap();

        assert!(!old.exists());
        assert!(!new.exists());
        assert!(!root.join("unmounted-metadata").exists(), "nothing is made on the way there");
    }

    #[test]
    fn a_metadata_with_the_old_folder_has_it_taken_over_once() {
        let root = folder("taken");
        // A mounted /metadata that never carried the old name: nothing to do,
        // and no failure to report.
        assert!(!take_over(&root).unwrap());

        fs::create_dir_all(root.join(OLD_NAME)).unwrap();
        fs::write(root.join(OLD_NAME).join("credential"), b"sealed").unwrap();

        assert!(take_over(&root).unwrap(), "there was something to take over");
        assert_eq!(fs::read(root.join(NEW_NAME).join("credential")).unwrap(), b"sealed");
        assert!(!root.join(OLD_NAME).exists(), "the old folder is gone");
        assert!(!take_over(&root).unwrap(), "a second start finds nothing left to take over");
    }

    #[test]
    fn the_folders_are_the_ones_the_mount_holds() {
        assert_eq!(OLD, format!("{METADATA}/{OLD_NAME}"));
        assert_eq!(NEW, format!("{METADATA}/{NEW_NAME}"));
    }
}
