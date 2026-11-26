//! The sensor service as a client: ask it what it has, turn one on, read the
//! samples it sends.
//!
//! Everything here is the wire protocol in `frame` put on the socket in
//! `qrtr`. No Android framework, no sensor HAL, no daemon of its own is needed
//! once the sensors' domain is up on the ADSP.

use anyhow::{ensure, Context, Result};
use std::time::{Duration, Instant};

use crate::frame::{self, Answer, Event};
use crate::qrtr::{Transport, SSC};

/// The by-name message the registry answers a listing under.
const BY_NAME: u32 = 512;
/// The configuration a continuous sensor takes.
const SENSOR_CONFIG: u32 = 513;
/// The configuration every sensor that reports on this device accepts.
const ON_CHANGE_CONFIG: u32 = 514;
/// A sample.
const SENSOR_EVENT: u32 = 1025;
/// Where the listing lives, as a name.
const REGISTRY_NAME: &str = "registry";
/// The batching hint a configuration carries: one millisecond.
const BATCH: [u8; 5] = [0x0a, 0x03, 0x08, 0xe8, 0x07];
/// The uid a by-name query is asked under; no sensor owns this one.
const NOBODY: [u8; 16] = [0xab; 16];

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

impl Client {
    /// Find the service through the kernel's name service and open a socket.
    pub fn open() -> Result<Self> {
        let transport = Transport::open()?;
        let (node, port) = transport.lookup(SSC).context("find the sensor service")?;
        Ok(Self { transport, node, port, transaction: 0 })
    }

    /// Where the service answers.
    pub fn server(&self) -> (u32, u32) {
        (self.node, self.port)
    }

    /// One request, sent under a uid, and the answer to it.
    ///
    /// The trailing client id is a switch because it is not always wanted: a
    /// registry listing that carries it is answered `INVALID_TYPE`.
    pub fn ask(&mut self, suid: [u8; 16], inner: u32, payload: &[u8], extra: Option<&[u8]>, with_tail: bool) -> Result<Answer> {
        self.transaction = self.transaction.wrapping_add(1);
        let transaction = self.transaction;
        let frame = frame::request(transaction, suid, inner, payload, extra, with_tail);
        self.transport.send(self.transport.address(self.node, self.port), &frame)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let Some((_, data)) = self.transport.receive(Duration::from_millis(20))? else { continue };
            let Some(answer) = frame::answer(&data) else { continue };
            if answer.transaction == transaction {
                return Ok(answer);
            }
        }
        anyhow::bail!("no answer to message {inner}")
    }

    /// Every sensor the registry lists.
    ///
    /// The listing is a by-name query of `registry`, and it goes without the
    /// trailing client id.
    pub fn list(&mut self) -> Result<Vec<Sensor>> {
        let mut extra = Vec::new();
        field_bytes(&mut extra, 1, REGISTRY_NAME.as_bytes());
        let answer = self.ask(NOBODY, BY_NAME, &[0x08, 0x01, 0x10, 0x01], Some(&extra), false)?;
        ensure!(answer.accepted, "the registry refused the listing: error {}", answer.error);

        let mut sensors: Vec<Sensor> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let Some((_, data)) = self.transport.receive(Duration::from_millis(50))? else { continue };
            for event in frame::events(&data) {
                for (name, suid) in frame::names(&event.payload) {
                    if sensors.iter().all(|s| s.name != name) {
                        sensors.push(Sensor { name, suid });
                    }
                }
            }
            if !sensors.is_empty() {
                // One more short wait, so a listing split across frames is whole.
                if self.transport.receive(Duration::from_millis(30))?.is_none() {
                    break;
                }
            }
        }
        Ok(sensors)
    }

    /// Find one sensor by the name the service lists it under.
    pub fn find(&mut self, name: &str) -> Result<Sensor> {
        self.list()?
            .into_iter()
            .find(|s| s.name == name || s.name.eq_ignore_ascii_case(name))
            .with_context(|| format!("the registry has no sensor named {name}"))
    }

    /// Turn a sensor on, at a rate in hertz, with the configuration every
    /// sensor on this device that reports at all accepts.
    pub fn enable(&mut self, suid: [u8; 16], rate_hz: u32) -> Result<Answer> {
        let mut payload = vec![0x08, 0x01, 0x10, 0x01];
        payload.push(0x1d);
        payload.extend_from_slice(&rate_hz.to_le_bytes());
        let answer = self.ask(suid, ON_CHANGE_CONFIG, &payload, Some(&BATCH), true)?;
        ensure!(answer.accepted, "the sensor refused the configuration: error {}", answer.error);
        Ok(answer)
    }

    /// Turn a continuous sensor on at a rate in hertz.
    ///
    /// The rate goes into the configuration as a 32-bit float, in hertz, which
    /// is what the vendor HAL composes for a continuous sensor. This device's
    /// accelerometer answers it with an error while its registry entry is
    /// missing, so it is kept apart from `enable` on purpose.
    pub fn enable_continuous(&mut self, suid: [u8; 16], rate_hz: f32) -> Result<Answer> {
        let mut extra = Vec::with_capacity(7);
        extra.push(0x0a);
        extra.push(0x05);
        extra.push(0x0d);
        extra.extend_from_slice(&rate_hz.to_le_bytes());
        let answer = self.ask(suid, SENSOR_CONFIG, &[0x08, 0x01, 0x10, 0x01], Some(&extra), true)?;
        ensure!(answer.accepted, "the sensor refused the configuration: error {}", answer.error);
        Ok(answer)
    }

    /// Wait for one frame, and read every event in it.
    pub fn events(&mut self, wait: Duration) -> Result<Vec<Event>> {
        let Some((_, data)) = self.transport.receive(wait)? else { return Ok(Vec::new()) };
        Ok(frame::events(&data))
    }

    /// Turn a named sensor on and hand back a receiver for its samples.
    pub fn read(&mut self, name: &str, rate_hz: u32) -> Result<Receiver<'_>> {
        let sensor = self.find(name)?;
        self.enable(sensor.suid, rate_hz)?;
        Ok(Receiver { client: self, sensor, rate: rate_hz })
    }
}

