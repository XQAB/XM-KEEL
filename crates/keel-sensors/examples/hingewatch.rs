//! Watches `hinge_angle` for a while and prints every sample as it arrives,
//! with the seconds since the watch started. On-change sensors only speak when
//! the value moves, so a still phone prints one line and then nothing.
//!
//! `cargo build -p keel-sensors --release --target aarch64-unknown-linux-musl --example hingewatch`
//! then `adb push .../hingewatch /data/local/tmp/ && adb shell '/data/local/tmp/hingewatch 30'`

#[cfg(unix)]
fn main() -> anyhow::Result<()> {
    use std::time::{Duration, Instant};

    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    let rate: u32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    let mut client = keel_sensors::Client::open()?;
    println!("service: node {} port {}", client.server().0, client.server().1);

    let started = Instant::now();
    let mut receiver = client.read("hinge_angle", rate)?;
    let mut seen = 0u32;
    while started.elapsed() < Duration::from_secs(seconds) {
        match receiver.next(Duration::from_millis(500)) {
            Ok(Some(values)) => {
                seen += 1;
                println!("{:>7.2}s {:?}", started.elapsed().as_secs_f32(), values);
            }
            Ok(None) => {}
            Err(e) => {
                println!("{:>7.2}s error: {e:#}", started.elapsed().as_secs_f32());
                break;
            }
        }
    }
    println!("end: {seen} samples in {seconds}s");
    receiver.stop()?;
    Ok(())
}

/// The sensor service is a Linux socket; there is nothing to show here.
#[cfg(not(unix))]
fn main() {}
