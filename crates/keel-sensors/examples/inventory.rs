//! Every uid in the registry, with the name its attributes give it.

#[cfg(unix)]
fn main() -> anyhow::Result<()> {
    let mut client = keel_sensors::Client::open()?;
    let sensors = client.list()?;
    println!("registry: {} uids", sensors.len());
    for sensor in &sensors {
        match client.describe(sensor.suid) {
            Ok(name) => println!("{:<20} {}", name, sensor.uid()),
            Err(e) => println!("{:<20} {e:#}", sensor.uid()),
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn main() {}
