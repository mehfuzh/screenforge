//! 3D look-up tables in DaVinci Resolve's `.cube` format, uploaded as a 3D texture that the
//! camera shader samples trilinearly (`composite.wgsl`, mode 1 with grading).

use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context};

/// A parsed `.cube` file: `size³` RGB triples, red varying fastest (the Resolve/Iridas order).
#[derive(Debug, Clone, PartialEq)]
pub struct CubeLut {
    pub title: String,
    pub size: u32,
    /// `size³ × 3` floats.
    pub data: Vec<f32>,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
}

/// 1D `.cube` files are expanded to a 3D table of this size.
const EXPAND_1D_SIZE: u32 = 33;

impl CubeLut {
    pub fn load(path: &Path) -> anyhow::Result<CubeLut> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn parse(text: &str) -> anyhow::Result<CubeLut> {
        let mut title = String::new();
        let mut size_3d: Option<u32> = None;
        let mut size_1d: Option<u32> = None;
        let mut domain_min = [0.0f32; 3];
        let mut domain_max = [1.0f32; 3];
        let mut data: Vec<f32> = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            let Some(head) = parts.next() else { continue };
            match head.to_ascii_uppercase().as_str() {
                "TITLE" => title = line[5..].trim().trim_matches('"').to_string(),
                "LUT_3D_SIZE" => size_3d = Some(parts.next().context("LUT_3D_SIZE without a value")?.parse()?),
                "LUT_1D_SIZE" => size_1d = Some(parts.next().context("LUT_1D_SIZE without a value")?.parse()?),
                "DOMAIN_MIN" => domain_min = triple(parts).context("DOMAIN_MIN needs three numbers")?,
                "DOMAIN_MAX" => domain_max = triple(parts).context("DOMAIN_MAX needs three numbers")?,
                // other keywords (LUT_3D_INPUT_RANGE …) are ignored
                h if h.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) => {}
                _ => {
                    let r: f32 = head.parse().with_context(|| format!("bad LUT line: {line:?}"))?;
                    let g: f32 = parts.next().and_then(|v| v.parse().ok()).with_context(|| format!("bad LUT line: {line:?}"))?;
                    let b: f32 = parts.next().and_then(|v| v.parse().ok()).with_context(|| format!("bad LUT line: {line:?}"))?;
                    data.extend_from_slice(&[r, g, b]);
                }
            }
        }
        if let Some(n) = size_3d {
            if n < 2 || n > 129 {
                bail!("unsupported LUT_3D_SIZE {n}");
            }
            if data.len() != (n * n * n * 3) as usize {
                bail!("expected {} entries for LUT_3D_SIZE {n}, found {}", n * n * n, data.len() / 3);
            }
            return Ok(CubeLut { title, size: n, data, domain_min, domain_max });
        }
        if let Some(n) = size_1d {
            if n < 2 || data.len() != (n * 3) as usize {
                bail!("expected {n} entries for LUT_1D_SIZE {n}, found {}", data.len() / 3);
            }
            // expand: out_c = table_c(in_c)
            let m = EXPAND_1D_SIZE;
            let curve = |c: usize, t: f32| -> f32 {
                let x = t * (n - 1) as f32;
                let i = (x.floor() as usize).min(n as usize - 2);
                let f = x - i as f32;
                data[i * 3 + c] * (1.0 - f) + data[(i + 1) * 3 + c] * f
            };
            let mut out = Vec::with_capacity((m * m * m * 3) as usize);
            for b in 0..m {
                for g in 0..m {
                    for r in 0..m {
                        out.push(curve(0, r as f32 / (m - 1) as f32));
                        out.push(curve(1, g as f32 / (m - 1) as f32));
                        out.push(curve(2, b as f32 / (m - 1) as f32));
                    }
                }
            }
            return Ok(CubeLut { title, size: m, data: out, domain_min, domain_max });
        }
        bail!("no LUT_3D_SIZE or LUT_1D_SIZE")
    }

    /// The identity table (what an unset LUT slot is bound to).
    pub fn identity(size: u32) -> CubeLut {
        let n = size.max(2);
        let mut data = Vec::with_capacity((n * n * n * 3) as usize);
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    data.extend_from_slice(&[r as f32 / (n - 1) as f32, g as f32 / (n - 1) as f32, b as f32 / (n - 1) as f32]);
                }
            }
        }
        CubeLut { title: "Identity".into(), size: n, data, domain_min: [0.0; 3], domain_max: [1.0; 3] }
    }

    fn at(&self, r: u32, g: u32, b: u32) -> [f32; 3] {
        let n = self.size;
        let i = ((b * n + g) * n + r) as usize * 3;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    /// Trilinear lookup on the CPU (tests, thumbnails). Input is clamped to the domain.
    pub fn sample(&self, rgb: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        let mut idx = [0u32; 3];
        let mut frac = [0f32; 3];
        for c in 0..3 {
            let t = ((rgb[c] - self.domain_min[c]) / (self.domain_max[c] - self.domain_min[c]).max(1e-6)).clamp(0.0, 1.0);
            let x = t * (n - 1) as f32;
            let i = (x.floor() as u32).min(n - 2);
            idx[c] = i;
            frac[c] = x - i as f32;
        }
        let mut out = [0f32; 3];
        for (dz, wz) in [(0, 1.0 - frac[2]), (1, frac[2])] {
            for (dy, wy) in [(0, 1.0 - frac[1]), (1, frac[1])] {
                for (dx, wx) in [(0, 1.0 - frac[0]), (1, frac[0])] {
                    let v = self.at(idx[0] + dx, idx[1] + dy, idx[2] + dz);
                    let w = wx * wy * wz;
                    for c in 0..3 {
                        out[c] += v[c] * w;
                    }
                }
            }
        }
        out
    }

    /// Serialize back to `.cube` text (bundled looks are generated this way).
    pub fn to_cube_string(&self) -> String {
        let mut s = String::with_capacity(self.data.len() * 10);
        if !self.title.is_empty() {
            s.push_str(&format!("TITLE \"{}\"\n", self.title));
        }
        s.push_str(&format!("LUT_3D_SIZE {}\n", self.size));
        if self.domain_min != [0.0; 3] || self.domain_max != [1.0; 3] {
            s.push_str(&format!("DOMAIN_MIN {} {} {}\nDOMAIN_MAX {} {} {}\n", self.domain_min[0], self.domain_min[1], self.domain_min[2], self.domain_max[0], self.domain_max[1], self.domain_max[2]));
        }
        for t in self.data.chunks_exact(3) {
            s.push_str(&format!("{:.6} {:.6} {:.6}\n", t[0], t[1], t[2]));
        }
        s
    }
}

