//! Messages inside the encrypted channel. Hand-rolled, big-endian, strictly length-checked.
use crate::PeerError;

/// Largest screen edge accepted.
pub const MAX_DIM: u16 = 16384;
/// Largest clipboard text accepted, in bytes.
pub const MAX_CLIP: usize = 1024 * 1024;
/// Largest cursor image edge accepted.
pub const MAX_CURSOR: u16 = 256;
/// Longest chat message accepted, in bytes.
pub const MAX_CHAT: usize = 2000;
/// Largest piece of a file in one message.
pub const MAX_CHUNK: usize = 48 * 1024;
/// A chat line made safe to send: no control characters, at most `MAX_CHAT` bytes.
pub fn clean_chat(t: &str) -> String {
    let mut out = String::new();
    for c in t.chars().filter(|c| !c.is_control()) {
        if out.len() + c.len_utf8() > MAX_CHAT {
            break;
        }
        out.push(c);
    }
    out
}

/// Files per transfer, size of one file and of one transfer.
pub const MAX_FILES: u32 = 10_000;
pub const MAX_FILE_SIZE: u64 = 8 * 1024 * 1024 * 1024;
pub const MAX_BATCH_BYTES: u64 = 64 * 1024 * 1024 * 1024;
/// Longest relative path of one file.
pub const MAX_PATH: usize = 400;
/// Largest raw tile (bytes) accepted: 64 MiB.
pub const MAX_TILE_BYTES: usize = 64 * 1024 * 1024;

/// A monitor (or the whole screen) inside the X screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i16,
    pub y: i16,
    pub w: u16,
    pub h: u16,
}

/// Most monitors listed.
pub const MAX_MONITORS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    // agent -> viewer
    /// First message after the handshake.
    Hello {
        view_only: bool,
        width: u16,
        height: u16,
    },
    /// A rectangle of the screen, BGRX pixels compressed with LZ4 (block format, size implied by w*h*4).
    Tile {
        x: u16,
        y: u16,
        w: u16,
        h: u16,
        lz4: Vec<u8>,
    },
    Bye(String),
    /// The shared monitors and which one is shown now. Sent after `Hello` and whenever the layout changes.
    Monitors {
        current: u8,
        rects: Vec<Rect>,
    },
    /// The remote pointer image: BGRA (premultiplied) pixels, LZ4-compressed, with its hotspot.
    Cursor {
        hot_x: u16,
        hot_y: u16,
        w: u16,
        h: u16,
        lz4: Vec<u8>,
    },
    // viewer -> agent
    /// Show another monitor (index into the last `Monitors` list); the agent answers with a new `Hello`.
    SelectMonitor(u8),
    MouseMove {
        x: u16,
        y: u16,
    },
    /// 1 = left, 2 = middle, 3 = right.
    MouseButton {
        button: u8,
        down: bool,
    },
    Wheel {
        dx: i16,
        dy: i16,
    },
    /// Linux evdev key code (KEY_*), the same on every Linux viewer and host.
    Key {
        code: u16,
        down: bool,
    },
    /// Ask to send `count` files (`total` bytes). `first` is the name of the first one, for the question
    /// shown to the user. Nothing is written until the other side answers yes.
    FilesOffer {
        batch: u32,
        count: u32,
        total: u64,
        first: String,
    },
    /// One file of an accepted batch: size and a relative path ('/' separated, never absolute).
    FileStart {
        batch: u32,
        id: u32,
        size: u64,
        path: String,
    },
    FileChunk {
        id: u32,
        data: Vec<u8>,
    },
    FileEnd {
        id: u32,
    },
    // both
    /// A chat line (UTF-8, at most `MAX_CHAT` bytes, control characters removed). Never logged.
    Chat(String),
    /// Answer to `FilesOffer`.
    FilesAnswer {
        batch: u32,
        accept: bool,
    },
    /// The receiver has stored `bytes` bytes of file `id` so far (lets the sender keep a window in flight).
    FileAck {
        id: u32,
        bytes: u64,
    },
    /// Stop a transfer, either side.
    FileAbort {
        batch: u32,
        reason: String,
    },
    /// Clipboard text (UTF-8, at most `MAX_CLIP` bytes). Never logged.
    Clip(String),
    Ping(u64),
    Pong(u64),
}

