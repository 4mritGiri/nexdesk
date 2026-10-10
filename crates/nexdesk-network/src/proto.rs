//! Control messages between agent / viewer and the relay. Tiny, length-prefixed, strictly validated.
//! After `Paired` the connection carries opaque bytes (the end-to-end encrypted session).
use std::io::{Read, Write};

use crate::NetError;

/// Control frames are tiny; anything bigger is hostile.
pub const MAX_FRAME: usize = 64;
pub const ID_LEN: usize = 9;
pub const TOKEN_LEN: usize = 16;

pub type Token = [u8; TOKEN_LEN];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ctl {
    /// agent -> relay: "I am reachable under this ID"
    Register(String),
    Registered,
    Taken,
    /// agent -> relay, every 15 s
    Heartbeat,
    /// relay -> agent: a viewer waits; open a second connection and `Accept` the token
    Incoming(Token),
    /// agent -> relay on the second connection
    Accept(Token),
    /// viewer -> relay
    Connect(String),
    NotFound,
    /// relay -> both: from now on raw bytes
    Paired,
    /// relay -> anyone: limit reached or request invalid
    Refused,
}

const T_REGISTER: u8 = 1;
const T_REGISTERED: u8 = 2;
const T_TAKEN: u8 = 3;
const T_HEARTBEAT: u8 = 4;
const T_INCOMING: u8 = 5;
const T_ACCEPT: u8 = 6;
const T_CONNECT: u8 = 7;
const T_NOTFOUND: u8 = 8;
const T_PAIRED: u8 = 9;
const T_REFUSED: u8 = 10;

/// Exactly nine ASCII digits.
pub fn valid_id(id: &str) -> bool {
    id.len() == ID_LEN && id.bytes().all(|b| b.is_ascii_digit())
}

/// A random ID, never starting with 0.
pub fn random_id() -> Result<String, NetError> {
    let mut b = [0u8; 8];
    getrandom::getrandom(&mut b).map_err(|_| NetError::Proto("no random numbers available"))?;
    let n = u64::from_le_bytes(b) % 900_000_000 + 100_000_000;
    Ok(n.to_string())
}

pub fn random_token() -> Result<Token, NetError> {
    let mut t = [0u8; TOKEN_LEN];
    getrandom::getrandom(&mut t).map_err(|_| NetError::Proto("no random numbers available"))?;
    Ok(t)
}

impl Ctl {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Ctl::Register(id) => [&[T_REGISTER][..], id.as_bytes()].concat(),
            Ctl::Registered => vec![T_REGISTERED],
            Ctl::Taken => vec![T_TAKEN],
            Ctl::Heartbeat => vec![T_HEARTBEAT],
            Ctl::Incoming(t) => [&[T_INCOMING][..], t].concat(),
            Ctl::Accept(t) => [&[T_ACCEPT][..], t].concat(),
            Ctl::Connect(id) => [&[T_CONNECT][..], id.as_bytes()].concat(),
            Ctl::NotFound => vec![T_NOTFOUND],
            Ctl::Paired => vec![T_PAIRED],
            Ctl::Refused => vec![T_REFUSED],
        }
    }

    pub fn decode(b: &[u8]) -> Result<Ctl, NetError> {
        let bad = |m| NetError::Proto(m);
        let (&tag, rest) = b.split_first().ok_or_else(|| bad("empty frame"))?;
        let id = |r: &[u8]| -> Result<String, NetError> {
            let s = std::str::from_utf8(r).map_err(|_| bad("bad id"))?;
            if valid_id(s) {
                Ok(s.to_string())
            } else {
                Err(bad("bad id"))
            }
        };
        let token =
            |r: &[u8]| -> Result<Token, NetError> { r.try_into().map_err(|_| bad("bad token")) };
        let empty = |r: &[u8], m: Ctl| {
            if r.is_empty() {
                Ok(m)
            } else {
                Err(bad("unexpected data"))
            }
        };
        match tag {
            T_REGISTER => Ok(Ctl::Register(id(rest)?)),
            T_REGISTERED => empty(rest, Ctl::Registered),
            T_TAKEN => empty(rest, Ctl::Taken),
            T_HEARTBEAT => empty(rest, Ctl::Heartbeat),
            T_INCOMING => Ok(Ctl::Incoming(token(rest)?)),
            T_ACCEPT => Ok(Ctl::Accept(token(rest)?)),
            T_CONNECT => Ok(Ctl::Connect(id(rest)?)),
            T_NOTFOUND => empty(rest, Ctl::NotFound),
            T_PAIRED => empty(rest, Ctl::Paired),
            T_REFUSED => empty(rest, Ctl::Refused),
            _ => Err(bad("unknown frame")),
        }
    }
}

pub fn write_ctl(w: &mut impl Write, m: &Ctl) -> Result<(), NetError> {
    let b = m.encode();
    w.write_all(&(b.len() as u16).to_be_bytes())?;
    w.write_all(&b)?;
    w.flush()?;
    Ok(())
}

pub fn read_ctl(r: &mut impl Read) -> Result<Ctl, NetError> {
    let mut l = [0u8; 2];
    r.read_exact(&mut l)?;
    let n = u16::from_be_bytes(l) as usize;
    if n == 0 || n > MAX_FRAME {
        return Err(NetError::Proto("bad frame length"));
    }
    let mut b = vec![0u8; n];
    r.read_exact(&mut b)?;
    Ctl::decode(&b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_hostile_input() {
        for m in [
            Ctl::Register("123456789".into()),
            Ctl::Registered,
            Ctl::Taken,
            Ctl::Heartbeat,
            Ctl::Incoming([7; 16]),
            Ctl::Accept([9; 16]),
            Ctl::Connect("987654321".into()),
            Ctl::NotFound,
            Ctl::Paired,
            Ctl::Refused,
        ] {
            let mut buf = Vec::new();
            write_ctl(&mut buf, &m).unwrap();
            assert_eq!(read_ctl(&mut &buf[..]).unwrap(), m);
        }
        assert!(Ctl::decode(&[]).is_err());
        assert!(Ctl::decode(&[0xEE]).is_err());
        assert!(Ctl::decode(&[T_REGISTER, b'1', b'2']).is_err(), "short id");
        assert!(Ctl::decode(b"\x01abcdefghi").is_err(), "non digits");
        assert!(Ctl::decode(&[T_ACCEPT, 1, 2, 3]).is_err(), "short token");
        assert!(Ctl::decode(&[T_PAIRED, 0]).is_err(), "trailing data");
        // oversize and zero length frames
        assert!(read_ctl(&mut &[0u8, 200, 1][..]).is_err());
        assert!(read_ctl(&mut &[0u8, 0][..]).is_err());
    }

    #[test]
    fn ids_are_nine_digits() {
        for _ in 0..50 {
            let id = random_id().unwrap();
            assert!(valid_id(&id) && !id.starts_with('0'), "{id}");
        }
        assert!(!valid_id("12345678") && !valid_id("1234567890") && !valid_id("12345678a"));
    }
}
