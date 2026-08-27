//! Image loading — decode a PNG from disk into a [`Surface`] the rasterizer can
//! blit.
//!
//! Images share the engine's surface registry with canvases, so a loaded image
//! *is* a canvas handle: `Graphics.draw` composites it, `Graphics.drawFrame`
//! picks a spritesheet cell out of it, and `Graphics.setCanvas` can even draw
//! into it. That keeps one concept where Love2D has two.
//!
//! PNG is the only format: it is lossless, ubiquitous in sprite pipelines, and
//! `png` is a pure-Rust decoder, so image support adds no system dependency.

use std::path::Path;

use crate::raster::Surface;

/// Decode the PNG at `path` into a premultiply-free ARGB surface.
///
/// The rasterizer samples straight (non-premultiplied) alpha, so the channels
/// go in as the file stores them.
pub fn load(path: &str) -> Result<Surface, String> {
    let bytes = std::fs::read(Path::new(path))
        .map_err(|e| format!("Graphics.newImage: cannot open {path:?}: {e}"))?;

    decode(&bytes, path)
}

/// Decode a PNG already held in memory.
///
/// Same decoder, no file: this is what backs `Graphics.newImageFromMemory`, so
/// an asset compiled into the program or fetched over the network needs no
/// temporary file on the way in. `name` only names the source in errors.
pub fn decode(bytes: &[u8], path: &str) -> Result<Surface, String> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("Graphics.newImage: {path:?} is not a readable PNG: {e}"))?;

    let mut raw = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut raw)
        .map_err(|e| format!("Graphics.newImage: failed to decode {path:?}: {e}"))?;

    let width = info.width as usize;
    let height = info.height as usize;
    if width == 0 || height == 0 {
        return Err(format!("Graphics.newImage: {path:?} has zero extent"));
    }

    // 16-bit channels are decoded as two bytes each, big-endian; take the high
    // byte. Anything else is already 8-bit.
    let stride = match info.bit_depth {
        png::BitDepth::Sixteen => 2,
        _ => 1,
    };

    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        // `read_info` expands palettes to RGB/RGBA for us, so reaching this arm
        // would mean the decoder changed under us.
        png::ColorType::Indexed => {
            return Err(format!(
                "Graphics.newImage: {path:?} uses an indexed palette the decoder did not expand"
            ));
        }
    };

    let mut buf = vec![0u32; width * height];
    let row_bytes = width * channels * stride;

    for y in 0..height {
        let row = &raw[y * row_bytes..(y + 1) * row_bytes];

        for x in 0..width {
            let px = &row[x * channels * stride..];
            let at = |c: usize| px[c * stride];

            let (r, g, b, a) = match channels {
                1 => (at(0), at(0), at(0), 0xFF),
                2 => (at(0), at(0), at(0), at(1)),
                3 => (at(0), at(1), at(2), 0xFF),
                _ => (at(0), at(1), at(2), at(3)),
            };

            buf[y * width + x] = u32::from_be_bytes([a, r, g, b]);
        }
    }

    Ok(Surface {
        buf,
        w: width,
        h: height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a 2x1 RGBA PNG and return its path.
    fn fixture(name: &str, color_type: png::ColorType, data: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        let file = std::fs::File::create(&path).expect("create fixture");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 2, 1);
        encoder.set_color(color_type);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("header");
        writer.write_image_data(data).expect("pixels");
        drop(writer);
        path
    }

    #[test]
    fn rgba_channels_land_in_argb_order() {
        // Opaque red, then half-transparent blue.
        let path = fixture(
            "saule_image_rgba.png",
            png::ColorType::Rgba,
            &[255, 0, 0, 255, 0, 0, 255, 128],
        );
        let Ok(surface) = load(path.to_str().unwrap()) else {
            panic!("decode failed");
        };

        assert_eq!((surface.w, surface.h), (2, 1));
        assert_eq!(surface.buf[0], 0xFFFF_0000);
        assert_eq!(surface.buf[1], 0x8000_00FF);
    }

    #[test]
    fn rgb_without_alpha_is_fully_opaque() {
        let path = fixture(
            "saule_image_rgb.png",
            png::ColorType::Rgb,
            &[0, 255, 0, 0, 0, 0],
        );
        let Ok(surface) = load(path.to_str().unwrap()) else {
            panic!("decode failed");
        };

        assert_eq!(surface.buf[0], 0xFF00_FF00);
        assert_eq!(surface.buf[1], 0xFF00_0000);
    }

    #[test]
    fn decoding_from_memory_matches_decoding_from_disk() {
        let path = fixture(
            "saule_image_mem.png",
            png::ColorType::Rgba,
            &[255, 0, 0, 255, 0, 0, 255, 128],
        );
        let bytes = std::fs::read(&path).expect("read fixture");

        let from_disk = load(path.to_str().unwrap()).expect("disk decode");
        let from_memory = decode(&bytes, "<memory>").expect("memory decode");

        assert_eq!(from_disk.buf, from_memory.buf);
        assert_eq!((from_memory.w, from_memory.h), (2, 1));
    }

    #[test]
    fn saving_then_loading_round_trips_every_channel() {
        let original = Surface {
            buf: vec![0xFFFF_0000, 0x8000_00FF, 0x0012_3456, 0xFF00_FF00],
            w: 2,
            h: 2,
        };
        let path = std::env::temp_dir().join("saule_image_roundtrip.png");
        save(&original, path.to_str().unwrap()).expect("save");

        let reloaded = load(path.to_str().unwrap()).expect("reload");
        assert_eq!(reloaded.buf, original.buf);
        assert_eq!((reloaded.w, reloaded.h), (2, 2));
    }

    #[test]
    fn a_missing_file_names_the_path() {
        let Err(err) = load("does_not_exist_92831.png") else {
            panic!("expected a failure");
        };
        assert!(err.contains("does_not_exist_92831.png"), "got: {err}");
    }
}