/// IEEE half-precision bits of a finite `f32` (round to nearest even).
pub fn f16_bits(v: f32) -> u16 {
    let x = v.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    let exp = ((x >> 23) & 0xff) as i32;
    let mant = x & 0x7f_ffff;
    if exp == 0xff {
        // inf / nan
        return sign | 0x7c00 | if mant != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        // subnormal half
        let m = mant | 0x80_0000;
        let shift = (14 - e) as u32;
        let half_m = m >> shift;
        let rem = m & ((1 << shift) - 1);
        let halfway = 1 << (shift - 1);
        let rounded = if rem > halfway || (rem == halfway && (half_m & 1) == 1) { half_m + 1 } else { half_m };
        return sign | rounded as u16;
    }
    let half_m = mant >> 13;
    let rem = mant & 0x1fff;
    let mut out = ((e as u32) << 10) | half_m;
    if rem > 0x1000 || (rem == 0x1000 && (half_m & 1) == 1) {
        out += 1; // may carry into the exponent, which is the correct rounding
    }
    sign | out as u16
}

fn triple<'a>(mut parts: impl Iterator<Item = &'a str>) -> Option<[f32; 3]> {
    Some([parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?])
}

/// A LUT on the GPU: `size³` RGBA16 texels, sampled with the compositor's linear sampler.
#[derive(Debug)]
pub struct LutTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub size: u32,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    pub title: String,
}

