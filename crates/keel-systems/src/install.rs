//! Installing the other system from its package, switching between the
//! systems on the phone, and taking the other system away again.

use crate::state::{State, Store};
use crate::{ForgottenSystem, Named, Other, Overview};
use anyhow::Context;
use keel_block::area::HEADER;

/// Where the areas' files are, in the data partition.
pub const FOLDER: &str = "/data/skyward/systems";
/// The devices the store and a root are laid out as while they are written.
pub const STORE_DEVICE: &str = "skyward-store";
pub const ROOT_DEVICE: &str = "skyward-root";
/// The gigabyte the pages count sizes in.
const GB: u64 = 1_000_000_000;
/// The sizes a root is offered in, in gigabytes, past what it least takes.
const OFFERED: [u64; 6] = [8, 16, 32, 64, 128, 256];

/// How far an install has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Setting room aside in the data partition: the bytes so far, of all.
    Allocating { done: u64, total: u64 },
    /// Writing the root image, then reading it back.
    Writing { done: u64, total: u64 },
    /// Putting the boot set in the store.
    Finishing,
}

/// The room the data partition has for a system's root, of `free` bytes
/// free: less the store where none is made yet, and less a super's worth,
/// which the installed system's updates keep for themselves, as Android
/// keeps it from its dynamic systems.
pub fn room(free: u64, store: u64, super_size: u64) -> u64 {
    free.saturating_sub(store + super_size + HEADER)
}

/// The sizes offered for a root that takes at least `least` bytes, with
/// `room` bytes to spare: what it least takes in whole gigabytes, and the
/// usual sizes past that, as far as there is room. The area made for one
/// is rounded up to its filesystem's blocks.
pub fn sizes(least: u64, room: u64) -> Vec<u64> {
    let first = least.div_ceil(GB).max(1);
    std::iter::once(first).chain(OFFERED.into_iter().filter(|&s| s > first)).map(|s| s * GB).filter(|&s| s <= room).collect()
}

/// What the pages show of the systems, from the state and the installed
/// system's name; `changed` says whether its vendor_boot or dtbo differ
/// from when the other system was installed.
pub fn overview(state: &State, installed: Named, changed: bool) -> Overview {
    Overview {
        installed,
        other: state.system.as_ref().map(|s| Other { id: s.id.clone(), name: s.name.clone(), version: s.version.clone(), size: s.root.size - HEADER }),
        other_starts: state.occupied.is_some(),
        changed,
        forgotten: state
            .forgotten
            .iter()
            .map(|forgotten| ForgottenSystem {
                id: forgotten.id.clone(),
                name: if forgotten.name.is_empty() { forgotten.id.clone() } else { forgotten.name.clone() },
                size: forgotten.root.size - HEADER,
                why: forgotten.why.clone(),
            })
            .collect(),
    }
}

impl Store {
    /// A store's bytes for the phone's partitions, where the store is not
    /// made yet; none where it is.
    pub fn needed(state: &State, boot: u64, init_boot: u64) -> u64 {
        match state.store {
            Some(_) => 0,
            None => Store::size_for(boot, init_boot),
        }
    }
}

/// Have `control` start the slot that was written over: the installed
/// system's, which is the other one from `occupied`, where the other
/// system's boot set was. Says which slot it started.
///
/// This is the whole of what a switch back over an updated slot does to the
/// phone besides the state — and the one part of it a test cannot ask a real
/// phone for, so the control is whatever is handed in (see
/// [`keel_vab::Starting`]).
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn start_at(control: &dyn keel_vab::Starting, occupied: keel_vab::Slot) -> anyhow::Result<keel_vab::Slot> {
    let installed = crate::guard::start_from(occupied);
    control.set_active(installed)?;
    Ok(installed)
}

/// Start the slot that was written over, and let the state go of the
/// installed system's saved copy and of the other system's record, naming
/// the room it still takes: the two halves of starting a slot as it stands,
/// one after the other, and both of them things a test can ask for — the
/// control is handed in and the state is a file at `path`.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn start_as_it_stands(control: &dyn keel_vab::Starting, occupied: keel_vab::Slot, path: &std::path::Path, why: &str) -> anyhow::Result<(keel_vab::Slot, Option<String>)> {
    let installed = start_at(control, occupied)?;
    let left = crate::guard::let_go_at(path, why)?;
    Ok((installed, left))
}

