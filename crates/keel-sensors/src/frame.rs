//! The bytes of the sensor service's wire protocol, and nothing else.
//!
//! Kept apart from the socket so the layout can be checked on a host: the
//! golden test at the bottom is a frame that was captured on a phone and
//! answered with a real sample.
//!
//! The layout, as it is on the wire:
//!
//! ```text
//! 00 <txn:1> 00 20 <qlen:BE16>            QMI header, msgid 0x0020 big endian
//! 00 01 <tlvlen:LE16> <pblen:LE16>        the protobuf is one TLV, tag 1
//! <protobuf>
//! 10 01 00 01                             optional tail, a client id of one
//! ```
//!
//! `qlen` counts from the TLV onwards and leaves out the padding byte before
//! it, which is why it is one shorter than what is actually sent.

/// The request message every call uses.
const REQUEST: u16 = 0x0020;
/// The reply to a request.
const RESPONSE: [u8; 2] = [0x00, 0x21];
/// An asynchronous event.
const INDICATION: [u8; 2] = [0x00, 0x22];

/// A body of events, decoded.
#[derive(Clone, Debug, Default)]
pub struct Event {
    /// Which sensor said it, as the sixteen bytes the service uses.
    pub suid: [u8; 16],
    /// The sensor's own message number, e.g. 1025 for a sample.
    pub message: u32,
    /// The payload the message carried, as it was on the wire.
    pub payload: Vec<u8>,
    /// Every float in the payload, in order.
    pub values: Vec<f32>,
}

/// What the service answered a request with.
#[derive(Clone, Copy, Debug, Default)]
pub struct Answer {
    pub transaction: u8,
    pub accepted: bool,
    pub error: u16,
    pub client: Option<u16>,
}

/// Build one request: the sensor's uid, its message number, the payload that
/// message takes, and an optional second body.
///
/// The tail is a client id that configuration requests need and registry
/// queries must not carry: with it, the lookup is answered `INVALID_TYPE`.
pub fn request(transaction: u8, suid: [u8; 16], message: u32, payload: &[u8], extra: Option<&[u8]>, with_tail: bool) -> Vec<u8> {
    let mut body = Vec::with_capacity(24 + payload.len() + extra.map_or(0, |e| e.len() + 2));
    field_bytes(&mut body, 1, &uid(suid));
    body.push(0x15);
    body.extend_from_slice(&message.to_le_bytes());
    if !payload.is_empty() {
        field_bytes(&mut body, 3, payload);
    }
    if let Some(extra) = extra {
        field_bytes(&mut body, 4, extra);
    }

    let tail = if with_tail { 4 } else { 0 };
    let length = 1 + 4 + body.len() + tail;
    let mut out = Vec::with_capacity(2 + length);
    out.push(0x00);
    out.push(transaction);
    out.extend_from_slice(&REQUEST.to_be_bytes());
    out.extend_from_slice(&(length as u16).to_be_bytes());
    out.extend_from_slice(&[0x00, 0x01]);
    out.extend_from_slice(&((body.len() + 2) as u16).to_le_bytes());
    out.extend_from_slice(&(body.len() as u16).to_le_bytes());
    out.extend_from_slice(&body);
    if with_tail {
        out.extend_from_slice(&[0x10, 0x01, 0x00, 0x01]);
    }
    out
}

