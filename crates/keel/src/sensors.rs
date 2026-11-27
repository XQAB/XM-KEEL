//! The sensor service on the ADSP, brought up by the recovery itself.
//!
//! Recovery starts no Android framework and no sensor HAL, so nothing brings
//! the sensors' protection domain up for it, and the pages need it for the
//! hinge. The work was measured on bixi, and these are the six steps that
//! make it answer:
//!
//! 1. the DSP partition mounted read-only at `/vendor/dsp`,
//! 2. the persist tree bound at `/persist`, where the registry lives,
//! 3. the fastrpc module loaded,
//! 4. the node it publishes, made with the minor the kernel actually gave,
//! 5. the linker namespace written, which the daemons start under,
//! 6. `pd-mapper`, then `adsrpcd` (where the phone has it), then `sscrpcd`.
//!
//! With the mounts alone the ADSP answers with 31 uids and no hinge; once the
//! daemons are up the registry fills in within a couple of seconds and the
//! hinge reports. Nothing here restarts the ADSP: the `/sys/kernel/boot_adsp`
//! node is the audio subsystem's, and writing it gains nothing that the
//! daemons do not already do.
//!
//! Every step is best effort. A phone whose init already did the work finds
//! the service standing and returns at once; anywhere one step fails, the
//! reason goes to the kernel log and the pages still come up.

use anyhow::{bail, Context, Result};
use keel_block::mount;
use std::fs;
use std::io::ErrorKind;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Where a phone names its partitions, in the order they are looked for.
const BY_NAME: [&str; 2] = ["/dev/block/by-name", "/dev/block/bootdevice/by-name"];
/// The partition holding the sensor firmware, the running slot's first.
const DSP: [&str; 2] = ["dsp_a", "dsp_b"];
/// Where that partition is mounted for the daemons to read.
const DSP_MOUNT: &str = "/vendor/dsp";
/// The persist tree, as init binds it: the registry the ADSP reads lives here.
const PERSIST: (&str, &str) = ("/mnt/vendor/persist", "/persist");
/// The module that carries fastrpc to the ADSP, and the node it publishes.
const MODULE: &str = "/vendor/lib/modules/frpc-adsprpc.ko";
const SECURE_NODE: &str = "/dev/fastrpc-adsp-secure";
/// The kernel's misc major, which the module's minor joins. The minor is the
/// kernel's to give and changes from boot to boot: read, never assumed.
const SECURE_MAJOR: u32 = 10;
/// The linker namespace the daemons are started under.
const LINKER: &str = "/linkerconfig/ld.config.txt";
/// Who may use the node: every process that attaches to the ADSP, as root.
const NODE_MODE: u32 = 0o666;

/// The linker namespace the daemons need on bixi, kept here rather than
/// assembled at run time: it was measured on the phone and is the same every
/// boot. Its `[vendor]` namespace searches the system libraries through
/// `/mnt/system`, which is where recovery keeps them.
const LINKER_CONFIG: &str = include_str!("sensors/ld.config.txt");

/// One daemon of the chain, and how long to let it settle before the next is
/// started. The one in the middle is on the phones that have it; bixi answers
/// with the registry without it, and a phone without it is not in trouble.
struct Daemon {
    program: &'static str,
    args: &'static [&'static str],
    settle: u64,
    needed: bool,
}

const DAEMONS: [Daemon; 3] = [
    Daemon { program: "/vendor/bin/pd-mapper", args: &[], settle: 1, needed: true },
    Daemon { program: "/vendor/bin/adsrpcd", args: &["rootpd", "adsp"], settle: 2, needed: false },
    Daemon { program: "/vendor/bin/sscrpcd", args: &["sensorspd", "adsp"], settle: 0, needed: true },
];

/// How long the registry is given to fill in after the daemons are up, in
/// steps of [`PROBE`]: measured as a couple of seconds, and long enough not
/// to hold the pages up when it never comes.
const READY: Duration = Duration::from_secs(10);
const PROBE: Duration = Duration::from_secs(2);

/// Bring the sensor service up, for the hinge to follow.
///
/// Called once, after the panel is up and before the hinge is followed. It
/// does nothing when the service already answers, which is the ordinary case
/// of a phone whose init started it.
pub fn bring_up() {
    let _ = attempt();
}

