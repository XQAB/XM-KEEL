//! The sensor service as a client: ask it what it has, turn one on, read the
//! samples it sends.
//!
//! Everything here is the wire protocol in `frame` put on the socket in
//! `qrtr`. No Android framework, no sensor HAL, no daemon of its own is needed
//! once the sensors' domain is up on the ADSP.
//!
//! The three messages that carry all of it:
//!
//! * 512, by name: ask for a sensor's uid, or with an empty name for every uid
//!   the registry has.
//! * 514, on change: turn a sensor on, at a rate.
//! * 1025, a sample; 130, a status that says the sensor will not be turned on.

use anyhow::{bail, ensure, Context, Result};
use std::time::{Duration, Instant};

use crate::frame::{self, Answer, Message};
use crate::qrtr::{Transport, SSC};

/// The by-name message: resolve a name to a uid, or list every uid.
const BY_NAME: u32 = 512;
/// A configuration a continuous sensor takes.
const SENSOR_CONFIG: u32 = 513;
/// The configuration this device's sensors take.
const ON_CHANGE_CONFIG: u32 = 514;
/// A sample.
const SENSOR_EVENT: u32 = 1025;
/// A status: the sensor will not run, and this is why.
const SENSOR_STATUS: u32 = 130;
/// The batching hint a configuration carries: one millisecond.
const BATCH: [u8; 5] = [0x0a, 0x03, 0x08, 0xe8, 0x07];
/// The uid a by-name query is asked under; no sensor owns this one.
const NOBODY: [u8; 16] = [0xab; 16];
/// The sensors this project reads by name, so a listing can name them. SSC's
/// own listing carries uids only; the names are what a by-name query resolves.
const ROSTER: [&str; 9] = [
    "registry",
    "accel",
    "gravity",
    "ori",
    "gyro",
    "hinge_angle",
    "flip_status",
    "ambient_light",
    "device_orient",
];

/// A sensor the service knows, with the uid it answers under.
#[derive(Clone, Debug)]
pub struct Sensor {
    pub name: String,
    pub suid: [u8; 16],
}

impl Sensor {
    /// The uid as this project's notes write it.
    pub fn uid(&self) -> String {
        frame::uid_text(self.suid)
    }
}

/// A client of the sensor service on the ADSP.
pub struct Client {
    transport: Transport,
    node: u32,
    port: u32,
    transaction: u8,
}

/// What turning a sensor on came back with.
#[derive(Clone, Debug)]
pub struct Enabled {
    /// The first sample, which a sensor that runs sends with its answer.
    pub sample: Vec<f32>,
}

impl Client {
    /// Find the service through the kernel's name service and open a socket.
    pub fn open() -> Result<Self> {
        let transport = Transport::open()?;
        let (node, port) = transport.lookup(SSC).context("find the sensor service")?;
        Ok(Self {
            transport,
            node,
            port,
            transaction: 0,
        })
    }

    /// Where the service answers.
    pub fn server(&self) -> (u32, u32) {
        (self.node, self.port)
    }

    /// One request, sent under a uid, and the message that answers it.
    ///
    /// The trailing client id is a switch because it is not always wanted: a
    /// registry query is answered differently with it in some builds.
    pub fn ask(
        &mut self,
        suid: [u8; 16],
        inner: u32,
        payload: &[u8],
        extra: Option<&[u8]>,
        with_tail: bool,
    ) -> Result<Message> {
        self.transaction = self.transaction.wrapping_add(1);
        let transaction = self.transaction;
        let frame = frame::request(transaction, suid, inner, payload, extra, with_tail);
        self.transport
            .send(self.transport.address(self.node, self.port), &frame)?;
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut acknowledgement: Option<Answer> = None;
        while Instant::now() < deadline {
            let Some((_, data)) = self.transport.receive(Duration::from_millis(20))? else {
                continue;
            };
            if let Some(answer) = frame::answer(&data) {
                ensure!(
                    answer.accepted,
                    "the service refused message {inner}: error {}",
                    answer.error
                );
                acknowledgement = Some(answer);
                continue;
            }
            if let Some(message) = frame::messages(&data).into_iter().next() {
                return Ok(message);
            }
        }
        match acknowledgement {
            Some(_) => bail!("message {inner} was acknowledged but never answered"),
            None => bail!("no answer to message {inner}"),
        }
    }