/// Reading one sensor's samples.
pub struct Receiver<'a> {
    client: &'a mut Client,
    sensor: Sensor,
    rate: u32,
}

impl<'a> Receiver<'a> {
    /// The sensor being read.
    pub fn sensor(&self) -> &Sensor {
        &self.sensor
    }

    /// The next sample, waiting up to `wait` for it.
    pub fn next(&mut self, wait: Duration) -> Result<Option<Vec<f32>>> {
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            for event in self.client.events(Duration::from_millis(50))? {
                if event.suid != self.sensor.suid || event.message != SENSOR_EVENT {
                    continue;
                }
                if !event.values.is_empty() {
                    return Ok(Some(event.values));
                }
            }
        }
        Ok(None)
    }

    /// Turn the sensor back off.
    pub fn stop(self) -> Result<()> {
        let mut payload = vec![0x08, 0x01, 0x10, 0x00];
        payload.push(0x1d);
        payload.extend_from_slice(&self.rate.to_le_bytes());
        self.client.ask(self.sensor.suid, ON_CHANGE_CONFIG, &payload, Some(&BATCH), true)?;
        Ok(())
    }
}

/// The first sample of a named sensor, read at `rate_hz`.
pub fn sample(name: &str, rate_hz: u32, wait: Duration) -> Result<Vec<f32>> {
    let mut client = Client::open()?;
    let mut receiver = client.read(name, rate_hz)?;
    let values = receiver.next(wait)?.with_context(|| format!("{name} sent no sample"))?;
    receiver.stop()?;
    Ok(values)
}

/// The hinge's angle in degrees, from the sensor that measures it.
pub fn hinge_angle() -> Result<f32> {
    let values = sample("hinge_angle", 10, Duration::from_secs(3))?;
    values.first().copied().context("hinge_angle sent an empty sample")
}

/// The light around the phone, in lux.
pub fn ambient_light() -> Result<f32> {
    let values = sample("ambient_light", 10, Duration::from_secs(3))?;
    values.first().copied().context("ambient_light sent an empty sample")
}

/// How far the phone is folded: the second value of the fold sensor's sample,
/// in degrees.
pub fn flip_status() -> Result<f32> {
    let values = sample("flip_status", 10, Duration::from_secs(3))?;
    values.get(1).copied().context("flip_status sent a short sample")
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