const T_HELLO: u8 = 1;
const T_TILE: u8 = 2;
const T_BYE: u8 = 3;
const T_CURSOR: u8 = 4;
const T_MONITORS: u8 = 5;
const T_SELECT: u8 = 0x14;
const T_CLIP: u8 = 0x30;
const T_MOVE: u8 = 0x10;
const T_BUTTON: u8 = 0x11;
const T_WHEEL: u8 = 0x12;
const T_KEY: u8 = 0x13;
const T_CHAT: u8 = 0x40;
const T_OFFER: u8 = 0x50;
const T_ANSWER: u8 = 0x51;
const T_FSTART: u8 = 0x52;
const T_FCHUNK: u8 = 0x53;
const T_FEND: u8 = 0x54;
const T_FACK: u8 = 0x55;
const T_FABORT: u8 = 0x56;
const T_PING: u8 = 0x20;
const T_PONG: u8 = 0x21;

fn bad(m: &'static str) -> PeerError {
    PeerError::Proto(m)
}

impl Msg {
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::new();
        match self {
            Msg::Hello {
                view_only,
                width,
                height,
            } => {
                v.push(T_HELLO);
                v.push(*view_only as u8);
                v.extend_from_slice(&width.to_be_bytes());
                v.extend_from_slice(&height.to_be_bytes());
            }
            Msg::Tile { x, y, w, h, lz4 } => {
                v.push(T_TILE);
                for n in [x, y, w, h] {
                    v.extend_from_slice(&n.to_be_bytes());
                }
                v.extend_from_slice(lz4);
            }
            Msg::Bye(s) => {
                v.push(T_BYE);
                let b = s.as_bytes();
                v.extend_from_slice(&b[..b.len().min(200)]);
            }
            Msg::Monitors { current, rects } => {
                v.push(T_MONITORS);
                v.push(*current);
                v.push(rects.len().min(MAX_MONITORS) as u8);
                for r in rects.iter().take(MAX_MONITORS) {
                    v.extend_from_slice(&r.x.to_be_bytes());
                    v.extend_from_slice(&r.y.to_be_bytes());
                    v.extend_from_slice(&r.w.to_be_bytes());
                    v.extend_from_slice(&r.h.to_be_bytes());
                }
            }
            Msg::SelectMonitor(i) => v.extend_from_slice(&[T_SELECT, *i]),
            Msg::Cursor {
                hot_x,
                hot_y,
                w,
                h,
                lz4,
            } => {
                v.push(T_CURSOR);
                for n in [hot_x, hot_y, w, h] {
                    v.extend_from_slice(&n.to_be_bytes());
                }
                v.extend_from_slice(lz4);
            }
            Msg::Clip(t) => {
                v.push(T_CLIP);
                v.extend_from_slice(t.as_bytes());
            }
            Msg::MouseMove { x, y } => {
                v.push(T_MOVE);
                v.extend_from_slice(&x.to_be_bytes());
                v.extend_from_slice(&y.to_be_bytes());
            }
            Msg::MouseButton { button, down } => {
                v.extend_from_slice(&[T_BUTTON, *button, *down as u8])
            }
            Msg::Wheel { dx, dy } => {
                v.push(T_WHEEL);
                v.extend_from_slice(&dx.to_be_bytes());
                v.extend_from_slice(&dy.to_be_bytes());
            }
            Msg::Key { code, down } => {
                v.push(T_KEY);
                v.extend_from_slice(&code.to_be_bytes());
                v.push(*down as u8);
            }
            Msg::Chat(t) => {
                v.push(T_CHAT);
                v.extend_from_slice(t.as_bytes());
            }
            Msg::FilesOffer {
                batch,
                count,
                total,
                first,
            } => {
                v.push(T_OFFER);
                v.extend_from_slice(&batch.to_be_bytes());
                v.extend_from_slice(&count.to_be_bytes());
                v.extend_from_slice(&total.to_be_bytes());
                v.extend_from_slice(first.as_bytes());
            }
            Msg::FilesAnswer { batch, accept } => {
                v.push(T_ANSWER);
                v.extend_from_slice(&batch.to_be_bytes());
                v.push(*accept as u8);
            }
            Msg::FileStart {
                batch,
                id,
                size,
                path,
            } => {
                v.push(T_FSTART);
                v.extend_from_slice(&batch.to_be_bytes());
                v.extend_from_slice(&id.to_be_bytes());
                v.extend_from_slice(&size.to_be_bytes());
                v.extend_from_slice(path.as_bytes());
            }
            Msg::FileChunk { id, data } => {
                v.push(T_FCHUNK);
                v.extend_from_slice(&id.to_be_bytes());
                v.extend_from_slice(data);
            }
            Msg::FileEnd { id } => {
                v.push(T_FEND);
                v.extend_from_slice(&id.to_be_bytes());
            }
            Msg::FileAck { id, bytes } => {
                v.push(T_FACK);
                v.extend_from_slice(&id.to_be_bytes());
                v.extend_from_slice(&bytes.to_be_bytes());
            }
            Msg::FileAbort { batch, reason } => {
                v.push(T_FABORT);
                v.extend_from_slice(&batch.to_be_bytes());
                let b = reason.as_bytes();
                v.extend_from_slice(&b[..b.len().min(200)]);
            }
            Msg::Ping(n) => {
                v.push(T_PING);
                v.extend_from_slice(&n.to_be_bytes());
            }
            Msg::Pong(n) => {
                v.push(T_PONG);
                v.extend_from_slice(&n.to_be_bytes());
            }
        }
        v
    }

    pub fn decode(b: &[u8]) -> Result<Msg, PeerError> {
        let (&tag, rest) = b.split_first().ok_or_else(|| bad("empty message"))?;
        let u16at = |i: usize| -> Result<u16, PeerError> {
            rest.get(i..i + 2)
                .map(|s| u16::from_be_bytes([s[0], s[1]]))
                .ok_or_else(|| bad("short message"))
        };
        let u32at = |i: usize| -> Result<u32, PeerError> {
            rest.get(i..i + 4)
                .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
                .ok_or_else(|| bad("short message"))
        };
        let u64at = |i: usize| -> Result<u64, PeerError> {
            rest.get(i..i + 8)
                .map(|s| {
                    let mut n = [0u8; 8];
                    n.copy_from_slice(s);
                    u64::from_be_bytes(n)
                })
                .ok_or_else(|| bad("short message"))
        };
        let exact = |n: usize| {
            if rest.len() == n {
                Ok(())
            } else {
                Err(bad("wrong message length"))
            }
        };
        let flag = |i: usize| -> Result<bool, PeerError> {
            match rest.get(i) {
                Some(0) => Ok(false),
                Some(1) => Ok(true),
                _ => Err(bad("bad flag")),
            }
        };
        Ok(match tag {
            T_HELLO => {
                exact(5)?;
                let (width, height) = (u16at(1)?, u16at(3)?);
                if width == 0 || height == 0 || width > MAX_DIM || height > MAX_DIM {
                    return Err(bad("bad screen size"));
                }
                Msg::Hello {
                    view_only: flag(0)?,
                    width,
                    height,
                }
            }
            T_TILE => {
                if rest.len() < 8 {
                    return Err(bad("short tile"));
                }
                let (x, y, w, h) = (u16at(0)?, u16at(2)?, u16at(4)?, u16at(6)?);
                if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM {
                    return Err(bad("bad tile size"));
                }
                if (w as usize) * (h as usize) * 4 > MAX_TILE_BYTES {
                    return Err(bad("tile too large"));
                }
                Msg::Tile {
                    x,
                    y,
                    w,
                    h,
                    lz4: rest[8..].to_vec(),
                }
            }
            T_BYE => Msg::Bye(
                String::from_utf8_lossy(rest)
                    .chars()
                    .filter(|c| !c.is_control())
                    .collect(),
            ),
            T_MONITORS => {
                let n = *rest.get(1).ok_or_else(|| bad("short monitors"))? as usize;
                if n == 0 || n > MAX_MONITORS || rest.len() != 2 + n * 8 {
                    return Err(bad("bad monitor list"));
                }
                let current = rest[0];
                if current as usize >= n {
                    return Err(bad("bad current monitor"));
                }
                let mut rects = Vec::with_capacity(n);
                for k in 0..n {
                    let o = 2 + k * 8;
                    let g = |i: usize| u16::from_be_bytes([rest[o + i], rest[o + i + 1]]);
                    let (x, y, rw, rh) = (g(0) as i16, g(2) as i16, g(4), g(6));
                    if rw == 0 || rh == 0 || rw > MAX_DIM || rh > MAX_DIM {
                        return Err(bad("bad monitor size"));
                    }
                    rects.push(Rect { x, y, w: rw, h: rh });
                }
                Msg::Monitors { current, rects }
            }
            T_SELECT => {
                exact(1)?;
                Msg::SelectMonitor(rest[0])
            }
            T_CURSOR => {
                if rest.len() < 8 {
                    return Err(bad("short cursor"));
                }
                let (hot_x, hot_y, w, h) = (u16at(0)?, u16at(2)?, u16at(4)?, u16at(6)?);
                if w == 0 || h == 0 || w > MAX_CURSOR || h > MAX_CURSOR || hot_x >= w || hot_y >= h
                {
                    return Err(bad("bad cursor"));
                }
                Msg::Cursor {
                    hot_x,
                    hot_y,
                    w,
                    h,
                    lz4: rest[8..].to_vec(),
                }
            }
            T_CLIP => {
                if rest.len() > MAX_CLIP {
                    return Err(bad("clipboard too large"));
                }
                Msg::Clip(
                    String::from_utf8(rest.to_vec()).map_err(|_| bad("clipboard is not UTF-8"))?,
                )
            }
            T_MOVE => {
                exact(4)?;
                Msg::MouseMove {
                    x: u16at(0)?,
                    y: u16at(2)?,
                }
            }
            T_BUTTON => {
                exact(2)?;
                let button = rest[0];
                if !(1..=3).contains(&button) {
                    return Err(bad("bad button"));
                }
                Msg::MouseButton {
                    button,
                    down: flag(1)?,
                }
            }
            T_WHEEL => {
                exact(4)?;
                Msg::Wheel {
                    dx: u16at(0)? as i16,
                    dy: u16at(2)? as i16,
                }
            }
            T_KEY => {
                exact(3)?;
                Msg::Key {
                    code: u16at(0)?,
                    down: flag(2)?,
                }
            }
            T_CHAT => {
                if rest.len() > MAX_CHAT {
                    return Err(bad("chat line too long"));
                }
                let t = String::from_utf8(rest.to_vec()).map_err(|_| bad("chat is not UTF-8"))?;
                Msg::Chat(t.chars().filter(|c| !c.is_control()).collect())
            }
            T_OFFER => {
                if rest.len() < 16 || rest.len() > 16 + MAX_PATH {
                    return Err(bad("bad file offer"));
                }
                let (batch, count, total) = (u32at(0)?, u32at(4)?, u64at(8)?);
                if count == 0 || count > MAX_FILES || total > MAX_BATCH_BYTES {
                    return Err(bad("file offer out of range"));
                }
                let first = String::from_utf8_lossy(&rest[16..])
                    .chars()
                    .filter(|c| !c.is_control())
                    .collect();
                Msg::FilesOffer {
                    batch,
                    count,
                    total,
                    first,
                }
            }
            T_ANSWER => {
                exact(5)?;
                Msg::FilesAnswer {
                    batch: u32at(0)?,
                    accept: flag(4)?,
                }
            }
            T_FSTART => {
                if rest.len() < 17 || rest.len() > 16 + MAX_PATH {
                    return Err(bad("bad file start"));
                }
                let size = u64at(8)?;
                if size > MAX_FILE_SIZE {
                    return Err(bad("file too large"));
                }
                let path = String::from_utf8(rest[16..].to_vec())
                    .map_err(|_| bad("file name is not UTF-8"))?;
                Msg::FileStart {
                    batch: u32at(0)?,
                    id: u32at(4)?,
                    size,
                    path,
                }
            }
            T_FCHUNK => {
                if rest.len() < 5 || rest.len() > 4 + MAX_CHUNK {
                    return Err(bad("bad file chunk"));
                }
                Msg::FileChunk {
                    id: u32at(0)?,
                    data: rest[4..].to_vec(),
                }
            }
            T_FEND => {
                exact(4)?;
                Msg::FileEnd { id: u32at(0)? }
            }
            T_FACK => {
                exact(12)?;
                Msg::FileAck {
                    id: u32at(0)?,
                    bytes: u64at(4)?,
                }
            }
            T_FABORT => {
                if rest.len() < 4 || rest.len() > 4 + 200 {
                    return Err(bad("bad abort"));
                }
                let reason = String::from_utf8_lossy(&rest[4..])
                    .chars()
                    .filter(|c| !c.is_control())
                    .collect();
                Msg::FileAbort {
                    batch: u32at(0)?,
                    reason,
                }
            }
            T_PING | T_PONG => {
                exact(8)?;
                let mut n = [0u8; 8];
                n.copy_from_slice(rest);
                let n = u64::from_be_bytes(n);
                if tag == T_PING {
                    Msg::Ping(n)
                } else {
                    Msg::Pong(n)
                }
            }
            _ => return Err(bad("unknown message")),
        })
    }
}