    /// Every uid the registry has.
    ///
    /// An empty name is the whole listing; it comes back as one reply carrying
    /// every uid, without names. The few this project reads by name are then
    /// resolved so the listing can show them.
    pub fn list(&mut self) -> Result<Vec<Sensor>> {
        let query = by_name("", false);
        let answer = self.ask(
            NOBODY,
            BY_NAME,
            &[0x08, 0x01, 0x10, 0x00],
            Some(&query),
            true,
        )?;
        ensure!(
            answer.message == 768 || answer.message == BY_NAME,
            "the registry answered {} instead of a listing",
            answer.message
        );
        let mut sensors: Vec<Sensor> = frame::entries(&answer.payload)
            .into_iter()
            .map(|(name, suid)| Sensor {
                name: name.unwrap_or_default(),
                suid,
            })
            .collect();
        ensure!(!sensors.is_empty(), "the registry listed nothing");

        for name in ROSTER {
            for resolved in self.named(name)? {
                match sensors
                    .iter_mut()
                    .find(|sensor| sensor.suid == resolved.suid)
                {
                    Some(sensor) => sensor.name = resolved.name,
                    None => sensors.push(resolved),
                }
            }
        }
        Ok(sensors)
    }

    /// The sensors one name resolves to. A name can stand for two uids, as the
    /// hinge does: one per side.
    pub fn named(&mut self, name: &str) -> Result<Vec<Sensor>> {
        let query = by_name(name, true);
        let answer = self.ask(
            NOBODY,
            BY_NAME,
            &[0x08, 0x01, 0x10, 0x00],
            Some(&query),
            true,
        )?;
        let mut sensors = Vec::new();
        for (entry, suid) in frame::entries(&answer.payload) {
            if suid == [0; 16] {
                continue;
            }
            sensors.push(Sensor {
                name: entry.unwrap_or_else(|| name.to_owned()),
                suid,
            });
        }
        Ok(sensors)
    }

    /// The first sensor one name resolves to, or the second if `second` is set.
    pub fn find(&mut self, name: &str) -> Result<Sensor> {
        self.named(name)?
            .into_iter()
            .next()
            .with_context(|| format!("the registry has no sensor named {name}"))
    }

    /// Turn a sensor on, at a rate in hertz.
    ///
    /// A sensor that runs answers with its first sample; one whose attributes
    /// the registry is missing answers with a status instead, and that is the
    /// error that comes back here.
    pub fn enable(&mut self, suid: [u8; 16], rate_hz: u32) -> Result<Enabled> {
        let mut payload = vec![0x08, 0x01, 0x10, 0x01];
        payload.push(0x1d);
        payload.extend_from_slice(&rate_hz.to_le_bytes());
        let answer = self.ask(suid, ON_CHANGE_CONFIG, &payload, Some(&BATCH), true)?;
        if answer.message == SENSOR_STATUS {
            let code = frame::first_int(&answer.payload).unwrap_or_default();
            bail!(
                "the sensor {} refused to run: status {}, code {}",
                frame::uid_text(suid),
                SENSOR_STATUS,
                code
            );
        }
        ensure!(
            answer.message == SENSOR_EVENT,
            "the sensor answered {} instead of a sample",
            answer.message
        );
        Ok(Enabled {
            sample: answer.values,
        })
    }

    /// Turn a continuous sensor on at a rate in hertz.
    ///
    /// The rate goes into the configuration as a 32-bit float, in hertz, which
    /// is what the vendor HAL composes for a continuous sensor.
    pub fn enable_continuous(&mut self, suid: [u8; 16], rate_hz: f32) -> Result<Enabled> {
        let mut extra = Vec::with_capacity(7);
        extra.push(0x0a);
        extra.push(0x05);
        extra.push(0x0d);
        extra.extend_from_slice(&rate_hz.to_le_bytes());
        let answer = self.ask(
            suid,
            SENSOR_CONFIG,
            &[0x08, 0x01, 0x10, 0x01],
            Some(&extra),
            true,
        )?;
        if answer.message == SENSOR_STATUS {
            let code = frame::first_int(&answer.payload).unwrap_or_default();
            bail!(
                "the sensor {} refused to run: status {}, code {}",
                frame::uid_text(suid),
                SENSOR_STATUS,
                code
            );
        }
        Ok(Enabled {
            sample: answer.values,
        })
    }

    /// Wait for one frame, and read every message in it.
    pub fn events(&mut self, wait: Duration) -> Result<Vec<Message>> {
        let Some((_, data)) = self.transport.receive(wait)? else {
            return Ok(Vec::new());
        };
        Ok(frame::messages(&data))
    }

    /// Send a frame exactly as given and collect whatever comes back, so the
    /// wire can be looked at without guessing.
    pub fn exchange(&mut self, frame: &[u8], wait: Duration) -> Result<Vec<Vec<u8>>> {
        self.transport
            .send(self.transport.address(self.node, self.port), frame)?;
        let deadline = Instant::now() + wait;
        let mut frames = Vec::new();
        while Instant::now() < deadline {
            let Some((_, data)) = self.transport.receive(Duration::from_millis(50))? else {
                continue;
            };
            frames.push(data);
        }
        Ok(frames)
    }

