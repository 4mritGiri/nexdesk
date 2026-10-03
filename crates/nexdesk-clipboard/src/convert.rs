//! Pure conversions between Windows clipboard payloads and what X11/Wayland apps expect.
//! Everything here is free of I/O so it can be unit tested.
use std::path::PathBuf;

/// UTF-8 (LF) -> CF_UNICODETEXT payload (UTF-16LE, CRLF, NUL terminated).
pub fn text_to_rdp(s: &str) -> Vec<u8> {
    let normalized = s.replace("\r\n", "\n").replace('\n', "\r\n");
    let mut out = Vec::with_capacity(normalized.len() * 2 + 2);
    for u in normalized.encode_utf16() {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out.extend_from_slice(&[0, 0]);
    out
}

/// CF_UNICODETEXT payload -> UTF-8 with LF line endings. Stops at the first NUL.
pub fn text_from_rdp(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units).replace("\r\n", "\n")
}

/// Percent-decode a URI path component. Invalid escapes are kept literally.
pub fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let h = (b[i + 1] as char).to_digit(16);
            let l = (b[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (h, l) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Parse `text/uri-list` or `x-special/gnome-copied-files` into local paths.
/// Only `file://` URIs (empty or `localhost` authority) are accepted.
pub fn parse_uri_list(data: &str) -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let mut out = Vec::new();
    for (i, line) in data.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // gnome-copied-files: first line is the action.
        if i == 0 && (line == "copy" || line == "cut") {
            continue;
        }
        let Some(rest) = line.strip_prefix("file://") else {
            continue;
        };
        let path = if let Some(p) = rest.strip_prefix("localhost") { p } else { rest };
        if !path.starts_with('/') {
            continue; // remote host authority: refuse
        }
        out.push(PathBuf::from(std::ffi::OsString::from_vec(percent_decode(path))));
    }
    out
}

// ---------------------------------------------------------------- images

fn le32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// PNG -> CF_DIB (BITMAPINFOHEADER, 24 bpp, bottom-up, alpha flattened onto white).
pub fn png_to_dib(png: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 || w > 32768 || h > 32768 {
        return Err("unsupported image size".into());
    }
    let stride = ((w as usize * 3) + 3) & !3;
    let image_size = stride * h as usize;
    let mut out = Vec::with_capacity(40 + image_size);
    out.extend_from_slice(&40u32.to_le_bytes()); // biSize
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes()); // positive = bottom-up
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&24u16.to_le_bytes()); // bit count
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(image_size as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for y in (0..h).rev() {
        let mut row = Vec::with_capacity(stride);
        for x in 0..w {
            let p = img.get_pixel(x, y).0;
            let a = u32::from(p[3]);
            let flat = |c: u8| ((u32::from(c) * a + 255 * (255 - a) + 127) / 255) as u8;
            row.extend_from_slice(&[flat(p[2]), flat(p[1]), flat(p[0])]);
        }
        row.resize(stride, 0);
        out.extend_from_slice(&row);
    }
    Ok(out)
}

/// CF_DIB (24/32 bpp BI_RGB or BI_BITFIELDS, optional top-down) -> PNG.
pub fn dib_to_png(dib: &[u8]) -> Result<Vec<u8>, String> {
    let size = le32(dib, 0).ok_or("short DIB")? as usize;
    if size < 40 || dib.len() < size {
        return Err("bad DIB header".into());
    }
    let w = le32(dib, 4).ok_or("short DIB")? as i32;
    let h = le32(dib, 8).ok_or("short DIB")? as i32;
    let bpp = u16::from_le_bytes(dib.get(14..16).ok_or("short DIB")?.try_into().unwrap());
    let compression = le32(dib, 16).ok_or("short DIB")?;
    let clr_used = le32(dib, 32).unwrap_or(0) as usize;
    if w <= 0 || h == 0 || w > 32768 || h.unsigned_abs() > 32768 {
        return Err("unsupported DIB size".into());
    }
    if !(bpp == 24 || bpp == 32) || !(compression == 0 || compression == 3) {
        return Err(format!("unsupported DIB format ({bpp} bpp, compression {compression})"));
    }
    // BI_BITFIELDS stores three masks after the header (only the standard BGR layout is supported).
    let mut off = size;
    if compression == 3 && size == 40 {
        off += 12;
    }
    off += clr_used.min(256) * 4;
    let (w, top_down, ah) = (w as usize, h < 0, h.unsigned_abs() as usize);
    let bytes_pp = usize::from(bpp / 8);
    let stride = (w * bytes_pp + 3) & !3;
    let pixels = dib.get(off..).ok_or("short DIB")?;
    if pixels.len() < stride * ah {
        return Err("truncated DIB pixel data".into());
    }
    let mut rgba = Vec::with_capacity(w * ah * 4);
    for row in 0..ah {
        let src_row = if top_down { row } else { ah - 1 - row };
        let line = &pixels[src_row * stride..src_row * stride + w * bytes_pp];
        for px in line.chunks_exact(bytes_pp) {
            // Alpha in 32 bpp BI_RGB bitmaps is almost always unused (0): treat as opaque.
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    let img = image::RgbaImage::from_raw(w as u32, ah as u32, rgba).ok_or("bad image buffer")?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_roundtrip_and_newlines() {
        let rdp = text_to_rdp("a\nb\r\nc é 😀");
        assert_eq!(&rdp[rdp.len() - 2..], &[0, 0]);
        assert_eq!(text_from_rdp(&rdp), "a\nb\nc é 😀");
        // CRLF on the wire
        let units: Vec<u16> = rdp.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(&units[..5], &[b'a' as u16, 13, 10, b'b' as u16, 13]);
    }

    #[test]
    fn text_from_rdp_stops_at_nul_and_handles_odd_length() {
        assert_eq!(text_from_rdp(&[b'h', 0, b'i', 0, 0, 0, b'x', 0]), "hi");
        assert_eq!(text_from_rdp(&[b'h', 0, 7]), "h");
        assert_eq!(text_from_rdp(&[]), "");
    }

    #[test]
    fn uri_list_parsing() {
        let l = "# comment\r\nfile:///tmp/a%20b.txt\r\nfile://localhost/tmp/c\r\nfile://otherhost/x\r\nhttp://x/y\r\n";
        assert_eq!(parse_uri_list(l), vec![PathBuf::from("/tmp/a b.txt"), PathBuf::from("/tmp/c")]);
        let g = "copy\nfile:///home/u/%C3%A9.txt\nfile:///x";
        assert_eq!(parse_uri_list(g), vec![PathBuf::from("/home/u/é.txt"), PathBuf::from("/x")]);
        assert!(parse_uri_list("cut").is_empty());
    }

    #[test]
    fn percent_decode_edges() {
        assert_eq!(percent_decode("%41%"), b"A%");
        assert_eq!(percent_decode("%zz%4"), b"%zz%4");
        assert_eq!(percent_decode("%2F"), b"/");
    }

    #[test]
    fn png_dib_roundtrip() {
        let mut img = image::RgbaImage::new(3, 2);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([(x * 80) as u8, (y * 100) as u8, 200, 255]);
        }
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let dib = png_to_dib(png.get_ref()).unwrap();
        assert_eq!(le32(&dib, 0), Some(40));
        let back = dib_to_png(&dib).unwrap();
        let out = image::load_from_memory(&back).unwrap().to_rgba8();
        assert_eq!(out, img);
    }

    #[test]
    fn dib_rejects_garbage() {
        assert!(dib_to_png(&[1, 2, 3]).is_err());
        assert!(dib_to_png(&[0u8; 60]).is_err());
    }
}
