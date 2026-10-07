//! Framing of handshake messages and encrypted records over TCP: `u32 big-endian length | bytes`.
use std::io::{Read, Write};
use std::net::TcpStream;

use nexdesk_crypto::{Opener, Sealer};

use crate::wire::Msg;
use crate::PeerError;

/// Before authentication only small frames are accepted (the biggest handshake message is about 6.5 KB).
pub const MAX_PRE_AUTH: usize = 16 * 1024;
/// After authentication: one tile plus overhead.
pub const MAX_FRAME: usize = nexdesk_crypto::MAX_RECORD + 64;
const AAD: &[u8] = b"nexdesk-peer-v1";

pub fn write_frame(s: &mut impl Write, data: &[u8]) -> Result<(), PeerError> {
    s.write_all(&(data.len() as u32).to_be_bytes())?;
    s.write_all(data)?;
    s.flush()?;
    Ok(())
}

pub fn read_frame(s: &mut impl Read, max: usize) -> Result<Vec<u8>, PeerError> {
    let mut len = [0u8; 4];
    match s.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(PeerError::Closed),
        Err(e) => return Err(e.into()),
    }
    let n = u32::from_be_bytes(len) as usize;
    if n > max {
        return Err(PeerError::Proto("frame too large"));
    }
    let mut buf = vec![0u8; n];
    s.read_exact(&mut buf)?;
    Ok(buf)
}

/// Receiving side of an authenticated connection.
pub struct Reader {
    pub(crate) stream: TcpStream,
    pub(crate) opener: Opener,
}

/// Sending side of an authenticated connection.
pub struct Writer {
    pub(crate) stream: TcpStream,
    pub(crate) sealer: Sealer,
}

impl Reader {
    pub fn recv(&mut self) -> Result<Msg, PeerError> {
        let rec = read_frame(&mut self.stream, MAX_FRAME)?;
        let plain = self.opener.open(&rec, AAD)?;
        Msg::decode(&plain)
    }
}

impl Writer {
    pub fn send(&mut self, m: &Msg) -> Result<(), PeerError> {
        let rec = self.sealer.seal(&m.encode(), AAD)?;
        write_frame(&mut self.stream, &rec)
    }

    /// Close both directions (wakes a reader blocked in another thread).
    pub fn shutdown(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}
