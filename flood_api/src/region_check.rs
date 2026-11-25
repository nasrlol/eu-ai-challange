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

/// Outer rings (EPSG:3035) of a GeoPackage geometry blob (GP header + WKB Polygon or MultiPolygon)
fn outer_rings(b: &[u8]) -> Result<Vec<Vec<Coord>>> {
    let env = [0, 32, 48, 48, 64].get(((b[3] >> 1) & 7) as usize).copied().context("bad GeoPackage envelope")?;
    let w = &b[8 + env..];
    let u32_at = |le: bool, i: usize| { let a = w[i..i + 4].try_into().unwrap(); if le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) } };
    let f64_at = |le: bool, i: usize| { let a = w[i..i + 8].try_into().unwrap(); if le { f64::from_le_bytes(a) } else { f64::from_be_bytes(a) } };
    // header of one WKB geometry at `at`: byte order + type
    let header = |at: usize| (w[at] == 1, u32_at(w[at] == 1, at + 1));
    let (le, kind) = header(0);
    let (polys, multi) = match kind {
        3 => (1, false),
        6 => (u32_at(le, 5) as usize, true),
        t => bail!("geometry type {t} is not a 2D polygon"),
    };
    let (mut at, mut out) = (if multi { 9 } else { 0 }, Vec::new());
    for _ in 0..polys {
        let (le, kind) = header(at);
        if kind != 3 {
            bail!("geometry part type {kind} is not a 2D polygon");
        }
        let rings = u32_at(le, at + 5);
        at += 9;
        for r in 0..rings {
            let n = u32_at(le, at) as usize;
            at += 4;
            if r == 0 {
                out.push((0..n).map(|k| Coord { x: f64_at(le, at + 16 * k), y: f64_at(le, at + 16 * k + 8) }).collect());
            }
            at += 16 * n;
        }
    }
    Ok(out)
}

fn open(gpkg: &Path) -> Result<rusqlite::Connection> {
    rusqlite::Connection::open_with_flags(gpkg, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("{} missing; run python code/region.py", gpkg.display()))
}

/// Study region hull, EPSG:3035 (outer ring only: the hull is convex)
fn hull(gpkg: &Path) -> Result<Polygon> {
    let b: Vec<u8> = open(gpkg)?.query_row("SELECT geom FROM hull", [], |r| r.get(0))?;
    let ring = outer_rings(&b)?.into_iter().next().context("empty study hull")?;
    Ok(Polygon::new(LineString::from(ring), vec![]))
}

/// Outline of a CEMS AOI (layer `aois`, e.g. EMSR517_AOI06_DEL; the product suffix may be left out
/// when only one product exists) as a lon/lat ring: its largest part, which always lies inside the hull
pub fn aoi_ring(gpkg: &Path, aoi: &str) -> Result<(String, Vec<(f64, f64)>)> {
    let db = open(gpkg)?;
    let mut st = db.prepare("SELECT area, geom FROM aois WHERE area = ?1 OR area LIKE ?1 || '\\_%' ESCAPE '\\'")?;
    let rows: Vec<(String, Vec<u8>)> = st.query_map([aoi], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    let (name, blob) = match rows.as_slice() {
        [one] => one.clone(),
        [] => bail!("unknown AOI {aoi:?}; the study region AOIs are listed in /files/region/study_region.geojson"),
        _ => bail!("AOI {aoi:?} is ambiguous: {}", rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>().join(", ")),
    };
    let rings = outer_rings(&blob)?;
    let largest = rings.into_iter()
        .max_by(|a, b| Polygon::new(LineString::from(a.clone()), vec![]).unsigned_area()
            .total_cmp(&Polygon::new(LineString::from(b.clone()), vec![]).unsigned_area()))
        .context("AOI has no polygon")?;
    let laea = Laea::new();
    Ok((name, largest.iter().map(|c| laea.inv(c.x, c.y)).collect()))
}

/// Study hull bbox in lon/lat [w, s, e, n]
pub fn hull_bbox(gpkg: &Path) -> Result<[f64; 4]> {
    let laea = Laea::new();
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for c in hull(gpkg)?.exterior().coords() {
        let (lon, lat) = laea.inv(c.x, c.y);
        b = [b[0].min(lon), b[1].min(lat), b[2].max(lon), b[3].max(lat)];
    }
    Ok(b)
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

#[cfg(test)]
mod tests {
    use super::outer_rings;

    /// GeoPackage blob: "GP", version, flags (little endian, no envelope), srs id, then the WKB
    fn gpkg(wkb: Vec<u8>) -> Vec<u8> {
        [b"GP".to_vec(), vec![0, 1], 3035u32.to_le_bytes().to_vec(), wkb].concat()
    }
    fn polygon(rings: &[&[(f64, f64)]]) -> Vec<u8> {
        let mut w = vec![1];
        w.extend(3u32.to_le_bytes());
        w.extend((rings.len() as u32).to_le_bytes());
        for r in rings {
            w.extend((r.len() as u32).to_le_bytes());
            r.iter().for_each(|&(x, y)| { w.extend(x.to_le_bytes()); w.extend(y.to_le_bytes()); });
        }
        w
    }

    #[test]
    fn polygon_and_multipolygon_outer_rings() {
        let sq = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 0.0)];
        let hole = [(1.0, 1.0), (2.0, 1.0), (2.0, 2.0), (1.0, 1.0)];
        let far = [(50.0, 50.0), (60.0, 50.0), (60.0, 60.0), (50.0, 50.0)];
        let one = outer_rings(&gpkg(polygon(&[&sq, &hole]))).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!((one[0][1].x, one[0][1].y), (10.0, 0.0));
        let mut multi = vec![1];
        multi.extend(6u32.to_le_bytes());
        multi.extend(2u32.to_le_bytes());
        multi.extend(polygon(&[&sq, &hole]));
        multi.extend(polygon(&[&far]));
        let two = outer_rings(&gpkg(multi)).unwrap();
        assert_eq!(two.len(), 2);
        assert_eq!((two[1][0].x, two[1][0].y), (50.0, 50.0)); // the hole was skipped, not read as a polygon
    }
}
