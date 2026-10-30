//! Windowed reads from tiled EPSG:4326 GeoTIFFs (EFAS, GHS-POP, Copernicus DEM) without GDAL:
//! only the tiles covering the requested bounds are decoded.
use anyhow::{bail, Context, Result};
use std::{fs::File, io::BufReader, path::Path};
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;

/// A north-up lon/lat raster held in memory: lon = x0 + col * dx, lat = y0 - row * dy (pixel corners).
pub struct Grid {
    pub data: Vec<f32>,
    pub width: usize,
    pub height: usize,
    pub x0: f64,
    pub y0: f64,
    pub dx: f64,
    pub dy: f64,
}

impl Grid {
    /// Same georeferencing, new values (e.g. slope computed from a DEM)
    pub fn with_data(&self, data: Vec<f32>) -> Grid {
        Grid { data, width: self.width, height: self.height, x0: self.x0, y0: self.y0, dx: self.dx, dy: self.dy }
    }

    pub fn get(&self, row: usize, col: usize) -> f32 {
        self.data[row * self.width + col]
    }

    /// Same lookup as `Area.at` in risk_model.py: floor to a pixel, clipped to the edge.
    pub fn at(&self, lon: f64, lat: f64) -> f32 {
        let col = ((lon - self.x0) / self.dx).floor().clamp(0.0, (self.width - 1) as f64) as usize;
        let row = ((self.y0 - lat) / self.dy).floor().clamp(0.0, (self.height - 1) as f64) as usize;
        self.get(row, col)
    }

    pub fn contains(&self, lon: f64, lat: f64) -> bool {
        let col = (lon - self.x0) / self.dx;
        let row = (self.y0 - lat) / self.dy;
        col >= 0.0 && row >= 0.0 && col < self.width as f64 && row < self.height as f64
    }
}

/// Read the pixels of `path` covering lon w..e, lat s..n (clipped to the file).
pub fn read_window(path: &Path, w: f64, s: f64, e: f64, n: f64) -> Result<Grid> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut dec = Decoder::new(BufReader::new(file))?.with_limits(Limits::unlimited());
    let (width, height) = dec.dimensions()?;
    let scale = dec.get_tag_f64_vec(Tag::ModelPixelScaleTag)?;
    let tie = dec.get_tag_f64_vec(Tag::ModelTiepointTag)?;
    let (dx, dy) = (scale[0], scale[1]);
    let (mut x0, mut y0) = (tie[3] - tie[0] * dx, tie[4] + tie[1] * dy);
    // GTRasterTypeGeoKey (1025) = 2 means the tiepoint is a pixel centre (Copernicus DEM); GDAL shifts by half a pixel
    if let Ok(keys) = dec.get_tag_u32_vec(Tag::GeoKeyDirectoryTag) {
        if keys.chunks(4).skip(1).any(|k| k[0] == 1025 && k[3] == 2) {
            x0 -= dx / 2.0;
            y0 += dy / 2.0;
        }
    }

    let c0 = ((w - x0) / dx).floor().max(0.0) as usize;
    let c1 = (((e - x0) / dx).floor() as i64).min(width as i64 - 1);
    let r0 = ((y0 - n) / dy).floor().max(0.0) as usize;
    let r1 = (((y0 - s) / dy).floor() as i64).min(height as i64 - 1);
    if c1 < c0 as i64 || r1 < r0 as i64 {
        bail!("{}: bounds outside raster", path.display());
    }
    let (c1, r1) = (c1 as usize, r1 as usize);
    let (ww, wh) = (c1 - c0 + 1, r1 - r0 + 1);

    let (tw, th) = dec.chunk_dimensions();
    let (tw, th) = (tw as usize, th as usize);
    let across = (width as usize).div_ceil(tw);
    // sparse files (EFAS) store all-nodata tiles with 0 bytes: GDAL reads them as nodata, the decoder
    // would fail with "corrupt deflate stream", so they stay NaN here
    let sizes = dec.get_tag_u64_vec(Tag::TileByteCounts).unwrap_or_default();
    let mut data = vec![f32::NAN; ww * wh];
    for tr in r0 / th..=r1 / th {
        for tc in c0 / tw..=c1 / tw {
            let idx = (tr * across + tc) as u32;
            if sizes.get(idx as usize) == Some(&0) {
                continue;
            }
            let (dw, _) = dec.chunk_data_dimensions(idx);
            let vals: Vec<f32> = match dec.read_chunk(idx)? {
                DecodingResult::F32(v) => v,
                DecodingResult::F64(v) => v.into_iter().map(|x| x as f32).collect(),
                _ => bail!("{}: unsupported sample type", path.display()),
            };
            // edge tiles may come back cropped; infer the row stride from the buffer
            let stride = if vals.len() == tw * th { tw } else { dw as usize };
            let rows = vals.len() / stride;
            for r in 0..rows {
                let gr = tr * th + r;
                if gr < r0 || gr > r1 {
                    continue;
                }
                for c in 0..stride {
                    let gc = tc * tw + c;
                    if gc >= c0 && gc <= c1 {
                        data[(gr - r0) * ww + (gc - c0)] = vals[r * stride + c];
                    }
                }
            }
        }
    }
    Ok(Grid { data, width: ww, height: wh, x0: x0 + c0 as f64 * dx, y0: y0 - r0 as f64 * dy, dx, dy })
}

/// scipy.ndimage.minimum_filter(a, size) with the default 'reflect' edges; separable, so rows then columns.
pub fn minimum_filter(g: &Grid, size: usize) -> Vec<f32> {
    let h = (size / 2) as i64;
    let reflect = |i: i64, n: i64| if i < 0 { -i - 1 } else if i >= n { 2 * n - i - 1 } else { i };
    let (w, ht) = (g.width as i64, g.height as i64);
    let fmin = |a: f32, b: f32| if a.is_nan() || b < a { b } else { a };
    let mut tmp = vec![0f32; g.data.len()];
    for r in 0..ht {
        for c in 0..w {
            let mut m = f32::NAN;
            for k in -h..=h {
                m = fmin(m, g.data[(r * w + reflect(c + k, w)) as usize]);
            }
            tmp[(r * w + c) as usize] = m;
        }
    }
    let mut out = vec![0f32; g.data.len()];
    for r in 0..ht {
        for c in 0..w {
            let mut m = f32::NAN;
            for k in -h..=h {
                m = fmin(m, tmp[(reflect(r + k, ht) * w + c) as usize]);
            }
            out[(r * w + c) as usize] = m;
        }
    }
    out
}
