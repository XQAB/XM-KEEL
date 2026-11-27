//! A nonblocking Qualcomm SSC client, for asking the sensor service what it
//! has — used by `keelbin sns`, and nothing else.
//!
//! No Android framework and no sensor HAL is needed: the kernel's QRTR
//! sockets reach the sensors' protection domain on the ADSP directly, and only
//! the small part of protobuf the sensor service speaks is parsed here. The
//! `pd-mapper` chain has to be running, or the ADSP never loads that domain;
//! in recovery `crate::sensors::bring_up()` starts it.
//!
//! **This is a diagnostic now, not an orientation source.** Recovery has no
//! gravity to turn the pages by, and that is measured, not assumed: on bixi
//! (Xiaomi MIX Flip 2, SM8750) the accelerometer refuses every configuration
//! code the service accepts (514 acknowledged, then refused with code 5; 512
//! refused with `error 7`), `gravity` is refused with code 2, `gyro` with 3,
//! `orientation` with 6, `tilt` is acknowledged and never answers (0 samples),
//! `device_orient` produces one fixed `[4, 4, 0 …]` in ten minutes of turning
//! the phone, and `screen_down` only knows the phone lying face down. The kernel
//! has no way around it either: recovery carries no `i2c-dev.ko` and no
//! `spidev.ko`, and `/sys/class/sensors` does not exist.
//!
//! So nothing here decides how the pages are turned. The cover screen is
//! driven by the hinge (`keel_sensors::Hinge`) and by the touch driver's
//! `capfold_status`, and the 180° turn stays a switch on the settings page.

use anyhow::{bail, ensure, Context, Result};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

/// The socket family the kernel gives QRTR, not yet in libc.
const AF_QRTR: i32 = 42;
/// The name service every QRTR node answers on.
const CONTROL: u32 = 0xffff_fffe;
/// The sensors' service on the ADSP.
const SSC: u32 = 400;
/// The UID the sensor service answers lookups under.
const LOOKUP: [u64; 2] = [0xabab_abab_abab_abab; 2];
/// How phones name the sensor that reports acceleration. The names are still
/// asked (they are what `sns` looks for), but on bixi none of them can be
/// configured — see the module note.
pub const ACCEL_NAMES: [&str; 5] = ["accel", "accelerometer", "gravity", "linear_accel", "device_orientation"];

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Address {
    family: u16,
    node: u32,
    port: u32,
}

struct Connection {
    socket: OwnedFd,
    local: Address,
    server: Option<Address>,
    transaction: u16,
}

impl Connection {
    fn open() -> Result<Self> {
        let fd = unsafe { libc::socket(AF_QRTR, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, 0) };
        ensure!(fd >= 0, "open QRTR: {}", io::Error::last_os_error());
        let socket = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut local = Address::default();
        let mut size = std::mem::size_of::<Address>() as libc::socklen_t;
        let got = unsafe { libc::getsockname(fd, (&mut local as *mut Address).cast(), &mut size) };
        ensure!(got == 0, "read QRTR address: {}", io::Error::last_os_error());
        let connection = Self { socket, local, server: None, transaction: 0 };
        let lookup = [10u32, SSC, 0, 0, 0].into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>();
        connection.send(Address { family: AF_QRTR as u16, node: local.node, port: CONTROL }, &lookup)?;
        Ok(connection)
    }

    fn send(&self, address: Address, data: &[u8]) -> Result<()> {
        let sent = unsafe {
            libc::sendto(
                self.socket.as_raw_fd(),
                data.as_ptr().cast(),
                data.len(),
                0,
                (&address as *const Address).cast(),
                std::mem::size_of::<Address>() as libc::socklen_t,
            )
        };
        ensure!(sent == data.len() as isize, "send SSC request: {}", io::Error::last_os_error());
        Ok(())
    }

    fn request(&mut self, uid: [u64; 2], message: u32, payload: &[u8]) -> Result<()> {
        self.transaction = self.transaction.wrapping_add(1);
        self.send(self.server.context("SSC has no address")?, &request(self.transaction, uid, message, payload))
    }

    fn send_full(
        &mut self,
        uid: [u64; 2],
        message: u32,
        payload: &[u8],
        extra: Option<&[u8]>,
        with_tail: bool,
    ) -> Result<()> {
        self.transaction = self.transaction.wrapping_add(1);
        self.send(
            self.server.context("SSC has no address")?,
            &request_full(self.transaction as u8, uid, message, payload, extra, with_tail),
        )
    }
}

// Only the small subset of protobuf the sensor service uses is parsed here.
// Every length and wire type is checked before slicing, and unknown fields
// are skipped, so a malformed message cannot panic the UI.

