//! The hinge, followed for as long as the caller wants, on its own thread.
//!
//! `hinge_angle` is an on-change sensor: it sends a sample when the angle
//! moves and stays quiet when the phone is still. Measured on bixi over one
//! close-and-open, the angle walks 179 → 1 → 179 with a frame at every step,
//! so a fold can be read straight off it without the accelerometer.
//!
//! This is for the outer display only: the inner one folds with the same
//! sensor, and its rules are not these.
//!
//! A reading is kept in shared state, so a UI thread reads the latest value
//! without waiting on the sensor: [`Hinge::angle`] and [`Hinge::folded`] never
//! block, and [`Hinge::wait`] blocks at most as long as it is told to. When the
//! ADSP or its daemon goes away the reading goes to `None` and the thread
//! keeps retrying in the background; nothing becomes an error the UI has to
//! handle.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::client::Client;
use crate::fold::Reading;

/// How often the sensor is asked to report, in hertz. It only sends on change.
const RATE_HZ: u32 = 10;
/// How long one read waits before checking that the service is still there.
const SLICE: Duration = Duration::from_millis(500);
/// How long silence is tolerated before the service is asked.
const SILENCE: Duration = Duration::from_secs(5);
/// The first wait before a reconnect, doubling up to the cap.
const BACKOFF: Duration = Duration::from_secs(1);
const BACKOFF_CAP: Duration = Duration::from_secs(8);

/// The hinge, followed in the background.
///
/// ```no_run
/// let hinge = keel_sensors::Hinge::open()?;
/// while let Some(reading) = hinge.wait(std::time::Duration::from_secs(1)) {
///     println!("{:?}", reading.folded());
/// }
/// # Ok::<(), anyhow::Error>(())
/// ```
pub struct Hinge {
    shared: Arc<(Mutex<Reading>, Condvar)>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Hinge {
    /// Start following the hinge. The socket is opened on the worker thread,
    /// so this returns at once even when the service is not there yet.
    pub fn open() -> Result<Self> {
        let shared = Arc::new((Mutex::new(Reading::default()), Condvar::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name("hinge".to_owned())
                .spawn(move || follow(shared, stop))?
        };
        Ok(Hinge {
            shared,
            stop,
            worker: Some(worker),
        })
    }

    /// The current angle, or `None` while the sensor is not answering.
    pub fn angle(&self) -> Option<f32> {
        self.reading().angle()
    }

    /// Folded (`true`), open (`false`), or unknown.
    pub fn folded(&self) -> Option<bool> {
        self.reading().folded()
    }

    /// Whether the sensor was last seen answering.
    pub fn online(&self) -> bool {
        self.reading().online()
    }

    /// The whole reading, as one value.
    pub fn reading(&self) -> Reading {
        *self.state()
    }

    /// Wait until the next sample, at most `timeout`.
    ///
    /// `None` means nothing new came in that time, which is the ordinary state
    /// of a still phone and not an error. This is the call a UI thread makes
    /// when it has nothing else to do.
    pub fn wait(&self, timeout: Duration) -> Option<Reading> {
        let (state, changed) = &*self.shared;
        let mut reading = state.lock().expect("the hinge state is not poisoned");
        let generation = reading.generation;
        let deadline = Instant::now() + timeout;
        while reading.generation == generation {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            let (next, timed) = changed
                .wait_timeout(reading, left)
                .expect("the hinge state is not poisoned");
            reading = next;
            if timed.timed_out() {
                return None;
            }
        }
        Some(*reading)
    }

    /// Stop following it and wait for the thread to let go.
    ///
    /// The thread gives up within one read slice; a caller that cannot wait
    /// should drop the value instead, which only raises the flag.
    pub fn close(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.shared.1.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    fn state(&self) -> MutexGuard<'_, Reading> {
        self.shared
            .0
            .lock()
            .expect("the hinge state is not poisoned")
    }
}

impl Drop for Hinge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.shared.1.notify_all();
        // Not joined: a caller dropping this does not want to wait out the
        // read slice, and the thread holds nothing but its own copies.
        let _ = self.worker.take();
    }
}

/// The worker: connect, follow, and reconnect until told to stop.
fn follow(shared: Arc<(Mutex<Reading>, Condvar)>, stop: Arc<AtomicBool>) {
    let mut backoff = BACKOFF;
    while !stop.load(Ordering::Relaxed) {
        match follow_once(&shared, &stop) {
            Ok(()) => backoff = BACKOFF,
            Err(_) => {
                shared
                    .0
                    .lock()
                    .expect("the hinge state is not poisoned")
                    .offline();
                shared.1.notify_all();
                sleep(backoff, &stop);
                backoff = (backoff * 2).min(BACKOFF_CAP);
            }
        }
    }
    shared
        .0
        .lock()
        .expect("the hinge state is not poisoned")
        .offline();
    shared.1.notify_all();
}

/// One connection's worth of following, until it fails or the caller stops.
fn follow_once(shared: &Arc<(Mutex<Reading>, Condvar)>, stop: &Arc<AtomicBool>) -> Result<()> {
    let mut client = Client::open()?;
    let sensor = client.find("hinge_angle")?;
    let mut receiver = client.read_sensor(&sensor, RATE_HZ)?;
    {
        let (state, changed) = &**shared;
        let mut reading = state.lock().expect("the hinge state is not poisoned");
        if !reading.online {
            reading.online = true;
            reading.generation += 1;
            changed.notify_all();
        }
    }
    let mut silent = Duration::ZERO;
    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = receiver.stop();
            return Ok(());
        }
        match receiver.next(SLICE)? {
            Some(values) => {
                silent = Duration::ZERO;
                if let Some(angle) = values.first() {
                    let (state, changed) = &**shared;
                    let mut reading = state.lock().expect("the hinge state is not poisoned");
                    reading.push(*angle);
                    changed.notify_all();
                }
            }
            None => {
                silent += SLICE;
                if silent >= SILENCE {
                    silent = Duration::ZERO;
                    receiver.ping()?;
                }
            }
        }
    }
}

/// Sleep, but give up as soon as the caller asks to stop.
fn sleep(how_long: Duration, stop: &AtomicBool) {
    let deadline = Instant::now() + how_long;
    while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
}