/// The uid as the service writes it: two fixed 64-bit fields.
pub fn uid(suid: [u8; 16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(18);
    out.push(0x09);
    out.extend_from_slice(&suid[..8]);
    out.push(0x11);
    out.extend_from_slice(&suid[8..]);
    out
}

/// The answer a reply frame carries, when the frame is one.
pub fn answer(data: &[u8]) -> Option<Answer> {
    let (flag, _, message, body) = header(data)?;
    if flag != 0x04 || message.to_be_bytes() != RESPONSE {
        return None;
    }
    let mut answer = Answer { transaction: data[1], ..Answer::default() };
    for (tag, _, value) in fields(body).ok()? {
        match tag {
            0x02 => {
                // result, then error, both little endian 16-bit.
                if value.len() >= 4 {
                    answer.accepted = u16::from_le_bytes([value[0], value[1]]) == 0;
                    answer.error = u16::from_le_bytes([value[2], value[3]]);
                } else if let Some(&first) = value.first() {
                    answer.accepted = first == 0;
                }
            }
            0x11 if value.len() >= 2 => {
                answer.client = Some(u16::from_le_bytes([value[0], value[1]]));
            }
            _ => {}
        }
    }
    Some(answer)
}

/// Every event in one frame.
pub fn events(data: &[u8]) -> Vec<Event> {
    let mut events = Vec::new();
    let Some((flag, _, message, body)) = header(data) else { return events };
    if flag != 0x04 || message.to_be_bytes() != INDICATION {
        return events;
    }
    let Ok(tlvs) = fields(body) else { return events };
    for (tag, _, value) in tlvs {
        if tag != 0x02 {
            continue;
        }
        let body = strip_length(value);
        let Ok(envelope) = fields(body) else { continue };
        let Some(suid) = envelope.iter().find(|&&(t, w, _)| t == 1 && w == 2).and_then(|&(_, _, v)| parse_uid(v)) else { continue };
        for &(_, _, value) in envelope.iter().filter(|&&(t, _, _)| t == 2) {
            let Ok(inner) = fields(value) else { continue };
            let message = inner
                .iter()
                .find(|&&(t, w, v)| t == 1 && (w == 5 || (w == 0 && v.len() == 4)))
                .map(|&(_, _, v)| u32::from_le_bytes(v[..4].try_into().unwrap()))
                .unwrap_or_default();
            let payload = inner.iter().find(|&&(t, _, _)| t == 3).map(|&(_, _, v)| v.to_vec()).unwrap_or_default();
            let values = floats(&payload);
            events.push(Event { suid, message, payload, values });
        }
    }
    events
}

/// The floats a payload holds: field 1, either packed or one per field.
pub fn floats(data: &[u8]) -> Vec<f32> {
    let Ok(parsed) = fields(data) else { return Vec::new() };
    parsed
        .iter()
        .filter(|&&(tag, wire, value)| tag == 1 && matches!(wire, 2 | 5) && value.len() % 4 == 0)
        .flat_map(|&(_, _, value)| value.as_chunks::<4>().0.iter().map(|v| f32::from_le_bytes(*v)))
        .collect()
}

/// Every name and uid pair a registry listing carries.
pub fn names(data: &[u8]) -> Vec<(String, [u8; 16])> {
    let Ok(parsed) = fields(data) else { return Vec::new() };
    let mut pairs = Vec::new();
    let mut pending: Option<String> = None;
    for (_, wire, value) in parsed {
        if wire != 2 {
            continue;
        }
        if let Some(suid) = parse_uid(value) {
            if let Some(name) = pending.take() {
                pairs.push((name, suid));
            }
            continue;
        }
        // A nested entry of its own, name first and uid second.
        let Ok(inner) = fields(value) else { continue };
        if let Some(name) = inner.iter().find_map(|&(t, w, v)| (t == 1 && w == 2 && printable(v)).then(|| String::from_utf8_lossy(v).into_owned())) {
            if let Some(suid) = inner.iter().find_map(|&(t, w, v)| (t == 2 && w == 2).then(|| parse_uid(v)).flatten()) {
                pairs.push((name, suid));
                pending = None;
                continue;
            }
            pending = Some(name);
            continue;
        }
        if printable(value) {
            pending = Some(String::from_utf8_lossy(value).into_owned());
        }
    }
    pairs
}

/// The uid inside a message, from its two fixed 64-bit fields.
pub fn parse_uid(data: &[u8]) -> Option<[u8; 16]> {
    let fields = fields(data).ok()?;
    let low = fields.iter().find(|&&(t, w, v)| t == 1 && w == 1 && v.len() == 8)?;
    let high = fields.iter().find(|&&(t, w, v)| t == 2 && w == 1 && v.len() == 8)?;
    let mut suid = [0u8; 16];
    suid[..8].copy_from_slice(low.2);
    suid[8..].copy_from_slice(high.2);
    Some(suid)
}

/// The uid written the way the logs and this project's notes write it.
pub fn uid_text(suid: [u8; 16]) -> String {
    let low = u64::from_le_bytes(suid[..8].try_into().unwrap());
    let high = u64::from_le_bytes(suid[8..].try_into().unwrap());
    format!("{low:016x}/{high:016x}")
}

fn header(data: &[u8]) -> Option<(u8, u8, u16, &[u8])> {
    if data.len() < 8 {
        return None;
    }
    let flag = data[0];
    let transaction = data[1];
    let message = u16::from_be_bytes([data[3], data[4]]);
    let length = u16::from_be_bytes([data[5], data[6]]) as usize;
    // The length leaves out the padding byte the sender writes before the TLV.
    let body = data.get(7..)?;
    let body = body.get(..=length).unwrap_or(body);
    Some((flag, transaction, message, body))
}

/// A value that may carry its own two-byte length, as replies do.
fn strip_length(value: &[u8]) -> &[u8] {
    if value.len() >= 3 {
        let length = u16::from_le_bytes([value[0], value[1]]) as usize;
        if length + 2 <= value.len() {
            return &value[2..length + 2];
        }
    }
    value
}

fn printable(value: &[u8]) -> bool {
    !value.is_empty() && value.iter().all(|b| b.is_ascii_graphic() || *b == b' ' || *b == b'_')
}

fn field_bytes(out: &mut Vec<u8>, tag: u8, value: &[u8]) {
    out.push(tag << 3 | 2);
    varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn varint_read(data: &mut &[u8]) -> Option<u64> {
    let mut result = 0u64;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = data.split_first()?;
        *data = rest;
        result |= (byte as u64 & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(result);
        }
    }
    None
}

/// Every field of a message: number, wire type, and the bytes it took.
fn fields(mut data: &[u8]) -> anyhow::Result<Vec<(u8, u8, &[u8])>> {
    let mut result = Vec::new();
    while !data.is_empty() {
        let key = varint_read(&mut data).ok_or_else(|| anyhow::anyhow!("short field key"))?;
        if key >> 3 == 0 || key >> 3 > 0xff {
            anyhow::bail!("field number out of range");
        }
        let tag = (key >> 3) as u8;
        match (key & 7) as u8 {
            0 => {
                let before = data;
                varint_read(&mut data).ok_or_else(|| anyhow::anyhow!("short varint"))?;
                let taken = &before[..before.len() - data.len()];
                result.push((tag, 0, taken));
            }
            1 => {
                if data.len() < 8 {
                    anyhow::bail!("short fixed64");
                }
                let (value, rest) = data.split_at(8);
                result.push((tag, 1, value));
                data = rest;
            }
            2 => {
                let length = varint_read(&mut data).ok_or_else(|| anyhow::anyhow!("short length"))? as usize;
                if length > data.len() {
                    anyhow::bail!("short field body");
                }
                let (value, rest) = data.split_at(length);
                result.push((tag, 2, value));
                data = rest;
            }
            5 => {
                if data.len() < 4 {
                    anyhow::bail!("short fixed32");
                }
                let (value, rest) = data.split_at(4);
                result.push((tag, 5, value));
                data = rest;
            }
            other => anyhow::bail!("unsupported wire type {other}"),
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame captured on the phone: inner 513 at 10 Hz for the accelerometer,
    /// answered with an ACK and two error events.
    const CAPTURED: &str = "00010020003100012a0028000a120988ea9c0b4ec2463811aba0d340e8d2eb5815010200001a040801100122070a050d0000204110010001";

    fn hex(data: &[u8]) -> String {
        data.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn bytes(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
    }

    /// The uid as it appears in the capture: low eight, then high eight.
    fn captured_uid() -> [u8; 16] {
        let mut suid = [0u8; 16];
        suid[..8].copy_from_slice(&bytes("88ea9c0b4ec24638"));
        suid[8..].copy_from_slice(&bytes("aba0d340e8d2eb58"));
        suid
    }

    #[test]
    fn a_continuous_configuration_is_the_frame_the_phone_saw() {
        let payload = [0x08, 0x01, 0x10, 0x01];
        let extra = [0x0a, 0x05, 0x0d, 0x00, 0x00, 0x20, 0x41]; // 10.0 Hz, fixed32
        let frame = request(1, captured_uid(), 513, &payload, Some(&extra), true);
        assert_eq!(hex(&frame), CAPTURED);
    }

    #[test]
    fn the_tail_is_a_switch() {
        let mut suid = [0u8; 16];
        suid[..8].copy_from_slice(&bytes("088285bf47d64e7d"));
        suid[8..].copy_from_slice(&bytes("b71d3ab9d3c58e2d"));
        let mut extra = Vec::new();
        field_bytes(&mut extra, 1, b"registry");
        let without = request(7, suid, 512, &[0x08, 0x01, 0x10, 0x01], Some(&extra), false);
        let with = request(7, suid, 512, &[0x08, 0x01, 0x10, 0x01], Some(&extra), true);
        assert_eq!(with.len(), without.len() + 4);
        assert_eq!(&with[with.len() - 4..], &[0x10, 0x01, 0x00, 0x01]);
        // The length excludes the padding byte, so it stays one below the bytes sent.
        let length = u16::from_be_bytes([without[4], without[5]]) as usize;
        assert_eq!(length + 1, without.len() - 6);
    }

    #[test]
    fn a_sample_reads_as_floats() {
        // 1025 with three floats and a status, as the light sensor sends it.
        let mut payload = Vec::new();
        let mut packed = Vec::new();
        packed.extend_from_slice(&1.667f32.to_le_bytes());
        packed.extend_from_slice(&0f32.to_le_bytes());
        packed.extend_from_slice(&0f32.to_le_bytes());
        field_bytes(&mut payload, 1, &packed);
        payload.extend_from_slice(&[0x10, 0x03]);
        assert_eq!(floats(&payload), vec![1.667, 0.0, 0.0]);
    }

    #[test]
    fn a_uid_reads_back() {
        let suid = captured_uid();
        assert_eq!(parse_uid(&uid(suid)), Some(suid));
        assert_eq!(uid_text(suid), "3846c24e0b9cea88/58ebd2e840d3a0ab");
    }
}