fn varint(data: &mut &[u8]) -> Result<u64> {
    let mut result = 0;
    for shift in (0..70).step_by(7) {
        let (&byte, rest) = data.split_first().context("short protobuf varint")?;
        *data = rest;
        ensure!(shift < 63 || byte <= 1, "protobuf varint overflow");
        result |= (byte as u64 & 127) << shift;
        if byte & 128 == 0 {
            return Ok(result);
        }
    }
    bail!("protobuf varint overflow")
}

fn fields(mut data: &[u8]) -> Result<Vec<(u64, u8, &[u8])>> {
    let mut result = Vec::new();
    while !data.is_empty() {
        let key = varint(&mut data)?;
        ensure!(key >> 3 != 0, "protobuf field zero");
        let wire = (key & 7) as u8;
        let size = match wire {
            0 => {
                let before = data;
                varint(&mut data)?;
                result.push((key >> 3, wire, &before[..before.len() - data.len()]));
                continue;
            }
            1 => 8,
            2 => usize::try_from(varint(&mut data)?)?,
            5 => 4,
            _ => bail!("unsupported protobuf wire type {wire}"),
        };
        ensure!(size <= data.len(), "short protobuf field");
        let (value, rest) = data.split_at(size);
        result.push((key >> 3, wire, value));
        data = rest;
    }
    Ok(result)
}

fn sensor_uid(data: &[u8]) -> Result<[u64; 2]> {
    let fields = fields(data)?;
    let word = |tag| {
        fields
            .iter()
            .find(|&&(n, w, _)| n == tag && w == 1)
            .map(|&(_, _, v)| u64::from_le_bytes(v.try_into().unwrap()))
            .context("missing sensor UID")
    };
    Ok([word(1)?, word(2)?])
}

type SensorEvent<'a> = ([u64; 2], u32, &'a [u8]);

fn events(data: &[u8]) -> Result<Vec<SensorEvent<'_>>> {
    ensure!(data.len() >= 7, "short QMI header");
    let size = u16::from_le_bytes([data[5], data[6]]) as usize;
    ensure!(data.len() == size + 7, "short QMI message");
    let mut tlvs = &data[7..];
    let mut result = Vec::new();
    while !tlvs.is_empty() {
        ensure!(tlvs.len() >= 3, "short QMI TLV");
        let tag = tlvs[0];
        let size = u16::from_le_bytes([tlvs[1], tlvs[2]]) as usize;
        ensure!(size <= tlvs.len() - 3, "short QMI TLV value");
        let value = &tlvs[3..size + 3];
        tlvs = &tlvs[size + 3..];
        if data[0] == 2 && tag == 2 {
            ensure!(value == [0, 0, 0, 0], "the sensor service rejected a request: {value:?}");
        }
        let message = u16::from_le_bytes([data[3], data[4]]);
        if data[0] != 4 || ![0x21, 0x22].contains(&message) || tag != 2 {
            continue;
        }
        ensure!(value.len() >= 2, "missing SSC payload length");
        let size = u16::from_le_bytes([value[0], value[1]]) as usize;
        ensure!(size == value.len() - 2, "short SSC payload");
        let envelope = fields(&value[2..])?;
        let uid = envelope.iter().find(|&&(t, w, _)| t == 1 && w == 2).context("missing SSC UID")?;
        let uid = sensor_uid(uid.2)?;
        for &(_, _, event) in envelope.iter().filter(|&&(t, w, _)| t == 2 && w == 2) {
            let fields = fields(event)?;
            let message = fields.iter().find(|&&(t, w, _)| t == 1 && w == 5).context("missing SSC event ID")?;
            let payload = fields.iter().find(|&&(t, w, _)| t == 3 && w == 2).context("missing SSC event payload")?;
            result.push((uid, u32::from_le_bytes(message.2.try_into().unwrap()), payload.2));
        }
    }
    Ok(result)
}

/// The integer value of one attribute, read off an attributes reply.
fn attribute_int(data: &[u8], wanted: u32) -> Option<u64> {
    let parsed = fields(data).ok()?;
    for (tag, wire, value) in parsed {
        if tag != 1 || wire != 2 {
            continue;
        }
        let Ok(inner) = fields(value) else { continue };
        let Some(id) = inner
            .iter()
            .find(|&&(tag, wire, _)| tag == 1 && matches!(wire, 0 | 5))
            .and_then(|&(_, wire, raw)| {
                if wire == 0 {
                    let mut rest = raw;
                    varint(&mut rest).ok()
                } else {
                    raw.get(..4)
                        .map(|head| u32::from_le_bytes(head.try_into().unwrap()) as u64)
                }
            })
        else {
            continue;
        };
        if id != u64::from(wanted) {
            continue;
        }
        // The value of an attribute is wrapped twice: the reply writes
        // `0a <len> 08 <id> 12 <len> 0a <len> 21 <value>`, so the number sits
        // one level below the field that carries it.
        return inner
            .iter()
            .find(|&&(tag, _, _)| tag == 2)
            .and_then(|&(_, wire, raw)| number(raw, wire));
    }
    None
}