fn attempt() -> Result<()> {
    // A phone whose sensors are not served through this daemon chain is left
    // alone: nothing here would be able to help it.
    if !Path::new(DAEMONS[2].program).exists() {
        return Ok(());
    }
    if hinge_answers() {
        let _ = keel_sys::kmsg("sensors: the sensor service already answers");
        return Ok(());
    }
    let _ = keel_sys::kmsg("sensors: bringing the sensor service up");
    let mut trouble: Vec<String> = Vec::new();
    let landed = |what: &str, done: Result<()>, trouble: &mut Vec<String>| {
        if let Err(e) = done {
            let _ = keel_sys::kmsg(&format!("sensors: {what}: {e:#}"));
            trouble.push(what.to_owned());
        }
    };
    landed("the DSP partition", mount_dsp(), &mut trouble);
    landed("the persist tree", bind_persist(), &mut trouble);
    landed("the fastrpc module", load_module(), &mut trouble);
    landed("the fastrpc node", secure_node(), &mut trouble);
    landed("the linker namespace", write_linker(), &mut trouble);
    for daemon in &DAEMONS {
        landed(daemon.program, start_daemon(daemon), &mut trouble);
    }
    let answered = wait_until_answering();
    let _ = keel_sys::kmsg(if answered {
        "sensors: the sensor service answers"
    } else {
        "sensors: the sensor service does not answer yet; the hinge will keep asking"
    });
    if trouble.is_empty() {
        Ok(())
    } else {
        bail!("{} step(s) did not land: {}", trouble.len(), trouble.join(", "))
    }
}

/// Whether the hinge is in the registry the ADSP is answering with.
fn hinge_answers() -> bool {
    match keel_sensors::Client::open() {
        Ok(mut client) => client.named("hinge_angle").map(|found| !found.is_empty()).unwrap_or(false),
        Err(_) => false,
    }
}

/// The registry is given a few seconds; the pages are not held up longer
/// than that, and the hinge retries on its own after.
fn wait_until_answering() -> bool {
    let deadline = Instant::now() + READY;
    loop {
        if hinge_answers() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(PROBE);
    }
}

/// Mount the partition the sensor firmware is on, read-only, where the
/// daemons look for it.
fn mount_dsp() -> Result<()> {
    let at = Path::new(DSP_MOUNT);
    if mount::is_mount_point(at)? {
        return Ok(());
    }
    fs::create_dir_all(at).with_context(|| format!("make {}", at.display()))?;
    let mut refused = Vec::new();
    for base in BY_NAME {
        for name in DSP {
            let device = PathBuf::from(base).join(name);
            if !device.exists() {
                continue;
            }
            match mount::mount_ro(&device, at) {
                Ok(fs) => {
                    let _ = keel_sys::kmsg(&format!("sensors: {} mounted as {fs}", device.display()));
                    return Ok(());
                }
                Err(e) => refused.push(format!("{}: {e:#}", device.display())),
            }
        }
    }
    bail!("no DSP partition could be mounted ({})", refused.join("; "))
}

/// Bind the persist tree at `/persist`, where the daemons expect it. The
/// registry the ADSP fills in lives under it.
fn bind_persist() -> Result<()> {
    let (from, to) = (Path::new(PERSIST.0), Path::new(PERSIST.1));
    if mount::is_mount_point(to)? {
        return Ok(());
    }
    if !from.exists() {
        bail!("{} is not mounted", from.display());
    }
    fs::create_dir_all(to).with_context(|| format!("make {}", to.display()))?;
    mount::bind(from, to).with_context(|| format!("bind {} at {}", from.display(), to.display()))
}

/// Load the module that carries fastrpc to the ADSP, when it is not loaded.
fn load_module() -> Result<()> {
    if loaded("frpc_adsprpc") {
        return Ok(());
    }
    let module = Path::new(MODULE);
    if !module.exists() {
        bail!("{MODULE} is not there");
    }
    let file = fs::File::open(module).with_context(|| format!("open {MODULE}"))?;
    let params = c"";
    let answer = unsafe { libc::syscall(libc::SYS_finit_module, file.as_raw_fd(), params.as_ptr(), 0_u32) };
    if answer < 0 {
        let error = std::io::Error::last_os_error();
        // A module the kernel already holds under another name is no trouble.
        if error.raw_os_error() == Some(libc::EEXIST) {
            return Ok(());
        }
        bail!("finit_module {MODULE}: {error}");
    }
    let _ = keel_sys::kmsg("sensors: frpc-adsprpc is loaded");
    Ok(())
}

/// Whether a module is loaded, as `/proc/modules` names it.
fn loaded(name: &str) -> bool {
    fs::read_to_string("/proc/modules")
        .map(|text| text.lines().any(|line| line.split_whitespace().next() == Some(name)))
        .unwrap_or(false)
}

