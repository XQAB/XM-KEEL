//! The browser's full mode: the phone's owner puts a Chromium on it, and
//! XM KEEL asks that Chromium to draw a page and hand back what it came to.
//!
//! Nothing here is needed for the browser to work. The plugin is a root
//! filesystem on the owner's storage with `usr/lib/chromium/chromium` in it;
//! when it is there, a page is read as the page a browser draws, scripts and
//! all, and the DOM it hands back is read the same way an answer's own HTML
//! is. When it is not there, or when it cannot start, the address is fetched
//! the plain way and the page is the text its HTML holds.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// Where the plugin is looked for, in the order the owner is likelier to
/// have put it. The first is the plugin's own README's place; the second is
/// the same directory seen through the running system's storage.
pub const PLACES: [&str; 3] = [
    // A real filesystem, where a file can be made executable: the plugin is
    // run out of a chroot, and /data/media is FUSE, which keeps the modes it
    // is given to itself and often says no to running what it holds.
    "/data/local/Skyward/plugins/chromium",
    // Where the phone's own file manager can put one by hand.
    "/data/media/0/Skyward/plugins/chromium",
    "/sdcard/Skyward/plugins/chromium",
];

/// The one file that has to be there for the plugin to be a plugin.
pub const CHROMIUM: &str = "usr/lib/chromium/chromium";

/// How long a page is given to come back. A recovery has nothing to do with
/// a browser that will not answer, and the owner is holding the phone.
const BUDGET: Duration = Duration::from_secs(30);

/// The flags a drawn page is read with: no sandbox (a recovery is already
/// root and has no user namespaces), no GPU (there is no display), the
/// profile and the scratch in the plugin's own `/tmp`, and the DOM on
/// stdout once the page's own scripts have had their turn.
const FLAGS: [&str; 17] = [
    "--headless=new",
    "--no-sandbox",
    "--disable-gpu",
    "--disable-dev-shm-usage",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-crash-reporter",
    "--disable-breakpad",
    "--ignore-certificate-errors",
    "--disable-quic",
    "--disable-dns-prefetch",
    "--disable-features=AsyncDns",
    "--hide-scrollbars",
    "--virtual-time-budget=5000",
    "--dump-dom",
];

/// The plugin, if the owner has put one on the phone.
pub fn plugin() -> Option<PathBuf> {
    PLACES.iter().map(Path::new).find(|place| place.join(CHROMIUM).is_file()).map(Path::to_path_buf)
}

/// The plugin `root` is on this phone and can draw a page: the two shelves
/// it writes in are there, and the three places of the running system it
/// has to see are bound into it. A place bound by an earlier page is left
/// bound: binding it twice fails, and so would every page after.
pub fn ready(root: &Path) -> Result<()> {
    for made in ["tmp", "tmp/given", "tmp/xdg", "tmp/cache", "tmp/config"] {
        std::fs::create_dir_all(root.join(made)).with_context(|| format!("make the plugin's {made}"))?;
    }
    for bound in ["dev", "proc", "sys"] {
        let at = root.join(bound);
        std::fs::create_dir_all(&at).with_context(|| format!("make the plugin's {bound}"))?;
        if is_mount(&at) {
            continue;
        }
        let from = format!("/{bound}");
        if unsafe { bind(&from, &at.to_string_lossy()) } != 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| format!("bind {from} into the plugin"));
        }
    }
    // Answers have to be looked up the way the recovery looks them up: the
    // plugin's own resolver file is what its Chromium reads. The recovery's
    // is written into it when the plugin has none of its own.
    let resolv = root.join("etc/resolv.conf");
    if let Ok(host) = std::fs::read_to_string("/etc/resolv.conf") {
        let mine = std::fs::read_to_string(&resolv).unwrap_or_default();
        if !host.trim().is_empty() && mine.trim().is_empty() {
            let _ = std::fs::write(&resolv, host);
        }
    }
    Ok(())
}

/// Whether `at` already has something mounted on it, as the kernel's own
/// mount table says.
///
/// The table names a mount by the path the kernel had for it when it was
/// made, and the plugin's place on the data partition is not always named
/// the same way: a shelf under `/data/media/0` is written down under
/// `/mnt/pass_through/0`, so comparing the two names as text misses every
/// bind an earlier page made. Then every page binds again, and each page
/// leaves three more mounts of /dev, /proc and /sys behind — ten and more,
/// on a phone that had opened the plugin ten times. The place is compared
/// by what is there instead: the device and the inode a mount lands on
/// are the same however the path to it is spelled.
fn is_mount(at: &Path) -> bool {
    let Ok(table) = std::fs::read_to_string("/proc/self/mountinfo") else { return false };
    let Some(mine) = identity(at) else { return false };
    table.lines().any(|line| {
        let Some(target) = line.split_whitespace().nth(4) else { return false };
        identity(Path::new(target)) == Some(mine)
    })
}