/// A number an attribute carries, however many wrappers it sits behind.
///
/// The reply nests a count as `0a <len> 08 <id> 12 <len> 0a <len> 21 <n>`,
/// so each layer is unwrapped until a number is reached.
fn number(raw: &[u8], wire: u8) -> Option<u64> {
    match wire {
        0 => {
            let mut rest = raw;
            varint(&mut rest).ok()
        }
        1 => raw
            .get(..8)
            .map(|bytes| u64::from_le_bytes(bytes.try_into().unwrap())),
        2 => fields(raw)
            .ok()?
            .first()
            .and_then(|&(_, wire, value)| number(value, wire)),
        5 => raw
            .get(..4)
            .map(|head| u32::from_le_bytes(head.try_into().unwrap()) as u64),
        _ => None,
    }
}

/// The first integer a payload holds, which a refusal's code is.
fn first_int(data: &[u8]) -> Option<u64> {
    let parsed = fields(data).ok()?;
    let &(_, wire, value) = parsed.iter().find(|&&(tag, _, _)| tag == 1)?;
    match wire {
        0 => {
            let mut rest = value;
            varint(&mut rest).ok()
        }
        5 => value
            .get(..4)
            .map(|head| u32::from_le_bytes(head.try_into().unwrap()) as u64),
        _ => None,
    }
}

