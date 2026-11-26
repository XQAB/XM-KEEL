//! The bytes of the sensor service's wire protocol, and nothing else.
//!
//! Kept apart from the socket so the layout can be checked on a host: the
//! golden test at the bottom is a frame that was captured on a phone and
//! answered with a real sample.
//!
//! A request, as it is on the wire:
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
//!
//! An answer, as it is on the wire:
//!
//! ```text
//! 04 <txn> 00 21 <len:BE16> 00           the service's reply
//! 01 08 00 <sequence:8>                  the sequence number
//! 02 <len:LE16> <body:LE16> <protobuf>   the protobuf, length counted twice
//! ```
//!
//! and the protobuf inside it:
//!
//! ```text
//! 0a 12 09 <uid low 8> 11 <uid high 8>   the sensor it is about
//! 12 <len> 0d <message:LE32> 11 <seq:8> 1a <len> <payload>
//! ```
//!
//! Events (msgid `00 22`) carry the same protobuf, so one reader takes both.

/// The request message every call uses.
const REQUEST: u16 = 0x0020;
/// The reply to a request.
const RESPONSE: u16 = 0x0021;
/// An asynchronous event.
const INDICATION: u16 = 0x0022;

/// One message out of the service: which sensor, which of its messages, and
/// the payload that message carried.
#[derive(Clone, Debug, Default)]
pub struct Message {
    /// The sensor the message is about, as the sixteen bytes the service uses.
    pub suid: [u8; 16],
    /// The sensor's own message number, e.g. 1025 for a sample, 130 for a
    /// status.
    pub message: u32,
    /// The payload the message carried, as it was on the wire.
    pub payload: Vec<u8>,
    /// Every float in the payload, in order.
    pub values: Vec<f32>,
}

