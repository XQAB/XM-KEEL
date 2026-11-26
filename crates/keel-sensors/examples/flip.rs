//! Follows the hinge and prints only what changes, for as long as it runs.
//!
//! `cargo build -p keel-sensors --release --target aarch64-unknown-linux-musl --example flip`

#[cfg(unix)]
fn main() -> anyhow::Result<()> {
    use std::time::Duration;

    let hinge = keel_sensors::Hinge::open()?;
    println!("hinge: following, {} Hz", 10);
    loop {
        match hinge.wait(Duration::from_secs(5)) {
            Some(reading) => println!(
                "angle {:?} folded {:?} online {}",
                reading.angle(),
                reading.folded(),
                reading.online()
            ),
            None => println!(
                "nothing for 5 s; angle {:?} folded {:?}",
                hinge.angle(),
                hinge.folded()
            ),
        }
    }
}

#[cfg(not(unix))]
fn main() {}