    /// Turn a named sensor on and hand back a receiver for its samples.
    pub fn read(&mut self, name: &str, rate_hz: u32) -> Result<Receiver<'_>> {
        let sensor = self.find(name)?;
        let enabled = self
            .enable(sensor.suid, rate_hz)
            .with_context(|| format!("turn on {}", frame::uid_text(sensor.suid)))?;
        Ok(Receiver {
            client: self,
            sensor,
            rate: rate_hz,
            queued: Some(enabled.sample),
        })
    }
}

/// Reading one sensor's samples.
pub struct Receiver<'a> {
    client: &'a mut Client,
    sensor: Sensor,
    rate: u32,
    queued: Option<Vec<f32>>,
}

impl<'a> Receiver<'a> {
    /// The sensor being read.
    pub fn sensor(&self) -> &Sensor {
        &self.sensor
    }

    /// The next sample, waiting up to `wait` for it.
    pub fn next(&mut self, wait: Duration) -> Result<Option<Vec<f32>>> {
        if let Some(sample) = self.queued.take() {
            return Ok(Some(sample));
        }
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            for message in self.client.events(Duration::from_millis(50))? {
                if message.message != SENSOR_EVENT || message.values.is_empty() {
                    continue;
                }
                if message.suid != [0; 16] && message.suid != self.sensor.suid {
                    continue;
                }
                return Ok(Some(message.values));
            }
        }
        Ok(None)
    }

    /// Turn the sensor back off.
    pub fn stop(self) -> Result<()> {
        let mut payload = vec![0x08, 0x01, 0x10, 0x00];
        payload.push(0x1d);
        payload.extend_from_slice(&self.rate.to_le_bytes());
        self.client.ask(
            self.sensor.suid,
            ON_CHANGE_CONFIG,
            &payload,
            Some(&BATCH),
            true,
        )?;
        Ok(())
    }
}

/// The first sample of a named sensor, read at `rate_hz`.
pub fn sample(name: &str, rate_hz: u32, wait: Duration) -> Result<Vec<f32>> {
    let mut client = Client::open()?;
    let sensor = client.find(name)?;
    let enabled = client.enable(sensor.suid, rate_hz)?;
    let mut receiver = Receiver {
        client: &mut client,
        sensor,
        rate: rate_hz,
        queued: Some(enabled.sample),
    };
    let values = receiver
        .next(wait)?
        .with_context(|| format!("{name} sent no sample"))?;
    receiver.stop()?;
    Ok(values)
}

/// The hinge's angle in degrees, from the sensor that measures it.
pub fn hinge_angle() -> Result<f32> {
    let values = sample("hinge_angle", 10, Duration::from_secs(3))?;
    values
        .first()
        .copied()
        .context("hinge_angle sent an empty sample")
}

/// The light around the phone, in lux.
pub fn ambient_light() -> Result<f32> {
    let values = sample("ambient_light", 10, Duration::from_secs(3))?;
    values
        .first()
        .copied()
        .context("ambient_light sent an empty sample")
}

/// How far the phone is folded: the second value of the fold sensor's sample,
/// in degrees.
pub fn flip_status() -> Result<f32> {
    let values = sample("flip_status", 10, Duration::from_secs(3))?;
    values
        .get(1)
        .copied()
        .context("flip_status sent a short sample")
}

/// The second body a by-name query carries: the name, which kind of datatype
/// it is, and whether it should be wakeup. It is field 2 of the second body,
/// which is the one thing this took longest to get right: the name in field 1
/// is answered `result=1 error=7`, a policy error, on this device.
///
/// An empty name is the whole registry, and that one is asked with `wait`
/// clear: with it set the answer is the empty entry alone.
fn by_name(name: &str, wait: bool) -> Vec<u8> {
    let mut inner = Vec::new();
    field_bytes(&mut inner, 1, name.as_bytes());
    inner.extend_from_slice(&[0x10, if wait { 0x01 } else { 0x00 }, 0x18, 0x00]);
    let mut out = vec![0x12];
    let mut size = inner.len();
    while size >= 0x80 {
        out.push((size as u8 & 0x7f) | 0x80);
        size >>= 7;
    }
    out.push(size as u8);
    out.extend_from_slice(&inner);
    out
}

fn field_bytes(out: &mut Vec<u8>, tag: u8, value: &[u8]) {
    out.push(tag << 3 | 2);
    let mut size = value.len();
    while size >= 0x80 {
        out.push((size as u8 & 0x7f) | 0x80);
        size >>= 7;
    }
    out.push(size as u8);
    out.extend_from_slice(value);
}