/// Encode a surface as an 8-bit RGBA PNG at `path`.
///
/// This is what `Graphics.saveImage` writes — a screenshot, a rendered canvas
/// exported for a test, or a generated asset. The surface stores straight
/// (non-premultiplied) ARGB, which is exactly what PNG wants, so the channels
/// are just reordered on the way out.
pub fn save(surface: &Surface, path: &str) -> Result<(), String> {
    let file = std::fs::File::create(Path::new(path))
        .map_err(|e| format!("Graphics.saveImage: cannot create {path:?}: {e}"))?;

    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(file),
        surface.w as u32,
        surface.h as u32,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);

    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("Graphics.saveImage: cannot write {path:?}: {e}"))?;

    let mut raw = Vec::with_capacity(surface.buf.len() * 4);
    for &px in &surface.buf {
        let [a, r, g, b] = px.to_be_bytes();
        raw.extend_from_slice(&[r, g, b, a]);
    }

    writer
        .write_image_data(&raw)
        .map_err(|e| format!("Graphics.saveImage: cannot write {path:?}: {e}"))
}

// ---------------------------------------------------------------------------
// Base64
// ---------------------------------------------------------------------------

/// Decode a base64-encoded PNG.
///
/// The native ABI carries strings as UTF-8, so raw PNG bytes cannot cross it —
/// base64 is what makes an *embedded* asset possible at all: a sprite sheet
/// pasted into a `.sau` source file, or image bytes that arrived over a
/// network, with no temporary file on the way in.
pub fn decode_base64(data: &str) -> Result<Surface, String> {
    let bytes = from_base64(data)?;
    decode(&bytes, "<base64>")
}

/// Standard base64 (RFC 4648) with optional `=` padding, tolerating the
/// whitespace a long literal is usually wrapped with.
fn from_base64(data: &str) -> Result<Vec<u8>, String> {
    /// Value of a base64 digit, or `None` for anything else.
    fn digit(b: u8) -> Option<u32> {
        match b {
            b'A'..=b'Z' => Some((b - b'A') as u32),
            b'a'..=b'z' => Some((b - b'a') as u32 + 26),
            b'0'..=b'9' => Some((b - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let mut out = Vec::with_capacity(data.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0u32;

    for (i, b) in data.bytes().enumerate() {
        if b.is_ascii_whitespace() || b == b'=' {
            continue;
        }
        let Some(value) = digit(b) else {
            return Err(format!(
                "Graphics.newImageFromBase64: byte {i} is not a base64 digit"
            ));
        };
        acc = (acc << 6) | value;
        bits += 6;

        // Every complete byte is emitted as soon as its eight bits are in.
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }

    if out.is_empty() {
        return Err("Graphics.newImageFromBase64: the data is empty".into());
    }
    Ok(out)
}

#[cfg(test)]
mod base64_tests {
    use super::*;

    #[test]
    fn decodes_the_rfc_examples() {
        assert_eq!(from_base64("TWFu").unwrap(), b"Man");
        assert_eq!(from_base64("TWE=").unwrap(), b"Ma");
        assert_eq!(from_base64("TQ==").unwrap(), b"M");
    }

    #[test]
    fn whitespace_between_groups_is_ignored() {
        assert_eq!(from_base64("TWFu\n  TWFu").unwrap(), b"ManMan");
    }

    #[test]
    fn a_stray_character_names_its_position() {
        let Err(err) = from_base64("TW!u") else {
            panic!("expected a failure");
        };
        assert!(err.contains("byte 2"), "got: {err}");
    }

    #[test]
    fn a_base64_png_round_trips_through_the_decoder() {
        let surface = Surface {
            buf: vec![0xFFFF_0000, 0x8000_00FF],
            w: 2,
            h: 1,
        };
        let path = std::env::temp_dir().join("saule_image_b64.png");
        save(&surface, path.to_str().unwrap()).expect("save");

        let bytes = std::fs::read(&path).expect("read back");
        let encoded = to_base64(&bytes);

        let decoded = decode_base64(&encoded).expect("decode");
        assert_eq!(decoded.buf, surface.buf);
    }

    /// Test-only encoder, so the round trip does not depend on a fixture
    /// string that would have to be regenerated whenever the PNG writer
    /// changes.
    fn to_base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(ALPHABET[(n >> 18) as usize & 63] as char);
            out.push(ALPHABET[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                ALPHABET[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }
}
