//! Prints what the sensor service on the ADSP has, reads the fold sensor and
//! the light sensor a few times, and says why the ones that refuse, refuse.
//!
//! `cargo build -p keel-sensors --release --target aarch64-unknown-linux-musl --example probe`

#[cfg(unix)]
fn main() -> anyhow::Result<()> {
    use std::time::Duration;

    let mut client = keel_sensors::Client::open()?;
    println!(
        "service: node {} port {}",
        client.server().0,
        client.server().1
    );

    let sensors = client.list()?;
    println!("registry: {} uids", sensors.len());
    for sensor in sensors.iter().filter(|sensor| !sensor.name.is_empty()) {
        println!("  {:<16} {}", sensor.name, sensor.uid());
    }

    for name in ["hinge_angle", "flip_status", "ambient_light"] {
        match client.read(name, 10) {
            Ok(mut receiver) => {
                for _ in 0..3 {
                    match receiver.next(Duration::from_secs(2)) {
                        Ok(Some(values)) => println!("{name}: {values:?}"),
                        Ok(None) => println!("{name}: nothing came back"),
                        Err(e) => println!("{name}: {e:#}"),
                    }
                }
                receiver.stop()?;
            }
            Err(e) => println!("{name}: {e:#}"),
        }
    }

    for name in ["accel", "gravity", "ori", "gyro"] {
        let sensor = client.find(name)?;
        match client.enable(sensor.suid, 10) {
            Ok(enabled) => println!("{name}: {:?}", enabled.sample),
            Err(e) => println!("{name}: {e:#}"),
        }
    }
    Ok(())
}

/// The sensor service is a Linux socket; there is nothing to show here.
#[cfg(not(unix))]
fn main() {}