/// Compress BGRX pixels of one tile.
pub fn pack_pixels(bgrx: &[u8]) -> Vec<u8> {
    lz4_flex::block::compress(bgrx)
}

/// Decompress a tile; the result must be exactly `w*h*4` bytes.
pub fn unpack_pixels(lz4: &[u8], w: u16, h: u16) -> Result<Vec<u8>, PeerError> {
    let want = w as usize * h as usize * 4;
    if want > MAX_TILE_BYTES {
        return Err(bad("tile too large"));
    }
    let out = lz4_flex::block::decompress(lz4, want).map_err(|_| bad("corrupt tile"))?;
    if out.len() != want {
        return Err(bad("tile size mismatch"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(m: Msg) {
        assert_eq!(Msg::decode(&m.encode()).unwrap(), m);
    }

    #[test]
    fn roundtrip_all() {
        rt(Msg::Hello {
            view_only: true,
            width: 1920,
            height: 1080,
        });
        rt(Msg::Tile {
            x: 1,
            y: 2,
            w: 3,
            h: 4,
            lz4: vec![9, 8, 7],
        });
        rt(Msg::Bye("denied".into()));
        rt(Msg::MouseMove { x: 5, y: 6 });
        rt(Msg::MouseButton {
            button: 3,
            down: true,
        });
        rt(Msg::Wheel { dx: -120, dy: 240 });
        rt(Msg::Key {
            code: 30,
            down: false,
        });
        rt(Msg::Cursor {
            hot_x: 1,
            hot_y: 2,
            w: 16,
            h: 16,
            lz4: vec![1, 2],
        });
        rt(Msg::Monitors {
            current: 1,
            rects: vec![
                Rect {
                    x: 0,
                    y: 0,
                    w: 1920,
                    h: 1080,
                },
                Rect {
                    x: -1280,
                    y: 0,
                    w: 1280,
                    h: 1024,
                },
            ],
        });
        rt(Msg::SelectMonitor(2));
        rt(Msg::Clip("héllo\nworld".into()));
        rt(Msg::Ping(u64::MAX));
        rt(Msg::Pong(1));
        rt(Msg::Chat("hi there \u{1F600}".into()));
        rt(Msg::FilesOffer {
            batch: 7,
            count: 3,
            total: 1 << 33,
            first: "docs/a.txt".into(),
        });
        rt(Msg::FilesAnswer {
            batch: 7,
            accept: true,
        });
        rt(Msg::FileStart {
            batch: 7,
            id: 2,
            size: 99,
            path: "docs/a.txt".into(),
        });
        rt(Msg::FileChunk {
            id: 2,
            data: vec![1, 2, 3],
        });
        rt(Msg::FileEnd { id: 2 });
        rt(Msg::FileAck { id: 2, bytes: 4096 });
        rt(Msg::FileAbort {
            batch: 7,
            reason: "no space".into(),
        });
    }

    #[test]
    fn transfer_messages_reject_hostile_input() {
        let mut o = vec![T_OFFER];
        o.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0]); // batch 1, count 0
        o.extend_from_slice(&0u64.to_be_bytes());
        assert!(Msg::decode(&o).is_err(), "zero files");
        let mut o = vec![T_OFFER];
        o.extend_from_slice(&1u32.to_be_bytes());
        o.extend_from_slice(&1u32.to_be_bytes());
        o.extend_from_slice(&u64::MAX.to_be_bytes());
        assert!(Msg::decode(&o).is_err(), "absurd total");
        let mut c = vec![T_FCHUNK, 0, 0, 0, 1];
        c.resize(5 + MAX_CHUNK + 1, 0);
        assert!(Msg::decode(&c).is_err(), "oversized chunk");
        assert!(Msg::decode(&[T_FCHUNK, 0, 0, 0, 1]).is_err(), "empty chunk");
        let mut s = vec![T_FSTART];
        s.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 1]);
        s.extend_from_slice(&u64::MAX.to_be_bytes());
        s.push(b'a');
        assert!(Msg::decode(&s).is_err(), "file larger than the cap");
        assert!(Msg::decode(&[T_ANSWER, 0, 0, 0, 1, 2]).is_err(), "flag");
        let mut ch = vec![T_CHAT];
        ch.resize(1 + MAX_CHAT + 1, b'a');
        assert!(Msg::decode(&ch).is_err(), "chat cap");
        assert_eq!(
            Msg::decode(&[T_CHAT, b'a', 0x1b, b'[', b'b']).unwrap(),
            Msg::Chat("a[b".into()),
            "control characters stripped"
        );
    }

    #[test]
    fn hostile_input_is_rejected_not_trusted() {
        assert!(Msg::decode(&[]).is_err());
        assert!(Msg::decode(&[0xEE]).is_err());
        assert!(Msg::decode(&[T_HELLO, 0, 0, 0, 0, 0]).is_err(), "zero size");
        assert!(
            Msg::decode(&[T_HELLO, 2, 7, 128, 4, 56]).is_err(),
            "flag must be 0/1"
        );
        assert!(Msg::decode(&[T_MOVE, 0, 1]).is_err(), "short");
        assert!(Msg::decode(&[T_MOVE, 0, 1, 0, 1, 9]).is_err(), "long");
        assert!(Msg::decode(&[T_BUTTON, 9, 1]).is_err(), "button range");
        // 16384 x 16384 x 4 = 1 GiB tile must be refused before any allocation
        let mut t = vec![T_TILE, 0, 0, 0, 0];
        t.extend_from_slice(&16384u16.to_be_bytes());
        t.extend_from_slice(&16384u16.to_be_bytes());
        assert!(Msg::decode(&t).is_err());
        let mut t = vec![T_TILE, 0, 0, 0, 0, 0xFF, 0xFF, 0, 1];
        t.push(0);
        assert!(Msg::decode(&t).is_err(), "w above MAX_DIM");
        assert!(
            Msg::decode(&[T_CLIP, 0xFF, 0xFE]).is_err(),
            "clipboard must be UTF-8"
        );
        let mut big = vec![T_CLIP];
        big.resize(1 + MAX_CLIP + 1, b'a');
        assert!(Msg::decode(&big).is_err(), "clipboard size cap");
        assert!(
            Msg::decode(&[T_CURSOR, 0, 0, 0, 0, 0, 0, 0, 0]).is_err(),
            "zero cursor"
        );
        assert!(
            Msg::decode(&[T_CURSOR, 0, 16, 0, 0, 0, 16, 0, 16]).is_err(),
            "hotspot outside the image"
        );
        assert!(
            Msg::decode(&[T_CURSOR, 0, 0, 0, 0, 1, 1, 0, 16]).is_err(),
            "cursor larger than MAX_CURSOR"
        );
        assert!(Msg::decode(&[T_MONITORS, 0, 0]).is_err(), "no monitors");
        assert!(
            Msg::decode(&[T_MONITORS, 1, 1, 0, 0, 0, 0, 0, 10, 0, 10]).is_err(),
            "current out of range"
        );
        assert!(
            Msg::decode(&[T_MONITORS, 0, 1, 0, 0, 0, 0, 0, 0, 0, 10]).is_err(),
            "zero width"
        );
        assert!(
            Msg::decode(&[T_MONITORS, 0, 2, 0, 0, 0, 0, 0, 10, 0, 10]).is_err(),
            "count larger than data"
        );
        assert!(Msg::decode(&[T_SELECT]).is_err() && Msg::decode(&[T_SELECT, 1, 2]).is_err());
        // control characters in Bye are stripped (it is shown to the user)
        let m = Msg::decode(&[T_BYE, b'a', 0x1b, b'b', b'\n']).unwrap();
        assert_eq!(m, Msg::Bye("ab".into()));
    }

    #[test]
    fn pixels_roundtrip_and_size_is_enforced() {
        let px: Vec<u8> = (0..4 * 4 * 4).map(|i| (i % 7) as u8).collect();
        let z = pack_pixels(&px);
        assert_eq!(unpack_pixels(&z, 4, 4).unwrap(), px);
        assert!(
            unpack_pixels(&z, 4, 5).is_err(),
            "claimed size larger than data"
        );
        assert!(
            unpack_pixels(&z, 2, 2).is_err(),
            "claimed size smaller than data"
        );
        assert!(unpack_pixels(&[0xFF; 20], 4, 4).is_err(), "garbage");
    }
}