/// What the transport acknowledged a request with.
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
/// The tail is a client id that configuration requests carry and registry
/// queries can leave out; on this device the registry answers either way.
pub fn request(
    transaction: u8,
    suid: [u8; 16],
    message: u32,
    payload: &[u8],
    extra: Option<&[u8]>,
    with_tail: bool,
) -> Vec<u8> {
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

/// The acknowledgement a reply frame carries, when the frame is one.
///
/// This is the transport's own answer, `result` and `error`, and it comes
/// before whatever the sensor itself says about the request.
pub fn answer(data: &[u8]) -> Option<Answer> {
    if data.first() != Some(&0x02) {
        return None;
    }
    // `02 04 00` and then the four bytes of result and error. The bytes before
    // it are the header and, sometimes, a pad, and they are not always the same
    // count, so the body is found rather than counted.
    let at = (4..data.len().min(12)).find(|&at| {
        data[at] == 0x02 && data.get(at + 1) == Some(&0x04) && data.get(at + 2) == Some(&0x00)
    })?;
    let value = data.get(at + 3..at + 7)?;
    let mut answer = Answer {
        transaction: data[1],
        ..Answer::default()
    };
    answer.accepted = u16::from_le_bytes([value[0], value[1]]) == 0;
    answer.error = u16::from_le_bytes([value[2], value[3]]);
    Some(answer)
}

/// Every message in one frame: a reply or an event, read the same way.
pub fn messages(data: &[u8]) -> Vec<Message> {
    let mut messages = Vec::new();
    let Some((flag, _, message, body)) = header(data) else {
        return messages;
    };
    if flag != 0x04 || (message != RESPONSE && message != INDICATION) {
        return messages;
    }
    let protobuf = tlvs(body)
        .into_iter()
        .find(|&(tag, _)| tag == 0x02)
        .map(|(_, value)| value);
    let Some(protobuf) = protobuf else {
        return messages;
    };
    let Ok(envelope) = fields(protobuf) else {
        return messages;
    };
    let suid = envelope
        .iter()
        .find(|&&(tag, wire, _)| tag == 1 && wire == 2)
        .and_then(|&(_, _, value)| parse_uid(value))
        .unwrap_or_default();
    for &(_, _, value) in envelope.iter().filter(|&&(tag, _, _)| tag == 2) {
        let Ok(inner) = fields(value) else { continue };
        let message = inner
            .iter()
            .find(|&&(tag, wire, value)| tag == 1 && (wire == 5 || (wire == 0 && value.len() == 4)))
            .map(|&(_, _, value)| u32::from_le_bytes(value[..4].try_into().unwrap()))
            .unwrap_or_default();
        let payload = inner
            .iter()
            .find(|&&(tag, _, _)| tag == 3)
            .map(|&(_, _, value)| value.to_vec())
            .unwrap_or_default();
        let values = floats(&payload);
        messages.push(Message {
            suid,
            message,
            payload,
            values,
        });
    }
    messages
}

/// Every entry of a registry listing: a name when the query carried one, and
/// the uid it stands for.
pub fn entries(data: &[u8]) -> Vec<(Option<String>, [u8; 16])> {
    let Ok(parsed) = fields(data) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut pending: Option<String> = None;
    for (_, wire, value) in parsed {
        if wire != 2 {
            continue;
        }
        // A bare uid, as a listing sends them: `09 <low 8> 11 <high 8>`.
        if let Some(suid) = parse_uid(value) {
            out.push((pending.take(), suid));
            continue;
        }
        // An entry of its own, a name and then a uid.
        let Ok(inner) = fields(value) else {
            // A name on its own, with its uid in the field after it.
            if printable(value) {
                pending = Some(String::from_utf8_lossy(value).into_owned());
            }
            continue;
        };
        let name = inner
            .iter()
            .find(|&&(tag, wire, _)| tag == 1 && wire == 2)
            .map(|&(_, _, value)| String::from_utf8_lossy(value).into_owned())
            .filter(|name| !name.is_empty());
        let suid = inner
            .iter()
            .find(|&&(tag, wire, _)| tag == 2 && wire == 2)
            .and_then(|&(_, _, value)| parse_uid(value));
        if let Some(suid) = suid {
            out.push((name, suid));
        }
    }
    out
}

/// The first integer a payload holds, which is what a status carries: the
/// reason the sensor will not run.
pub fn first_int(data: &[u8]) -> Option<u64> {
    let parsed = fields(data).ok()?;
    let &(_, wire, value) = parsed.iter().find(|&&(tag, _, _)| tag == 1)?;
    match wire {
        0 => {
            let mut rest = value;
            varint_read(&mut rest)
        }
        5 => value
            .get(..4)
            .map(|head| u32::from_le_bytes(head.try_into().unwrap()) as u64),
        _ => None,
    }
}

/// The floats a payload holds: field 1, either packed or one per field.
pub fn floats(data: &[u8]) -> Vec<f32> {
    let Ok(parsed) = fields(data) else {
        return Vec::new();
    };
    parsed
        .iter()
        .filter(|&&(tag, wire, value)| tag == 1 && matches!(wire, 2 | 5) && value.len() % 4 == 0)
        .flat_map(|&(_, _, value)| {
            value
                .as_chunks::<4>()
                .0
                .iter()
                .map(|v| f32::from_le_bytes(*v))
        })
        .collect()
}

/// The uid inside a message, from its two fixed 64-bit fields.
pub fn parse_uid(data: &[u8]) -> Option<[u8; 16]> {
    let fields = fields(data).ok()?;
    let low = fields
        .iter()
        .find(|&&(tag, wire, value)| tag == 1 && wire == 1 && value.len() == 8)?;
    let high = fields
        .iter()
        .find(|&&(tag, wire, value)| tag == 2 && wire == 1 && value.len() == 8)?;
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

/// The frame's header: flag, transaction, message, and the body that follows.
///
/// The body is found rather than counted: the length the header declares and
/// the pad around it are not placed the same way in every frame the service
/// sends, so the body is taken from the `01 08 00` every body begins with.
fn header(data: &[u8]) -> Option<(u8, u8, u16, &[u8])> {
    if data.len() < 10 {
        return None;
    }
    let flag = data[0];
    let transaction = data[1];
    let message = u16::from_be_bytes([data[2], data[3]]);
    let at = (4..data.len().min(12)).find(|&at| {
        data[at] == 0x01 && data.get(at + 1) == Some(&0x08) && data.get(at + 2) == Some(&0x00)
    })?;
    Some((flag, transaction, message, &data[at..]))
}

/// The TLVs of a body, whose lengths are little endian and whose second tag
/// counts its own two-byte length before the protobuf.
fn tlvs(body: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 3 <= body.len() {
        let tag = body[at];
        if tag == 0 {
            at += 1;
            continue;
        }
        let length = u16::from_le_bytes([body[at + 1], body[at + 2]]) as usize;
        let Some(value) = body.get(at + 3..at + 3 + length) else {
            break;
        };
        let value = if tag == 0x02 {
            // A protobuf is counted once more, two bytes inside the length; an
            // acknowledgement's four bytes of result and error are not.
            match value
                .get(..2)
                .map(|head| u16::from_le_bytes([head[0], head[1]]) as usize)
            {
                Some(inner) if inner + 2 == value.len() => &value[2..],
                _ => value,
            }
        } else {
            value
        };
        out.push((tag, value));
        at += 3 + length;
    }
    out
}

/// Whether a value reads as a name: letters, digits, and the separators these
/// names use.
fn printable(value: &[u8]) -> bool {
    !value.is_empty()
        && value
            .iter()
            .all(|byte| byte.is_ascii_graphic() || *byte == b' ' || *byte == b'_')
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
                let length =
                    varint_read(&mut data).ok_or_else(|| anyhow::anyhow!("short length"))? as usize;
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
    /// The reply the phone gave to a by-name query for `registry`.
    const LOOKUP: &str = "04010022005400010800460400000000000002460044000a1209abababababababab11abababababababab122e0d000300001141752ce5220000001a1e0a08726567697374727912120975221e70b441255e1159277f00a75427e1";
    /// The reply the phone gave when the hinge was turned on: a sample, not a
    /// status.
    const HINGE: &str = "041c0021004e0001080070040000000000000240003e000a1209a3b53fe2753556e11112624a343a91e37212280d01040000113d9d4677230000001a180a14000033439a9dd5ba0000008000000040000000001003";

    fn hex(data: &[u8]) -> String {
        data.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn bytes(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
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
    fn a_lookup_answer_names_the_uid_it_resolved() {
        let messages = messages(&bytes(LOOKUP));
        assert_eq!(messages.len(), 1);
        let message = &messages[0];
        assert_eq!(message.message, 768);
        let entries = entries(&message.payload);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0.as_deref(), Some("registry"));
        assert_eq!(uid_text(entries[0].1), "5e2541b4701e2275/e12754a7007f2759");
    }

    #[test]
    fn a_sample_reads_as_floats() {
        let messages = messages(&bytes(HINGE));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message, 1025);
        assert_eq!(
            uid_text(messages[0].suid),
            "e1563575e23fb5a3/72e3913a344a6212"
        );
        assert_eq!(messages[0].values[0], 179.0);
    }

    #[test]
    fn a_status_is_not_a_sample() {
        let refused = "040300210038000108007004000000000000022a0028000a120988ea9c0b4ec2463811aba0d340e8d2eb5812120d820000001121f1ca5f230000001a020803";
        let messages = messages(&bytes(refused));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message, 130);
        assert!(messages[0].values.is_empty());
    }

    #[test]
    fn a_uid_reads_back() {
        let suid = captured_uid();
        assert_eq!(parse_uid(&uid(suid)), Some(suid));
        assert_eq!(uid_text(suid), "3846c24e0b9cea88/58ebd2e840d3a0ab");
    }
}