/// What tells a place apart from another place that has the same path spelled
/// differently: the device it is on and its inode there.
fn identity(at: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(at).ok()?;
    Some((meta.dev(), meta.ino()))
}

/// Bind `from` onto `at` the way the plugin's own `run.sh` would with
/// `mount`: the kernel call, so the recovery does not need a `mount` of its
/// own to reach it.
unsafe fn bind(from: &str, at: &str) -> i32 {
    let source = std::ffi::CString::new(from).expect("a path without a NUL");
    let target = std::ffi::CString::new(at).expect("a path without a NUL");
    unsafe { libc::mount(source.as_ptr(), target.as_ptr(), std::ptr::null(), libc::MS_BIND | libc::MS_REC, std::ptr::null()) }
}

/// The DOM `url` comes to with the plugin at `root` drawing it, the page
/// given `BUDGET` and then hung up, its whole process group with it: a page
/// that will not load must not hold the browser's own loop.
pub fn dom(root: &Path, url: &str) -> Result<String> {
    ready(root)?;
    // What it says when it will not draw goes to a file in the plugin's own
    // tmp, opened here so the descriptor survives the chroot: a browser that
    // quits over a missing library or a profile it cannot make leaves that
    // as the only account of why.
    let said = root.join("tmp/plugin.err");
    let said_file = std::fs::File::create(&said).with_context(|| format!("make {}", said.display()))?;
    let mut command = Command::new("/usr/lib/chromium/chromium");
    command
        .args(FLAGS)
        // Inside the chroot, where `ready` made the shelf: a path of the
        // recovery's own does not exist in there.
        .arg("--user-data-dir=/tmp/given")
        .arg(url)
        .env_clear()
        .env("PATH", "/bin:/usr/bin:/usr/sbin")
        .env("HOME", "/root")
        .env("TMPDIR", "/tmp")
        .env("XDG_RUNTIME_DIR", "/tmp/xdg")
        .env("XDG_CACHE_HOME", "/tmp/cache")
        .env("XDG_CONFIG_HOME", "/tmp/config")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(said_file));
    let root = root.to_path_buf();
    unsafe {
        command.pre_exec(move || {
            let dir = std::ffi::CString::new(root.to_string_lossy().as_bytes()).expect("a path without a NUL");
            if libc::chroot(dir.as_ptr()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::chdir(c"/".as_ptr()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // Its own session, so hanging it up takes what it started too,
            // and nothing of it outlives the page it was asked for.
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().with_context(|| format!("start the plugin to draw {url}"))?;
    let pid = child.id() as i32;
    let done = Arc::new(AtomicBool::new(false));
    let hung = Arc::new(AtomicBool::new(false));
    {
        let (done, hung) = (done.clone(), hung.clone());
        std::thread::Builder::new().name("browser-full-cut".into()).spawn(move || {
            let started = Instant::now();
            while !done.load(Ordering::Relaxed) && started.elapsed() < BUDGET {
                std::thread::sleep(Duration::from_millis(100));
            }
            if !done.load(Ordering::Relaxed) {
                hung.store(true, Ordering::Relaxed);
                // The whole group: Chromium starts a few processes of its own.
                unsafe { libc::kill(-pid, libc::SIGKILL) };
            }
        })?;
    }
    let mut out = String::new();
    let read = child
        .stdout
        .take()
        .context("the plugin's answer")?
        .read_to_string(&mut out)
        .with_context(|| format!("read what the plugin drew for {url}"));
    done.store(true, Ordering::Relaxed);
    let status = child.wait().with_context(|| format!("wait for the plugin drawing {url}"));
    if hung.load(Ordering::Relaxed) {
        bail!("the plugin did not answer within {} seconds, so it was hung up", BUDGET.as_secs());
    }
    read?;
    let status = status?;
    // What it wrote down, in the words a person can act on: the last lines
    // it said before it quit, or before it was hung up.
    let excuse = || {
        let text = std::fs::read_to_string(&said).unwrap_or_default();
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        let from = lines.len().saturating_sub(3);
        lines[from..].join(" / ")
    };
    if !status.success() {
        let excuse = excuse();
        let excuse = if excuse.is_empty() { String::new() } else { format!(", and said: {excuse}") };
        bail!("the plugin gave up drawing {url}: {status}{excuse}");
    }
    if out.trim().is_empty() {
        let excuse = excuse();
        let excuse = if excuse.is_empty() { String::new() } else { format!("; it said: {excuse}") };
        bail!("the plugin drew {url} and handed back nothing{excuse}");
    }
    Ok(out)
}
