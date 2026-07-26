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
    let file = std::fs::File::open(Path::new(path))
        .map_err(|e| format!("Graphics.newImage: cannot open {path:?}: {e}"))?;

    let decoder = png::Decoder::new(std::io::BufReader::new(file));
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
    fn a_missing_file_names_the_path() {
        let Err(err) = load("does_not_exist_92831.png") else {
            panic!("expected a failure");
        };
        assert!(err.contains("does_not_exist_92831.png"), "got: {err}");
    }
}
