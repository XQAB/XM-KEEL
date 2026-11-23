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

/// Move the old folder to the new name. /metadata is writable only while the
/// files are moved, as everywhere else it is written; it is mounted for the
/// moment where it is not mounted yet, as in fastboot.
#[cfg(target_os = "android")]
pub fn metadata() -> Result<()> {
    let (old, new) = (Path::new(OLD), Path::new(NEW));
    if !old.exists() {
        return Ok(());
    }
    crate::settings::with_metadata(|| {
        keel_vab::metadata_writable(true)?;
        let moved = move_over(old, new);
        let back = keel_vab::metadata_writable(false);
        moved?;
        back
    })
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
}