/// Whether the state kept at `path` names any system a write over its slot
/// left behind. A state that is not there names nothing.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn left_behind_at(path: &std::path::Path) -> bool {
    State::load(path).map(|state| !state.forgotten.is_empty()).unwrap_or(false)
}

/// Take one of the systems a write over its slot left behind out of the
/// state, answering with the room it still takes: the room itself is taken
/// out of the data partition by the caller, which is where an area can be
/// removed at all.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn forget_from(state: &mut State, id: &str) -> anyhow::Result<crate::state::Forgotten> {
    let at = state.forgotten.iter().position(|forgotten| forgotten.id == id).with_context(|| format!("no system written over is named {id:?}"))?;
    Ok(state.forgotten.remove(at))
}

#[cfg(target_os = "android")]
pub use sys::*;

#[cfg(target_os = "android")]
mod sys {
    use super::*;
    use crate::boot::{self, Start};
    use crate::guard;
    use crate::package::{Contents, BOOT, INIT_BOOT, ROOT};
    use anyhow::Result;
    use crate::state::{System, Who, PATH};
    use crate::store::{self, OTHER};
    use crate::switch::{self, Places};
    use anyhow::{ensure, Context};
    use sha2::{Digest, Sha256};
    use keel_block::area::{self, Area};
    use keel_block::{dm, BY_NAME};
    use keel_vab::Slot;
    use std::fs::{self, OpenOptions};
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};
    use zip::ZipArchive;

    const CHUNK: usize = 1 << 20;

    fn by_name() -> &'static Path {
        Path::new(BY_NAME)
    }

    /// Where the data partition is mounted.
    const DATA: &std::ffi::CStr = c"/data";

    fn data() -> &'static Path {
        Path::new(DATA.to_str().expect("the data partition's mount point is written in ASCII"))
    }

    /// The bytes free in the data partition's filesystem.
    pub fn free() -> Result<u64> {
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(DATA.as_ptr(), &mut s) } != 0 {
            return Err(std::io::Error::last_os_error()).context("look at the room in the data partition");
        }
        Ok(s.f_bavail as u64 * s.f_frsize as u64)
    }

    /// The room for a root, in the data partition as it is now.
    pub fn room_now(slot: Slot) -> Result<u64> {
        let state = state()?;
        let size = |name: &str| keel_block::size_bytes(store::partition(by_name(), name, slot));
        let needed = Store::needed(&state, size(store::BOOT)?, size(store::INIT_BOOT)?);
        Ok(room(free()?, needed, keel_block::size_bytes(by_name().join("super"))?))
    }

    /// Lay the store out as a device, once it is found where it was, run
    /// `work` on it, and take the device away again.
    fn with_store<T>(state: &State, work: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
        let store = state.store.as_ref().context("no store holds the boot sets")?;
        if let Err(why) = store.area.check_header() {
            // /data can be cleared while /metadata keeps the record that names
            // the store there: the record then points at bytes that are gone.
            // Nothing can be reached through it, and saying what happened is
            // the only way the screen's "couldn't switch" line can be
            // understood; installing the other system again makes a new store.
            let _ = keel_sys::kmsg(&format!("the store the record names is not in the data partition ({why:#}); the systems it holds are gone with the data partition"));
            return Err(anyhow::anyhow!("the data partition no longer holds the store the systems were installed in ({why:#}); install the other system again to make one"));
        }
        if dm::status(STORE_DEVICE)?.is_some() {
            dm::remove(STORE_DEVICE)?;
        }
        let node = store.area.map(STORE_DEVICE)?;
        let done = work(&node);
        let gone = dm::remove(STORE_DEVICE);
        let done = done?;
        gone?;
        Ok(done)
    }

    const METADATA: &str = "/metadata";

    /// Mount /metadata, where the state is kept, read-only where it is not
    /// mounted yet, as the recovery keeps it.
    pub fn metadata() -> Result<()> {
        let at = Path::new(METADATA);
        if !keel_block::mount::is_mount_point(at)? {
            keel_block::mount::mount_ro(&by_name().join("metadata"), at)?;
        }
        Ok(())
    }

    /// Do `work` on /metadata read-only, mounting it for the moment it takes
    /// where it is not mounted yet, and taking it away again after — the
    /// recovery's own mount is not what fastbootd has to be handed.
    fn read_only<T>(work: impl FnOnce() -> Result<T>) -> Result<T> {
        let at = Path::new(METADATA);
        if keel_block::mount::is_mount_point(at)? {
            return work();
        }
        keel_block::mount::mount_ro(&by_name().join("metadata"), at)?;
        let done = work();
        keel_block::mount::umount(at)?;
        done
    }

    /// Run `work` with /metadata writable, and leave it read-only after.
    fn writable<T>(work: impl FnOnce() -> Result<T>) -> Result<T> {
        metadata()?;
        let at = Path::new(METADATA);
        keel_block::mount::remount_writable(at, true)?;
        let done = work();
        let back = keel_block::mount::remount_writable(at, false);
        let done = done?;
        back?;
        Ok(done)
    }

    /// The state as kept now.
    pub fn state() -> Result<State> {
        metadata()?;
        State::load(Path::new(PATH))
    }

    /// The program the other system starts with: this recovery's own.
    fn program() -> PathBuf {
        Path::new("/").join(keel_config::ramdisk::SYSTEM_INIT)
    }

    /// Have the bootloader start `to`, writing its boot set into the boot
    /// partitions; see [`switch::switch`]. Switching to the other system
    /// waits for any update to be merged, and first checks that the other
    /// system's root is still where it was put.
    pub fn switch_to(next: Slot, to: Who) -> Result<()> {
        let state = state()?;
        if to == Who::Other {
            let update = keel_vab::snapshot_state()?;
            ensure!(update == keel_vab::SnapshotState::None, "an update is in flight ({update:?}), and the installed system has to start once more to merge it");
            state.system.as_ref().context("no other system is installed")?.root.check_header()?;
        }
        if to == Who::Installed {
            let slot = match state.occupied.as_deref() {
                Some(suffix) => Slot::from_suffix(suffix).with_context(|| format!("the other system is in slot {suffix:?}"))?,
                None => next,
            };
            if let Err(why) = guard::written_over(by_name(), &state, slot) {
                // The three things the guard compares are all about the
                // installed system's slot, `slot.other()`: its boot set
                // against what was saved, its vendor_boot and dtbo against
                // what was installed against. So the slot that was written
                // over, and the one to start, is the one the other system is
                // not in; `start_as_it_is` picks it.
                return start_as_it_is(slot, &why);
            }
        }
        let (path, program) = (Path::new(PATH), program());
        writable(|| with_store(&state, |device| switch::switch(&Places { state: path, by_name: by_name(), store: device, program: &program }, next, to)))
    }

    /// Put the installed system's boot set back for the sake of erasing the
    /// data partition or wiping the other system away, where it is still the
    /// boot set the partitions hold.
    ///
    /// Where it is not — an update wrote the installed system's slot while
    /// the other system ran — the saved copy is let go and the bootloader is
    /// pointed at the slot as it stands: what is there is the updated
    /// system's own boot image beside its own system and vendor, which
    /// starts itself. Writing the copy back would start the phone with a
    /// boot image from before the update over a system from after it. The
    /// erase goes on either way, because the data partition being erased is
    /// not something the other system has to start for.
    fn switch_back_for_erase(state: &State, slot: Slot) -> Result<()> {
        if let Err(why) = guard::written_over(by_name(), state, slot) {
            return start_as_it_is(slot, &why);
        }
        let (path, program) = (Path::new(PATH), program());
        writable(|| with_store(state, |device| switch::switch(&Places { state: path, by_name: by_name(), store: device, program: &program }, slot, Who::Installed)))
    }

    /// Have the bootloader start `slot` as it stands, writing no boot set:
    /// what is in its partitions is the system that was updated there, its
    /// own boot image beside its own system and vendor, and that set starts
    /// itself and merges what an update left to merge as it does.
    ///
    /// `slot` is the slot the other system's boot set is in — what
    /// [`guard::written_over`] said was written over — and the slot that
    /// starts is the other one, where the installed system is: starting
    /// `slot` itself would start the other system after its boot set was
    /// let go of, which the state no longer says is there, and in
    /// [`Self::remove`] with the root it is about to delete.
    ///
    /// This is what a switch back does when the saved copy is not the boot
    /// set the partitions hold any more. Writing the copy back — which is
    /// what a switch back does when nothing has been written over — would
    /// start the phone with a boot image from before an update over the
    /// system and vendor from after it, and it would not come up, or it
    /// would come up part way and crash. What was saved is let go instead,
    /// with the other system's record, because nothing holds its boot set
    /// any more; its root stays named in the state as forgotten, so the room
    /// it takes can be taken back and a reinstall of the same package is not
    /// blocked by it. kmsg says so, so a phone that then starts from an
    /// updated slot has an account of why it started as it was, why the
    /// other system has to be installed again, and what is still taking
    /// room.
    fn start_as_it_is(slot: Slot, why: &anyhow::Error) -> Result<()> {
        let (binder, hardware) = (keel_binder::driver::Binder::open()?, keel_binder::driver::Binder::hardware()?);
        let control = keel_vab::boot::BootControl::open(&binder, &hardware)?;
        let said = format!("{why:#}");
        let (installed, left) = match writable(|| start_as_it_stands(&control, slot, Path::new(PATH), &said)) {
            Ok(done) => done,
            Err(e) => {
                // The slot is where the phone starts from now, and the state
                // still says otherwise: the page will show the systems as
                // they were read before this. Saying so is all that can be
                // done from here — switching again repeats it, and the guard
                // still finds the slot written over, so it comes back to this
                // same place rather than writing anything back.
                let _ = keel_sys::kmsg(&format!(
                    "slot {} starts as it stands, but what the state says could not be written ({e:#}); the systems page shows what it said before until the slot is switched again",
                    crate::guard::start_from(slot).letter()
                ));
                return Err(e);
            }
        };
        let _ = keel_sys::kmsg(&format!(
            "the installed system's partitions were written over ({said}); slot {} starts as it is, its boot set let go of rather than written back, and the other system's record goes with it{} - it is to be installed again",
            installed.letter(),
            match &left {
                Some(id) => format!(", its root left in the data partition as {id} until it is installed again or taken away"),
                None => String::new(),
            }
        ));
        Ok(())
    }

    /// Finish a switch cut off before, where one was; says whether there
    /// was one.
    pub fn resume() -> Result<bool> {
        let state = state()?;
        if state.switching.is_none() {
            return Ok(false);
        }
        let (path, program) = (Path::new(PATH), program());
        writable(|| with_store(&state, |device| switch::resume(&Places { state: path, by_name: by_name(), store: device, program: &program })))
    }

    /// Put the installed system's boot set back where the other system's
    /// is, as everything that writes or erases the installed system first
    /// does; says whether it had to.
    ///
    /// Where the saved copy is not what the partitions hold any more — an
    /// update wrote the installed system's slot while the other system ran
    /// — the copy is let go and the bootloader is pointed at the slot as it
    /// stands instead: a boot image from before the update over a system
    /// from after it does not start, or starts part way and crashes, while
    /// the updated slot starts itself. Either way the phone has somewhere
    /// to start from, which refusing to switch would not give it.
    pub fn ensure_installed() -> Result<bool> {
        resume()?;
        let state = state()?;
        let Some(slot) = state.occupied.as_deref().map(|s| Slot::from_suffix(s).with_context(|| format!("the other system is in slot {s:?}"))).transpose()? else {
            return Ok(false);
        };
        switch_to(slot, Who::Installed)?;
        Ok(true)
    }

    /// Whether anything has been written over what the other system was
    /// installed against; see [`guard::changed`].
    pub fn changed(state: &State, current: Slot) -> Result<bool> {
        guard::changed(by_name(), state, current)
    }

    /// Install the system in the package `zip`, whose contents are
    /// `contents`, with a root of `size` bytes: its root written into a
    /// new area of the data partition, which has to be open, and its boot
    /// set, with `program` to start it, put in the store, made where there
    /// is none yet. `slot` is the slot the phone runs from, whose
    /// init_boot image lends its header.
    pub fn install<R: Read + Seek>(zip: &mut ZipArchive<R>, contents: &Contents, size: u64, program: &[u8], slot: Slot, progress: &mut dyn FnMut(Step)) -> Result<()> {
        writable(|| install_now(zip, contents, size, program, slot, progress))
    }

    fn install_now<R: Read + Seek>(zip: &mut ZipArchive<R>, contents: &Contents, size: u64, program: &[u8], slot: Slot, progress: &mut dyn FnMut(Step)) -> Result<()> {
        let path = Path::new(PATH);
        let mut state = State::load(path)?;
        ensure!(state.switching.is_none(), "a switch begun before is not done yet");
        ensure!(state.system.is_none(), "another system is installed already");
        ensure!(size >= contents.least(), "a root of {size} bytes is less than the {} the system takes", contents.least());
        let folder = Path::new(FOLDER);
        let id = contents.manifest.id.clone();
        let file = folder.join(format!("{id}.img"));
        // A system that was written over leaves its root behind, named in the
        // state as forgotten; installing the same package again looks for that
        // very file, and `area::create` will not make one where one is. It is
        // taken away before the room is counted, because it is the room this
        // install is about to take: the bytes it holds are still in the
        // partition, so counting first would answer "there is room for less
        // than this" to a phone where taking those bytes back is all that is
        // needed — and would not even let it try.
        if let Some(at) = state.forgotten.iter().position(|forgotten| forgotten.id == id) {
            let forgotten = state.forgotten.remove(at);
            forgotten.root.remove().context("take away the room the system it replaces was left in")?;
            state.save(path)?;
        }
        let room = room_now(slot)?;
        ensure!(
            size <= room,
            "the data partition has room for {room} bytes, less than {size}{}",
            match state.forgotten.is_empty() {
                true => String::new(),
                false => format!(
                    "; {} system(s) written over still take {} there, and can be taken back on the systems page",
                    state.forgotten.len(),
                    state.forgotten.iter().map(|forgotten| forgotten.root.size).sum::<u64>()
                ),
            }
        );
        fs::create_dir_all(folder).with_context(|| format!("make {}", folder.display()))?;
        if state.store.is_none() {
            let (boot, init_boot) = (keel_block::size_bytes(store::partition(by_name(), store::BOOT, slot))?, keel_block::size_bytes(store::partition(by_name(), store::INIT_BOOT, slot))?);
            let area = area::create(&folder.join("store"), Store::size_for(boot, init_boot), &mut |done, total| progress(Step::Allocating { done, total }))?;
            state.store = Some(Store { area, boot, init_boot });
            state.save(path)?;
        }
        // A file left at the path by anything else is taken away too, rather
        // than failing an install of several gigabytes over it.
        if file.exists() {
            fs::remove_file(&file).with_context(|| format!("take away what is left at {}", file.display()))?;
        }
        let root = area::create(&file, size + HEADER, &mut |done, total| progress(Step::Allocating { done, total }))?;
        let made = (|| -> Result<System> {
            write_root(zip, &root, contents.root, progress)?;
            progress(Step::Finishing);
            let start = Start { id: id.clone(), filesystem: contents.manifest.filesystem.clone(), init: contents.manifest.init.clone(), root: root.clone() };
            let (init_boot, own_ramdisk) = match contents.init_boot {
                true => {
                    let mut own = Vec::new();
                    zip.by_name(INIT_BOOT).context("open init_boot.img")?.read_to_end(&mut own).context("read init_boot.img")?;
                    let (image, own_ramdisk) = boot::with_start(&own, program, &start)?;
                    (image, Some(own_ramdisk))
                }
                false => (boot::init_boot(&fs::read(store::partition(by_name(), store::INIT_BOOT, slot)).context("read the phone's init_boot")?, program, &start)?, None),
            };
            let set = with_store(&state, |device| {
                let mut image = zip.by_name(BOOT).context("open boot.img")?;
                let store = state.store.as_ref().expect("made above");
                store::put(store, device, OTHER, &mut image, contents.boot, &init_boot)
            })?;
            Ok(System {
                id: id.clone(),
                name: contents.manifest.name.clone(),
                version: contents.manifest.version.clone(),
                filesystem: contents.manifest.filesystem.clone(),
                init: contents.manifest.init.clone(),
                root: root.clone(),
                set,
                own_ramdisk,
                vendor_boot: guard::sum_of(&store::partition(by_name(), "vendor_boot", slot))?,
                dtbo: guard::sum_of(&store::partition(by_name(), "dtbo", slot))?,
            })
        })();
        match made {
            Ok(system) => {
                state.system = Some(system);
                state.save(path)
            }
            Err(e) => match root.remove() {
                Ok(()) => Err(e),
                Err(r) => Err(e.context(format!("taking the root away afterwards failed too: {r:#}"))),
            },
        }
    }

    /// Write the root image from the package into the root area, after its
    /// header, then read it back.
    fn write_root<R: Read + Seek>(zip: &mut ZipArchive<R>, root: &Area, len: u64, progress: &mut dyn FnMut(Step)) -> Result<()> {
        ensure!(len <= root.size - HEADER, "the root image takes {len} bytes, more than its area's {}", root.size - HEADER);
        if dm::status(ROOT_DEVICE)?.is_some() {
            dm::remove(ROOT_DEVICE)?;
        }
        let node = root.map(ROOT_DEVICE)?;
        let written = (|| -> Result<()> {
            let mut out = OpenOptions::new().read(true).write(true).open(&node).with_context(|| format!("open {}", node.display()))?;
            let mut image = zip.by_name(ROOT).context("open rootfs.img")?;
            out.seek(SeekFrom::Start(HEADER))?;
            let mut buf = vec![0u8; CHUNK];
            let (mut sum, mut done) = (Sha256::new(), 0);
            while done < len {
                let n = (len - done).min(CHUNK as u64) as usize;
                image.read_exact(&mut buf[..n]).context("read rootfs.img")?;
                out.write_all(&buf[..n]).context("write the root")?;
                sum.update(&buf[..n]);
                done += n as u64;
                progress(Step::Writing { done, total: len * 2 });
            }
            out.sync_all().context("sync the root")?;
            keel_block::drop_cache(&out)?;
            out.seek(SeekFrom::Start(HEADER))?;
            let (mut back, mut read) = (Sha256::new(), 0);
            while read < len {
                let n = (len - read).min(CHUNK as u64) as usize;
                out.read_exact(&mut buf[..n]).context("read the root back")?;
                back.update(&buf[..n]);
                read += n as u64;
                progress(Step::Writing { done: len + read, total: len * 2 });
            }
            ensure!(back.finalize() == sum.finalize(), "the root reads back different from what was written");
            Ok(())
        })();
        let gone = dm::remove(ROOT_DEVICE);
        written?;
        gone
    }

    /// Take the other system away: its boot set out of the partitions
    /// where it is there, its root and the store out of the data partition,
    /// which has to be open, and the state with them.
    ///
    /// The boot set goes back whether or not the installed system's
    /// partitions are still what they were: the other system is being taken
    /// away, so nothing is left to protect, and refusing here would leave a
    /// phone holding a boot set for a system that is about to be deleted.
    /// What was written over is said in kmsg, so a phone that starts part
    /// way afterwards has an account of why.
    pub fn remove() -> Result<()> {
        ensure!(keel_block::mount::is_mount_point(data())?, "the data partition is not open");
        resume()?;
        let state = state()?;
        if let Some(suffix) = state.occupied.as_deref() {
            let slot = Slot::from_suffix(suffix).with_context(|| format!("the other system is in slot {suffix:?}"))?;
            switch_back_for_erase(&state, slot)?;
        }
        // Every area the state names is deleted from the state read after the
        // switch back, not from the one before it: that switch, where the
        // partitions were written over, lets go of the other system's record
        // and moves its root onto `forgotten` as it saves. Reading it again
        // here means the root that was just moved is deleted from the same
        // list that names it, rather than being guessed at from a copy taken
        // before.
        writable(|| {
            let path = Path::new(PATH);
            let mut state = State::load(path)?;
            let mut areas: Vec<Area> = state.system.iter().map(|system| system.root.clone()).collect();
            areas.extend(state.forgotten.iter().map(|forgotten| forgotten.root.clone()));
            for area in &areas {
                area.remove()?;
            }
            if state.system.is_some() || state.occupied.is_some() || state.saved.is_some() || !state.forgotten.is_empty() {
                state.system = None;
                state.occupied = None;
                state.saved = None;
                state.forgotten.clear();
                state.save(path)?;
            }
            if let Some(store) = &state.store {
                store.area.remove()?;
                state = State::default();
                state.save(path)?;
            }
            let folder = PathBuf::from(FOLDER);
            if folder.exists() && fs::read_dir(&folder)?.next().is_none() {
                fs::remove_dir(&folder).with_context(|| format!("delete {}", folder.display()))?;
            }
            Ok(())
        })
    }

    /// Whether the state names any system a write over its slot left behind.
    /// A phone can stop holding both slots' boot sets — the recovery and the
    /// system partitions change under it — while the record still names roots
    /// those writes left in the data partition; the systems page is the only
    /// place that can give that room back, so it is offered for this alone.
    ///
    /// The record is read with /metadata mounted read-only for the moment it
    /// takes and taken away again: a cold start has not mounted it yet, and
    /// fastbootd has to be handed it as it was found. A state that cannot be
    /// read is one that names nothing.
    pub fn left_behind() -> bool {
        read_only(|| Ok(left_behind_at(Path::new(PATH)))).unwrap_or(false)
    }

    /// Take away one of the systems a write over its slot left behind: its
    /// root out of the data partition, which has to be open, and its entry
    /// out of the state. What is installed and what is left of the rest stay
    /// as they are — this is the page's "take it back", not a reinstall and
    /// not a removal of the other system.
    pub fn forget(id: &str) -> Result<()> {
        ensure!(keel_block::mount::is_mount_point(data())?, "the data partition is not open");
        resume()?;
        writable(|| {
            let path = Path::new(PATH);
            let mut state = State::load(path)?;
            let forgotten = forget_from(&mut state, id)?;
            forgotten.root.remove().with_context(|| format!("take away the room {id:?} still takes"))?;
            state.save(path)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_room_for_a_root_keeps_a_supers_worth_for_updates() {
        assert_eq!(room(59 * GB, 600 << 20, 9 * GB), 59 * GB - (600 << 20) - 9 * GB - HEADER);
        assert_eq!(room(GB, 0, 9 * GB), 0, "none to spare");
    }

    #[test]
    fn the_sizes_offered_start_at_what_the_root_takes_and_stay_within_room() {
        assert_eq!(sizes(3 << 30, 50 * GB), [4 * GB, 8 * GB, 16 * GB, 32 * GB], "3 GiB is past 3 GB");
        assert_eq!(sizes(100 << 20, 9 * GB), [GB, 8 * GB]);
        assert_eq!(sizes(20 * GB, 300 * GB), [20 * GB, 32 * GB, 64 * GB, 128 * GB, 256 * GB]);
        assert!(sizes(20 * GB, 10 * GB).is_empty(), "no room for it at all");
    }

    #[test]
    fn a_state_that_names_a_system_left_behind_is_one_whose_room_can_be_asked_for() {
        // A cold start has nothing where the state is kept; writing one there
        // is what installing the other system and a write over its slot do.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skyward/systems.toml");
        assert!(!left_behind_at(&path), "nothing kept, nothing left behind");
        let (_at, _path, mut state) = crate::guard::tests::phone(b"old boot", b"other system boot");
        crate::guard::let_go(&mut state, "an update wrote over it");
        state.save(&path).unwrap();
        assert!(left_behind_at(&path), "the room the root still takes is named, so the page can be offered for it");
    }

    #[test]
    fn taking_back_one_room_removes_that_entry_only_and_leaves_the_others() {
        // Two systems a write over their slot left behind, taken back one at
        // a time: the record is what a test on the host can hold, so the
        // areas are the ones `phone` invents and no file is there to lose.
        let (_at, _path, mut state) = crate::guard::tests::phone(b"old boot", b"other system boot");
        let earlier = state.system.as_ref().unwrap().root.clone();
        state.forgotten.push(crate::state::Forgotten { id: "earlier".into(), name: "Earlier".into(), root: earlier.clone(), why: "an update wrote over it".into() });
        let id = state.system.as_ref().unwrap().id.clone();
        crate::guard::let_go(&mut state, "an update wrote over it");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("systems.toml");
        state.save(&path).unwrap();
        let mut saved = State::load(&path).unwrap();
        forget_from(&mut saved, &id).unwrap();
        saved.save(&path).unwrap();
        let after = State::load(&path).unwrap();
        assert_eq!(after.forgotten.len(), 1, "the one taken back is gone from the record");
        assert_eq!(after.forgotten[0].id, "earlier", "and the one let go of before is still named");
        assert_eq!(after.forgotten[0].root, earlier, "with the room it was left in still its own");
    }

    #[test]
    fn taking_back_a_room_the_state_does_not_name_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("systems.toml");
        crate::state::State::default().save(&path).unwrap();
        let mut state = State::load(&path).unwrap();
        let why = forget_from(&mut state, "gone").unwrap_err();
        assert!(format!("{why:#}").contains("no system written over is named"), "the page's line can be understood: {why:#}");
        assert!(state.forgotten.is_empty(), "and nothing was taken out of the record");
    }

    /// A boot control that remembers what it was told to start.
    struct Recorder(std::sync::Mutex<Vec<keel_vab::Slot>>);

    impl keel_vab::Starting for Recorder {
        fn active(&self) -> anyhow::Result<keel_vab::Slot> {
            Ok(keel_vab::Slot::A)
        }

        fn slots(&self) -> anyhow::Result<keel_vab::BootSlots> {
            Ok(keel_vab::BootSlots { active: keel_vab::Slot::A, bootable: [true, true], successful: [true, true] })
        }

        fn set_active(&self, slot: keel_vab::Slot) -> anyhow::Result<()> {
            self.0.lock().unwrap().push(slot);
            Ok(())
        }
    }

    #[test]
    fn a_slot_started_over_an_updated_one_is_the_installed_systems_not_the_other_ones() {
        let control = Recorder(Default::default());
        assert_eq!(start_at(&control, keel_vab::Slot::B).unwrap(), keel_vab::Slot::A, "the other system is in B, so A starts");
        assert_eq!(*control.0.lock().unwrap(), [keel_vab::Slot::A], "and the boot control is told so, not B");
    }

    #[test]
    fn once_the_other_systems_slot_is_let_go_of_there_is_nothing_left_to_switch_to() {
        let (_dir, _at, state) = crate::guard::tests::phone(b"old boot", b"other system boot");
        let mut gone = state;
        crate::guard::let_go(&mut gone, "an update wrote over it");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("systems.toml");
        gone.save(&path).unwrap();
        let places = crate::switch::Places { state: &path, by_name: dir.path(), store: dir.path(), program: dir.path() };
        let why = crate::switch::switch(&places, keel_vab::Slot::A, crate::state::Who::Other).unwrap_err();
        assert!(format!("{why:#}").contains("no other system is installed"), "the trap that ends in the other system starting every time is shut: {why:#}");
    }

    #[test]
    fn a_slot_started_as_it_stands_starts_the_installed_one_and_saves_the_rest_as_let_go_of() {
        // The two halves of `start_as_it_is` a test can ask for: the control
        // is a recorder, and the state is a file in a temporary directory.
        let (_dir, _at, state) = crate::guard::tests::phone(b"old boot", b"other system boot");
        let id = state.system.as_ref().unwrap().id.clone();
        let root = state.system.as_ref().unwrap().root.clone();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("systems.toml");
        state.save(&path).unwrap();
        let control = Recorder(Default::default());
        let (started, left) = start_as_it_stands(&control, keel_vab::Slot::B, &path, "an update wrote over it").unwrap();
        assert_eq!(started, keel_vab::Slot::A, "the other system is in B, so the updated installed system in A starts");
        assert_eq!(*control.0.lock().unwrap(), [keel_vab::Slot::A], "the boot control was told A, never B");
        assert_eq!(left.as_deref(), Some(id.as_str()), "what was let go of is said, so kmsg can name the room it still takes");
        let after = crate::state::State::load(&path).unwrap();
        assert!(after.occupied.is_none() && after.saved.is_none() && after.system.is_none(), "the state says nothing of the other system starts any more");
        assert_eq!(after.forgotten.into_iter().map(|forgotten| forgotten.root).collect::<Vec<_>>(), [root], "the room it takes is still named");
    }
}
