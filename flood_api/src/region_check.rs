//! Checks on a requested region before any OSM or raster work: shape, size, and overlap with the
//! study region (the hull in code/outputs/region/study_region.gpkg, same file and rule as
//! flood_tool.py: refuse if more than 1 % lies outside).
use crate::features::Laea;
use anyhow::{bail, Context, Result};
use geo::{Area, BooleanOps, Coord, LineString, Polygon};
use std::path::Path;

pub const MAX_KM2: f64 = 3000.0; // largest training AOI is ~2,900 km²
const MIN_KM2: f64 = 0.01; // one 100 m grid cell
const MAX_OUTSIDE: f64 = 0.01;

/// Study region hull, EPSG:3035, read from the GeoPackage geometry blob (GP header + WKB polygon)
fn hull(gpkg: &Path) -> Result<Polygon> {
    let db = rusqlite::Connection::open_with_flags(gpkg, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("{} missing; run python code/region.py", gpkg.display()))?;
    let b: Vec<u8> = db.query_row("SELECT geom FROM hull", [], |r| r.get(0))?;
    let env = [0, 32, 48, 48, 64].get(((b[3] >> 1) & 7) as usize).copied().context("bad GeoPackage envelope")?;
    let w = &b[8 + env..];
    let le = w[0] == 1;
    let u32_at = |i: usize| { let a = w[i..i + 4].try_into().unwrap(); if le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) } };
    let f64_at = |i: usize| { let a = w[i..i + 8].try_into().unwrap(); if le { f64::from_le_bytes(a) } else { f64::from_be_bytes(a) } };
    if u32_at(1) != 3 {
        bail!("study hull is not a 2D polygon");
    }
    // outer ring only: the hull is convex
    let n = u32_at(9) as usize;
    let pts: Vec<Coord> = (0..n).map(|k| Coord { x: f64_at(13 + 16 * k), y: f64_at(21 + 16 * k) }).collect();
    Ok(Polygon::new(LineString::from(pts), vec![]))
}

/// Region ring (lon/lat) → area in km² if it may be built, else an error explaining why
pub fn check(ring: &[(f64, f64)], gpkg: &Path) -> Result<f64> {
    if ring.len() < 4 || ring.iter().any(|&(x, y)| !x.is_finite() || !y.is_finite() || x.abs() > 180.0 || y.abs() > 90.0) {
        bail!("region needs at least 3 distinct lon/lat corners");
    }
    let laea = Laea::new();
    let poly = Polygon::new(LineString::from(ring.iter().map(|&(lon, lat)| laea.fwd(lon, lat)).collect::<Vec<_>>()), vec![]);
    let km2 = poly.unsigned_area() / 1e6;
    if km2 < MIN_KM2 {
        bail!("region has (almost) no area ({km2:.4} km²)");
    }
    if km2 > MAX_KM2 {
        bail!("region is {km2:.0} km²; split it into parts of at most {MAX_KM2:.0} km²");
    }
    let outside = 1.0 - poly.intersection(&hull(gpkg)?).unsigned_area() / 1e6 / km2;
    if outside > MAX_OUTSIDE {
        bail!("{:.0}% of the region lies outside the study region (outputs/region/study_region.gpkg); the models were not built for it",
              outside * 100.0);
    }
    Ok(km2)
}
