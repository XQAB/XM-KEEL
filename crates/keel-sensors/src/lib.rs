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

pub mod flip;
pub mod fold;
pub mod frame;

#[cfg(unix)]
pub mod client;
#[cfg(unix)]
pub mod hinge;
#[cfg(unix)]
pub mod qmi;
#[cfg(unix)]
pub mod qrtr;

pub use flip::FlipController;
pub use fold::{fold, Reading, FOLDED_BELOW, OPEN_ABOVE};

#[cfg(unix)]
pub use client::{
    ambient_light, flip_status, hinge_angle, sample, Client, Enabled, Receiver, Sensor,
};
#[cfg(unix)]
pub use hinge::Hinge;

/// Where the sensors' service lives, as `(node, port)`.
#[cfg(unix)]
pub fn lookup(service: u32) -> anyhow::Result<(u32, u32)> {
    qrtr::lookup(service)
}

/// The first message in a frame, and whether it is a sample: `(uid, values)`.
///
/// Both a reply and an event are read the same way, so a caller that only has
/// bytes can still get at them.
#[cfg(unix)]
pub fn parse_event(buffer: &[u8]) -> Option<([u8; 16], Vec<f32>)> {
    frame::messages(buffer)
        .into_iter()
        .next()
        .map(|message| (message.suid, message.values))
}

/// The same, from a platform without the socket: the frame layout alone still
/// decodes, which is what the host tests use.
#[cfg(not(unix))]
pub fn parse_event(buffer: &[u8]) -> Option<([u8; 16], Vec<f32>)> {
    frame::messages(buffer)
        .into_iter()
        .next()
        .map(|message| (message.suid, message.values))
}
