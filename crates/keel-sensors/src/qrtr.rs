//! The kernel's QRTR socket: how this device reaches the sensors' own service
//! on the ADSP without any Android framework in the way.
//!
//! Nothing else in the system has to run for this: the sockets are a kernel
//! feature, the name service is answered by the kernel's own dispatcher, and
//! the address it hands back points straight at the sensors' domain.

use anyhow::{bail, ensure, Context, Result};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

/// The socket family the kernel gives QRTR; libc has no name for it yet.
const AF_QRTR: i32 = 42;
/// Where every node answers questions about its services.
pub const CONTROL: u32 = 0xffff_fffe;
/// The sensors' service: Qualcomm's sensor service on the ADSP.
pub const SSC: u32 = 400;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Address {
    family: u16,
    node: u32,
    port: u32,
}

impl Address {
    fn at(node: u32, port: u32) -> Self {
        Self {
            family: AF_QRTR as u16,
            node,
            port,
        }
    }
}

/// One open QRTR socket, and where it answers from.
pub struct Transport {
    socket: OwnedFd,
    local: Address,
}

impl Transport {
    /// Open a socket. It does not bind: the first question gives it an address.
    pub fn open() -> Result<Self> {
        let fd = unsafe {
            libc::socket(
                AF_QRTR,
                libc::SOCK_DGRAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                0,
            )
        };
        ensure!(fd >= 0, "open QRTR socket: {}", io::Error::last_os_error());
        let socket = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut local = Address::default();
        let mut size = std::mem::size_of::<Address>() as libc::socklen_t;
        let got = unsafe { libc::getsockname(fd, (&mut local as *mut Address).cast(), &mut size) };
        ensure!(
            got == 0,
            "read the QRTR address: {}",
            io::Error::last_os_error()
        );
        Ok(Self { socket, local })
    }

    /// The node the kernel gave this socket.
    pub fn node(&self) -> u32 {
        self.local.node
    }

    /// The port the kernel gave this socket.
    pub fn port(&self) -> u32 {
        self.local.port
    }

    /// Ask the name service where a service lives. The answer is `(node, port)`.
    pub fn lookup(&self, service: u32) -> Result<(u32, u32)> {
        let mut question = Vec::with_capacity(20);
        for word in [0x0au32, service, 0, 0, 0] {
            question.extend_from_slice(&word.to_le_bytes());
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        // An unbound socket reads back as node zero until the kernel has given
        // it an address; this node is number one, so ask there.
        let node = if self.local.node == 0 {
            1
        } else {
            self.local.node
        };
        // The first send is what gives the socket an address; a receive before
        // it is answered with EADDRNOTAVAIL.
        self.send(Address::at(node, CONTROL), &question)?;
        let mut asked = Instant::now();
        while Instant::now() < deadline {
            if asked.elapsed() >= Duration::from_millis(250) {
                self.send(Address::at(node, CONTROL), &question)?;
                asked = Instant::now();
            }
            let Some((from, data)) = self.receive(Duration::from_millis(50))? else {
                continue;
            };
            if from.port != CONTROL {
                continue;
            }
            let words = words(&data).context("a name service reply is twenty bytes")?;
            if words[1] != service {
                continue;
            }
            match words[0] {
                4 if words[3] != 0 && words[4] != 0 => return Ok((words[3], words[4])),
                5 => bail!("the sensor service is not running"),
                _ => continue,
            }
        }
        bail!("the name service did not say where service {service} is")
    }

    /// Send one datagram to one address.
    pub(crate) fn send(&self, to: Address, data: &[u8]) -> Result<()> {
        let sent = unsafe {
            libc::sendto(
                self.socket.as_raw_fd(),
                data.as_ptr().cast(),
                data.len(),
                0,
                (&to as *const Address).cast(),
                std::mem::size_of::<Address>() as libc::socklen_t,
            )
        };
        ensure!(
            sent == data.len() as isize,
            "send to the sensor service: {}",
            io::Error::last_os_error()
        );
        Ok(())
    }

    /// Wait up to `wait` for one datagram.
    pub(crate) fn receive(&self, wait: Duration) -> Result<Option<(Address, Vec<u8>)>> {
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
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    io::ErrorKind::Interrupted => continue,
                    _ => return Err(error).context("receive from the sensor service"),
                }
            }
            return Ok(Some((from, buffer[..n as usize].to_vec())));
        }
    }

    /// An address on this node, at this port.
    pub(crate) fn address(&self, node: u32, port: u32) -> Address {
        Address::at(node, port)
    }
}

fn words(data: &[u8]) -> Option<[u32; 5]> {
    let words: Vec<u32> = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| u32::from_le_bytes(*v))
        .collect();
    (words.len() == 5 && data.len() == 20)
        .then(|| [words[0], words[1], words[2], words[3], words[4]])
}

/// Where a service lives, as `(node, port)`, on a socket of its own.
///
/// This is the kernel's name service: no Android service is involved, so it
/// works in recovery as soon as the ADSP is up.
pub fn lookup(service: u32) -> Result<(u32, u32)> {
    Transport::open()?.lookup(service)
}
