//! RGBA8 images: PNG decode and encode through the `png` crate (no `image`
//! crate), nearest sampling, cell means and the nearest downsample the
//! bake uses.

use std::io::Cursor;

/// An 8-bit RGBA image, rows top to bottom (glTF's UV origin is the top
/// left, so `v` grows with the row).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba {
    /// Width in texels.
    pub width: u32,
    /// Height in texels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub data: Vec<u8>,
}

impl Rgba {
    /// A `width` x `height` image filled with `fill`.
    pub fn filled(width: u32, height: u32, fill: [u8; 4]) -> Rgba {
        let n = width as usize * height as usize;
        Rgba {
            width,
            height,
            data: fill.iter().copied().cycle().take(n * 4).collect(),
        }
    }

    /// Decode a PNG of any colour type and depth into RGBA8.
    pub fn decode_png(bytes: &[u8]) -> Result<Rgba, String> {
        if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err("not a PNG file (only PNG textures are supported)".into());
        }
        let mut decoder = png::Decoder::new(Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().map_err(|e| format!("png: {e}"))?;
        let size = reader
            .output_buffer_size()
            .ok_or("png: image too large to decode")?;
        let mut buf = vec![0; size];
        let info = reader
            .next_frame(&mut buf)
            .map_err(|e| format!("png: {e}"))?;
        if info.bit_depth != png::BitDepth::Eight {
            return Err(format!("png: unexpected bit depth {:?}", info.bit_depth));
        }
        let channels = match info.color_type {
            png::ColorType::Grayscale => 1,
            png::ColorType::GrayscaleAlpha => 2,
            png::ColorType::Rgb => 3,
            png::ColorType::Rgba => 4,
            png::ColorType::Indexed => return Err("png: palette was not expanded".into()),
        };
        let (w, h) = (info.width as usize, info.height as usize);
        let mut data = Vec::with_capacity(w * h * 4);
        for row in buf.chunks(info.line_size).take(h) {
            for px in row[..w * channels].chunks(channels) {
                let rgba = match channels {
                    1 => [px[0], px[0], px[0], 255],
                    2 => [px[0], px[0], px[0], px[1]],
                    3 => [px[0], px[1], px[2], 255],
                    _ => [px[0], px[1], px[2], px[3]],
                };
                data.extend_from_slice(&rgba);
            }
        }
        Ok(Rgba {
            width: info.width,
            height: info.height,
            data,
        })
    }

    /// Encode as an RGBA8 PNG. The same image always gives the same bytes.
    pub fn encode_png(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, self.width, self.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().map_err(|e| format!("png: {e}"))?;
            writer
                .write_image_data(&self.data)
                .map_err(|e| format!("png: {e}"))?;
            writer.finish().map_err(|e| format!("png: {e}"))?;
        }
        Ok(out)
    }

    /// The texel at `(x, y)`.
    pub fn texel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }

    /// Overwrite the texel at `(x, y)`.
    pub fn set(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.data[i..i + 4].copy_from_slice(&rgba);
    }

    /// The texel a nearest sampler reads at `uv` (repeat wrapping).
    pub fn texel_at(&self, uv: [f32; 2]) -> (u32, u32) {
        (
            wrap_texel(uv[0], self.width),
            wrap_texel(uv[1], self.height),
        )
    }

    /// The colour a nearest sampler reads at `uv`.
    pub fn sample(&self, uv: [f32; 2]) -> [u8; 4] {
        let (x, y) = self.texel_at(uv);
        self.texel(x, y)
    }

    /// Mean RGB over the rect `(x, y, w, h)` (clipped to the image).
    pub fn mean_rgb(&self, x: u32, y: u32, w: u32, h: u32) -> [u8; 3] {
        let mut sum = [0u64; 3];
        let mut n = 0u64;
        for ty in y..(y + h).min(self.height) {
            for tx in x..(x + w).min(self.width) {
                let t = self.texel(tx, ty);
                for c in 0..3 {
                    sum[c] += u64::from(t[c]);
                }
                n += 1;
            }
        }
        if n == 0 {
            return [0; 3];
        }
        [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8]
    }

    /// Nearest downsample by `factor`: each output texel takes the source
    /// texel at the centre of its `factor` x `factor` block.
    pub fn downsample_nearest(&self, factor: u32) -> Rgba {
        let width = self.width.div_ceil(factor).max(1);
        let height = self.height.div_ceil(factor).max(1);
        let mut out = Rgba::filled(width, height, [0; 4]);
        for y in 0..height {
            for x in 0..width {
                let sx = (x * factor + factor / 2).min(self.width - 1);
                let sy = (y * factor + factor / 2).min(self.height - 1);
                out.set(x, y, self.texel(sx, sy));
            }
        }
        out
    }

    /// A content hash of the decoded texels and size (FNV-1a, 64 bits), so
    /// identical atlases shipped in several folders count as one texture.
    pub fn content_hash(&self) -> u64 {
        let mut h = fnv1a(FNV_OFFSET, &self.width.to_le_bytes());
        h = fnv1a(h, &self.height.to_le_bytes());
        fnv1a(h, &self.data)
    }
}

/// `#rrggbb`.
pub fn hex(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// The texel index a nearest sampler with repeat wrapping reads for one
/// texture coordinate.
pub fn wrap_texel(t: f32, size: u32) -> u32 {
    let f = t - t.floor();
    ((f * size as f32) as u32).min(size.saturating_sub(1))
}

/// FNV-1a offset basis.
pub const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// Fold `bytes` into an FNV-1a 64-bit hash.
pub fn fnv1a(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trips_and_encodes_the_same_bytes_twice() {
        let mut img = Rgba::filled(5, 3, [10, 20, 30, 255]);
        img.set(4, 2, [200, 100, 50, 128]);
        let a = img.encode_png().unwrap();
        let b = img.encode_png().unwrap();
        assert_eq!(a, b);
        assert_eq!(Rgba::decode_png(&a).unwrap(), img);
        assert!(Rgba::decode_png(b"GIF89a").is_err());
    }

    #[test]
    fn nearest_sampling_wraps_and_downsampling_takes_block_centres() {
        let mut img = Rgba::filled(4, 4, [0, 0, 0, 255]);
        img.set(1, 1, [255, 0, 0, 255]);
        img.set(3, 3, [0, 255, 0, 255]);
        assert_eq!(img.sample([0.3, 0.3]), [255, 0, 0, 255]);
        assert_eq!(img.sample([1.3, -0.7]), [255, 0, 0, 255]);
        assert_eq!(img.sample([1.0, 1.0]), [0, 0, 0, 255]);
        let half = img.downsample_nearest(2);
        assert_eq!((half.width, half.height), (2, 2));
        assert_eq!(half.texel(0, 0), [255, 0, 0, 255]);
        assert_eq!(half.texel(1, 1), [0, 255, 0, 255]);
        assert_eq!(img.mean_rgb(0, 0, 2, 2), [63, 0, 0]);
        assert_eq!(hex([255, 16, 1]), "#ff1001");
        assert_ne!(img.content_hash(), half.content_hash());
    }
}
