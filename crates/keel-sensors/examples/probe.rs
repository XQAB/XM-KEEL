//! Prints what the sensor service on the ADSP has, then reads the fold sensor
//! and the light sensor a few times.
//!
//! `cargo build --release --target aarch64-unknown-linux-musl --example probe`

use anyhow::Result;
use std::time::Duration;

fn main() -> Result<()> {
    let mut client = keel_sensors::Client::open()?;
    println!("server: node {} port {}", client.server().0, client.server().1);

    let sensors = client.list()?;
    println!("sensors: {}", sensors.len());
    for sensor in &sensors {
        println!("  {:<24} {}", sensor.name, sensor.uid());
    }

    for name in ["hinge_angle", "flip_status", "ambient_light"] {
        let mut receiver = match client.read(name, 10) {
            Ok(receiver) => receiver,
            Err(e) => {
                println!("{name}: {e:#}");
                continue;
            }
        };
        for _ in 0..3 {
            match receiver.next(Duration::from_secs(2)) {
                Ok(Some(values)) => println!("{name}: {values:?}"),
                Ok(None) => println!("{name}: nothing came back"),
                Err(e) => println!("{name}: {e:#}"),
            }
        }
        receiver.stop()?;
    }
    Ok(())
}