/// Every float a sample carries, in order.
fn floats(data: &[u8]) -> Vec<f32> {
    let Ok(parsed) = fields(data) else { return Vec::new() };
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

fn bytes(out: &mut Vec<u8>, tag: u8, value: &[u8]) {
    out.push(tag * 8 + 2);
    let mut size = value.len();
    while size >= 128 {
        out.push((size as u8 & 127) | 128);
        size >>= 7;
    }
    out.push(size as u8);
    out.extend_from_slice(value);
}

fn request(transaction: u16, uid: [u64; 2], message: u32, payload: &[u8]) -> Vec<u8> {
    let mut id = vec![9];
    id.extend_from_slice(&uid[0].to_le_bytes());
    id.push(17);
    id.extend_from_slice(&uid[1].to_le_bytes());
    let mut envelope = Vec::new();
    bytes(&mut envelope, 1, &id);
    envelope.push(0x15);
    envelope.extend_from_slice(&message.to_le_bytes());
    bytes(&mut envelope, 3, &[8, 1, 16, 1]); // APSS, non-wakeup delivery
    let mut body = Vec::new();
    bytes(&mut body, 2, payload);
    bytes(&mut envelope, 4, &body);
    let mut qmi = vec![0];
    qmi.extend_from_slice(&transaction.to_le_bytes());
    qmi.extend_from_slice(&0x20u16.to_le_bytes());
    qmi.extend_from_slice(&(envelope.len() as u16 + 9).to_le_bytes());
    qmi.push(1);
    qmi.extend_from_slice(&(envelope.len() as u16 + 2).to_le_bytes());
    qmi.extend_from_slice(&(envelope.len() as u16).to_le_bytes());
    qmi.extend_from_slice(&envelope);
    qmi.extend_from_slice(&[0x10, 1, 0, 1]); // large indications
    qmi
}

/// Build a request the way the service expects its configuration and
/// attribute messages: the payload in its own field, an optional second
/// body, and the client id only where the message takes one. The lookup
/// path above predates this; the service answers it anyway, so it stays.
fn request_full(
    transaction: u8,
    uid: [u64; 2],
    message: u32,
    payload: &[u8],
    extra: Option<&[u8]>,
    with_tail: bool,
) -> Vec<u8> {
    let mut id = vec![9];
    id.extend_from_slice(&uid[0].to_le_bytes());
    id.push(17);
    id.extend_from_slice(&uid[1].to_le_bytes());
    let mut body = Vec::new();
    bytes(&mut body, 1, &id);
    body.push(0x15);
    body.extend_from_slice(&message.to_le_bytes());
    if !payload.is_empty() {
        bytes(&mut body, 3, payload);
    }
    if let Some(extra) = extra {
        bytes(&mut body, 4, extra);
    }
    let tail = if with_tail { 4 } else { 0 };
    let length = 1 + 4 + body.len() + tail;
    let mut qmi = vec![0];
    qmi.push(transaction);
    qmi.extend_from_slice(&0x20u16.to_be_bytes());
    qmi.extend_from_slice(&(length as u16).to_be_bytes());
    qmi.extend_from_slice(&[0, 1]);
    qmi.extend_from_slice(&((body.len() + 2) as u16).to_le_bytes());
    qmi.extend_from_slice(&(body.len() as u16).to_le_bytes());
    qmi.extend_from_slice(&body);
    if with_tail {
        qmi.extend_from_slice(&[0x10, 0x01, 0x00, 0x01]); // a client id of one
    }
    qmi
}

// The probe below is the same client without the UI: it prints what the
// service says instead of steering the pages, and it is what tells us where
// a phone's sensor stack actually is.

impl Connection {
    /// Wait for one frame, up to `wait`.
    fn receive(&self, wait: Duration) -> Result<Option<(Address, Vec<u8>)>> {
        let deadline = Instant::now() + wait;
        loop {
            let mut buffer = [0u8; 65536];
            let mut from = Address::default();
            let mut size = std::mem::size_of::<Address>() as libc::socklen_t;
            let n = unsafe {
                libc::recvfrom(
                    self.socket.as_raw_fd(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    0,
                    (&mut from as *mut Address).cast(),
                    &mut size,
                )
            };
            if n < 0 {
                let error = io::Error::last_os_error();
                match error.kind() {
                    io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return Ok(None);
                        }
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    io::ErrorKind::Interrupted => continue,
                    _ => return Err(error).context("receive SSC data"),
                }
            }
            return Ok(Some((from, buffer[..n as usize].to_vec())));
        }
    }
}

/// Ask the sensor service what it has under these names, then watch the sensor
/// that answered report. Everything goes over the kernel's QRTR socket.
///
/// A sensor that only reports on change (attribute 16 set) refuses the
/// periodic configuration, so it is asked with the on-change one instead.
pub fn probe(names: &[String], rate: f32, seconds: u64, raw: bool) -> Result<()> {
    let mut connection = Connection::open()?;
    println!("qrtr: local node {}, port {}", connection.local.node, connection.local.port);

    // The name service knows where the sensors' service is; if it says nothing,
    // the address this phone is known to use is tried before giving up.
    let deadline = Instant::now() + Duration::from_secs(3);
    while connection.server.is_none() && Instant::now() < deadline {
        let Some((from, data)) = connection.receive(Duration::from_millis(100))? else { continue };
        if from.port == CONTROL {
            if let Some(words) = control(&data) {
                println!(
                    "control: cmd {} service {} instance {} node {} port {}",
                    words[0], words[1], words[2], words[3], words[4]
                );
                if words[1] == SSC && words[3] != 0 && words[4] != 0 {
                    connection.server = Some(Address { family: AF_QRTR as u16, node: words[3], port: words[4] });
                }
            }
        }
    }
    let server = match connection.server {
        Some(server) => server,
        None => {
            let fallback = Address { family: AF_QRTR as u16, node: 5, port: 18 };
            println!("ssc: the name service said nothing, trying node 5 port 18");
            connection.server = Some(fallback);
            fallback
        }
    };
    println!("ssc: node {}, port {}", server.node, server.port);

    // Every name is asked in the same request to the lookup service, whose own
    // UID is the 0xabab one; the reply carries the UID the sensor really has.
    for name in names {
        let mut query = Vec::new();
        bytes(&mut query, 1, name.as_bytes());
        query.extend_from_slice(&[0x10, 1, 0x18, 1]);
        connection.request(LOOKUP, 512, &query)?;
    }
    println!("asked: {} names", names.len());

    let mut sensor = None;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        let Some((from, data)) = connection.receive(Duration::from_millis(100))? else { continue };
        if from.port == CONTROL {
            if let Some(words) = control(&data) {
                println!(
                    "control: cmd {} service {} instance {} node {} port {}",
                    words[0], words[1], words[2], words[3], words[4]
                );
            }
            continue;
        }
        if raw {
            println!("frame: {}", hex(&data));
        }
        let messages = match events(&data) {
            Ok(messages) => messages,
            Err(e) => {
                println!("frame: {} bytes, not parsed: {e:#}", data.len());
                continue;
            }
        };
        for (uid, message, payload) in messages {
            if uid == LOOKUP && message == 768 {
                match lookup_reply(payload) {
                    Ok((names, uid)) => {
                        println!("reply: names {names:?} uid {}", uid.map(uid_text).unwrap_or_else(|| "none".into()));
                        if sensor.is_none() && uid.is_some() {
                            sensor = uid;
                        }
                    }
                    Err(e) => println!("reply: not parsed: {e:#}"),
                }
            } else {
                println!("message: uid {} message {message}, {} bytes", uid_text(uid), payload.len());
            }
        }
    }

    let Some(wanted) = sensor else {
        println!("none: no sensor UID came back for those names");
        return Ok(());
    };
    println!("sensor: uid {}, asking for {rate} reports per second", uid_text(wanted));

    // Attribute 16 says whether the sensor only reports on change. Those
    // refuse the periodic configuration and are asked with the on-change
    // one, whose rate is a 32-bit integer rather than the float 513 takes.
    const BATCH: [u8; 5] = [0x0a, 0x03, 0x08, 0xe8, 0x07];
    connection.send_full(wanted, 1, &[0x08, 0x01, 0x10, 0x00], Some(&[]), false)?;
    let mut on_change = false;
    let asking = Instant::now() + Duration::from_secs(2);
    while Instant::now() < asking {
        let Some((from, data)) = connection.receive(Duration::from_millis(100))? else { continue };
        if from.port == CONTROL {
            continue;
        }
        if raw {
            println!("frame: {}", hex(&data));
        }
        let Ok(messages) = events(&data) else { continue };
        let mut heard = false;
        for (uid, message, payload) in messages {
            if uid != wanted {
                continue;
            }
            if message == 130 {
                println!("status: refused, code {}", first_int(payload).unwrap_or_default());
                heard = true;
                continue;
            }
            if message != 128 {
                continue;
            }
            match attribute_int(payload, 16) {
                Some(value) => {
                    on_change = value == 1;
                    println!("attr 16: {value}");
                }
                None => println!("attr 16: not reported"),
            }
            heard = true;
        }
        if heard {
            break;
        }
    }
    if on_change {
        let mut payload = vec![0x08, 0x01, 0x10, 0x01, 0x1d];
        payload.extend_from_slice(&(rate as u32).to_le_bytes());
        connection.send_full(wanted, 514, &payload, Some(&BATCH), true)?;
    } else {
        let mut extra = vec![0x0a, 0x05, 0x0d];
        extra.extend_from_slice(&rate.to_le_bytes());
        connection.send_full(wanted, 513, &[0x08, 0x01, 0x10, 0x01], Some(&extra), true)?;
    }

    let deadline = Instant::now() + Duration::from_secs(seconds + 2);
    let mut samples = 0u32;
    while Instant::now() < deadline && samples < 2048 {
        let Some((from, data)) = connection.receive(Duration::from_millis(100))? else { continue };
        if from.port == CONTROL {
            continue;
        }
        if raw {
            println!("frame: {}", hex(&data));
        }
        let Ok(messages) = events(&data) else { continue };
        for (uid, message, payload) in messages {
        if uid != wanted {
            continue;
        }
        if message == 130 {
            println!("status: refused, code {}", first_int(payload).unwrap_or_default());
            continue;
        }
        if message != 1025 {
            continue;
        }
        let values = floats(payload);
        samples += 1;
        if values.len() == 3 {
            println!("accel: {:>8.2} {:>8.2} {:>8.2}", values[0], values[1], values[2]);
        } else {
            println!("sample: {values:?}");
        }
        }
    }
    println!("end: {samples} samples");
    Ok(())
}

/// The five words of a name service packet, when this frame is one.
fn control(data: &[u8]) -> Option<[u32; 5]> {
    if data.len() != 20 {
        return None;
    }
    let words: Vec<_> = data.as_chunks::<4>().0.iter().map(|v| u32::from_le_bytes(*v)).collect();
    Some([words[0], words[1], words[2], words[3], words[4]])
}

/// The names and the sensor UID a lookup reply carries.
fn lookup_reply(data: &[u8]) -> Result<(Vec<String>, Option<[u64; 2]>)> {
    let fields = fields(data)?;
    let names = fields
        .iter()
        .filter(|&&(tag, wire, _)| tag == 1 && wire == 2)
        .map(|&(_, _, value)| String::from_utf8_lossy(value).into_owned())
        .collect();
    let uid = fields
        .iter()
        .find_map(|&(tag, wire, value)| (tag == 2 && wire == 2).then(|| sensor_uid(value).ok()).flatten());
    Ok((names, uid))
}

fn uid_text(uid: [u64; 2]) -> String {
    format!("{:016x}:{:016x}", uid[0], uid[1])
}

fn hex(data: &[u8]) -> String {
    let pairs: Vec<_> = data.iter().map(|b| format!("{b:02x}")).collect();
    pairs.join("")
}
