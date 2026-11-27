//! One request without a client around it: open the socket, find the service,
//! send the message, hand back the message that answers it.
//!
//! This is the shape the notes ask for, for a one-off probe or for a caller
//! that only ever sends one query.

use anyhow::Result;
use std::time::{Duration, Instant};

use crate::frame::{self, Answer, Message};
use crate::qrtr::{Transport, SSC};

/// One request to the sensor service, at its own uid, and the message back.
///
/// `extra` is the second body, in the shapes `frame::request` takes; `with_tail`
/// adds the client id, which a configuration carries and a registry query can
/// leave out.
pub fn request(
    inner: u32,
    payload: &[u8],
    extra: Option<&[u8]>,
    with_tail: bool,
) -> Result<Message> {
    request_to([0xab; 16], inner, payload, extra, with_tail)
}

/// The same, addressed to a sensor's own uid.
pub fn request_to(
    suid: [u8; 16],
    inner: u32,
    payload: &[u8],
    extra: Option<&[u8]>,
    with_tail: bool,
) -> Result<Message> {
    let transport = Transport::open()?;
    let (node, port) = transport.lookup(SSC)?;
    let frame = frame::request(1, suid, inner, payload, extra, with_tail);
    transport.send(transport.address(node, port), &frame)?;

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut acknowledgement: Option<Answer> = None;
    while Instant::now() < deadline {
        let Some((_, data)) = transport.receive(Duration::from_millis(20))? else {
            continue;
        };
        if let Some(answer) = frame::answer(&data) {
            acknowledgement = Some(answer);
            continue;
        }
        if let Some(message) = frame::messages(&data).into_iter().next() {
            return Ok(message);
        }
    }
    match acknowledgement {
        Some(answer) if !answer.accepted => anyhow::bail!(
            "the service refused message {inner}: error {}",
            answer.error
        ),
        Some(_) => anyhow::bail!("message {inner} was acknowledged but never answered"),
        None => anyhow::bail!("no answer to message {inner}"),
    }
}
