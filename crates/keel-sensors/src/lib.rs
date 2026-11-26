//! Reading the phone's sensors straight from the ADSP, with no Android
//! framework in the way.
//!
//! The kernel's QRTR sockets reach Qualcomm's sensor service directly; this
//! crate speaks its small protobuf dialect and nothing else. It is meant for
//! recovery, where no sensor HAL and no framework service exists, and where a
//! cold boot leaves the service with its complete registry.
//!
//! ```ignore
//! # fn main() -> anyhow::Result<()> {
//! let angle = keel_sensors::hinge_angle()?;
//! let lux = keel_sensors::ambient_light()?;
//! # let _ = (angle, lux);
//! # Ok(())
//! # }
//! ```
//!
//! Two things have to be true before any of this answers: the sensors' domain
//! has to be up on the ADSP, and it must not have been restarted since boot.
//! A restart clears the registry, and nothing on the application processor can
//! put it back; see `docs/sensors-in-recovery.md`.

pub mod frame;

#[cfg(unix)]
pub mod client;
#[cfg(unix)]
pub mod qrtr;

#[cfg(unix)]
pub use client::{ambient_light, flip_status, hinge_angle, sample, Client, Receiver, Sensor};