/// Make the node the module publishes, when the kernel has not made it. Its
/// minor is the kernel's own and differs between boots; it is read from
/// `/proc/misc` every time.
fn secure_node() -> Result<()> {
    let node = Path::new(SECURE_NODE);
    if node.exists() {
        let _ = fs::set_permissions(node, fs::Permissions::from_mode(NODE_MODE));
        return Ok(());
    }
    let text = fs::read_to_string("/proc/misc").context("read /proc/misc")?;
    let minor = text
        .lines()
        .find_map(|line| {
            let mut words = line.split_whitespace();
            let minor: u32 = words.next()?.parse().ok()?;
            (words.next()? == "fastrpc-adsp-secure").then_some(minor)
        })
        .context("fastrpc-adsp-secure is not in /proc/misc")?;
    let path = std::ffi::CString::new(SECURE_NODE).expect("the node's path holds no NUL");
    let number = device_number(SECURE_MAJOR, minor);
    let answer = unsafe { libc::mknod(path.as_ptr(), libc::S_IFCHR | NODE_MODE, number) };
    if answer < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == ErrorKind::AlreadyExists {
            return Ok(());
        }
        bail!("mknod {SECURE_NODE} {SECURE_MAJOR}:{minor}: {error}");
    }
    let _ = fs::set_permissions(node, fs::Permissions::from_mode(NODE_MODE));
    let _ = keel_sys::kmsg(&format!("sensors: {SECURE_NODE} made, minor {minor}"));
    Ok(())
}

/// The kernel's own numbering of a character device, as `mknod` takes it
/// (linux/dev_t: major in bits 8-19 and 32-43, minor in 0-7 and 20-31).
fn device_number(major: u32, minor: u32) -> u64 {
    let major = u64::from(major);
    let minor = u64::from(minor);
    (major & 0xfff) << 8 | (minor & 0xff) | (minor & !0xff) << 12
}

/// Write the linker namespace out, when what is there is not already this.
fn write_linker() -> Result<()> {
    let at = Path::new(LINKER);
    if fs::read_to_string(at).is_ok_and(|text| text == LINKER_CONFIG) {
        return Ok(());
    }
    if let Some(parent) = at.parent() {
        fs::create_dir_all(parent).with_context(|| format!("make {}", parent.display()))?;
    }
    fs::write(at, LINKER_CONFIG).with_context(|| format!("write {LINKER}"))?;
    let _ = keel_sys::kmsg("sensors: the linker namespace is written");
    Ok(())
}

/// Start a daemon, unless it is running, and let it settle. A daemon the
/// phone does not have is skipped, and only one that is needed but missing or
/// that will not start counts as trouble.
fn start_daemon(daemon: &Daemon) -> Result<()> {
    let name = name_of(daemon.program);
    if running(name) {
        return Ok(());
    }
    if !Path::new(daemon.program).exists() {
        if daemon.needed {
            bail!("{} is not there", daemon.program);
        }
        return Ok(());
    }
    // In a session of its own: a recovery that ends does not take the daemon
    // with it, and neither does the cable being pulled.
    let mut command = Command::new(daemon.program);
    command.args(daemon.args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let child = command.spawn().with_context(|| format!("start {} {}", daemon.program, daemon.args.join(" ")))?;
    let _ = keel_sys::kmsg(&format!("sensors: {} started as {}", name, child.id()));
    if daemon.settle > 0 {
        std::thread::sleep(Duration::from_secs(daemon.settle));
    }
    Ok(())
}

/// The name a program is known by in `/proc`, which is its file name.
fn name_of(program: &str) -> &str {
    Path::new(program).file_name().and_then(|n| n.to_str()).unwrap_or(program)
}

/// Whether a program is already running, by the name its command line starts
/// with.
fn running(name: &str) -> bool {
    fs::read_dir("/proc").is_ok_and(|entries| {
        entries
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().chars().all(|c| c.is_ascii_digit()))
            .any(|entry| {
                let Ok(line) = fs::read_to_string(entry.path().join("cmdline")) else { return false };
                let first = line.split('\0').next().unwrap_or("");
                Path::new(first).file_name().and_then(|n| n.to_str()) == Some(name)
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_node_is_numbered_the_kernels_way() {
        // 10:282 is the node the module publishes on bixi.
        assert_eq!(device_number(10, 282), 1_051_162);
        // And the way it reads back: major in bits 8-19, minor in 0-7 and 20-31.
        let number = device_number(10, 282);
        assert_eq!((number >> 8) & 0xfff, 10);
        assert_eq!((number & 0xff) | ((number >> 12) & !0xff), 282);
    }

    #[test]
    fn the_linker_namespace_is_the_measured_one() {
        // 545 bytes, the one the daemons were started under on the phone, and
        // the one `/linkerconfig/ld.config.txt` holds there now.
        assert_eq!(LINKER_CONFIG.len(), 545);
        assert!(LINKER_CONFIG.starts_with("dir.recovery = /system/bin"));
        assert!(LINKER_CONFIG.contains("[vendor]"));
        assert!(LINKER_CONFIG.contains("/mnt/system/system/lib64/bootstrap"));
        assert!(LINKER_CONFIG.contains("/odm/${LIB}"));
        assert!(LINKER_CONFIG.ends_with('\n'), "the file ends with a newline, as the phone's does");
    }
}