impl LutTexture {
    /// Half floats: filterable on every backend without extra device features (16-bit UNORM
    /// needs `TEXTURE_FORMAT_16BIT_NORM`).
    pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

    pub fn upload(device: &wgpu::Device, queue: &wgpu::Queue, lut: &CubeLut, label: &str) -> Arc<Self> {
        let n = lut.size;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: n, height: n, depth_or_array_layers: n },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: Self::FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut bytes: Vec<u8> = Vec::with_capacity((n * n * n * 8) as usize);
        for t in lut.data.chunks_exact(3) {
            for v in [t[0], t[1], t[2], 1.0] {
                bytes.extend_from_slice(&f16_bits(v.clamp(0.0, 1.0)).to_le_bytes());
            }
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(n * 8), rows_per_image: Some(n) },
            wgpu::Extent3d { width: n, height: n, depth_or_array_layers: n },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D3), ..Default::default() });
        Arc::new(LutTexture { texture, view, size: n, domain_min: lut.domain_min, domain_max: lut.domain_max, title: lut.title.clone() })
    }

    pub fn identity(device: &wgpu::Device, queue: &wgpu::Queue) -> Arc<Self> {
        Self::upload(device, queue, &CubeLut::identity(2), "lut-identity")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_resolve_cube() {
        let text = "# generated\nTITLE \"Invert\"\nLUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n1 1 1\n0 1 1\n1 0 1\n0 0 1\n1 1 0\n0 1 0\n1 0 0\n0 0 0\n";
        let lut = CubeLut::parse(text).unwrap();
        assert_eq!(lut.title, "Invert");
        assert_eq!(lut.size, 2);
        let s = lut.sample([0.25, 0.5, 1.0]);
        assert!((s[0] - 0.75).abs() < 1e-5 && (s[1] - 0.5).abs() < 1e-5 && s[2].abs() < 1e-5, "{s:?}");
        // round trip through the writer
        let again = CubeLut::parse(&lut.to_cube_string()).unwrap();
        assert_eq!(again, lut);
    }

    #[test]
    fn identity_is_identity_and_1d_expands() {
        let id = CubeLut::identity(17);
        for c in [[0.0, 0.0, 0.0], [0.3, 0.6, 0.9], [1.0, 1.0, 1.0]] {
            let s = id.sample(c);
            for i in 0..3 {
                assert!((s[i] - c[i]).abs() < 1e-5, "{c:?} → {s:?}");
            }
        }
        let one_d = CubeLut::parse("LUT_1D_SIZE 2\n0 0 0\n0.5 0.5 0.5\n").unwrap();
        assert_eq!(one_d.size, 33);
        let s = one_d.sample([1.0, 0.5, 0.0]);
        assert!((s[0] - 0.5).abs() < 1e-5 && (s[1] - 0.25).abs() < 1e-5 && s[2].abs() < 1e-5, "{s:?}");
    }

    #[test]
    fn half_floats() {
        assert_eq!(f16_bits(0.0), 0x0000);
        assert_eq!(f16_bits(1.0), 0x3c00);
        assert_eq!(f16_bits(0.5), 0x3800);
        assert_eq!(f16_bits(-2.0), 0xc000);
        assert_eq!(f16_bits(65504.0), 0x7bff);
        assert_eq!(f16_bits(1e-8), 0x0000);
        // 0.1 → 0x2e66 (nearest even)
        assert_eq!(f16_bits(0.1), 0x2e66);
    }

    #[test]
    fn rejects_short_tables() {
        assert!(CubeLut::parse("LUT_3D_SIZE 2\n0 0 0\n").is_err());
        assert!(CubeLut::parse("0 0 0\n").is_err());
    }
}
