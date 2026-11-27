//! Prints what the sensor service on the ADSP has, reads the fold sensor and
//! the light sensor, and says why the ones that refuse, refuse.
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

    for name in ["hinge_angle", "flip_status", "ambient_light", "tilt"] {
        print!("{name}: ");
        match client.named(name) {
            Ok(found) if found.is_empty() => println!("not there"),
            Ok(found) => println!(
                "{:?}",
                found.iter().map(|sensor| sensor.uid()).collect::<Vec<_>>()
            ),
            Err(error) => println!("{error:#}"),
        }
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

    // The ones that will not run, and what they say about it. A name this
    // phone does not have is skipped, not fatal.
    for name in ["accel", "gravity", "ori", "gyro", "tilt"] {
        let Ok(found) = client.named(name) else {
            continue;
        };
        for sensor in found {
            match client.enable(sensor.suid, 10) {
                Ok(enabled) => println!("{name} {}: {:?}", sensor.uid(), enabled.sample),
                Err(e) => println!("{name} {}: {e:#}", sensor.uid()),
            }
        }
    }

    // One sensor's attributes, which is where its name lives when the listing
    // has none.
    let hinge = client.find("hinge_angle")?;
    for attribute in client.attributes(hinge.suid)? {
        match (attribute.text(), attribute.int()) {
            (Some(text), _) => println!("attr {:>2}: {text}", attribute.id),
            (None, Some(value)) => println!("attr {:>2}: {value}", attribute.id),
            (None, None) => println!("attr {:>2}: {:02x?}", attribute.id, attribute.value),
        }
    }
    Ok(())
}

/// The sensor service is a Linux socket; there is nothing to show here.
#[cfg(not(unix))]
fn main() {}
