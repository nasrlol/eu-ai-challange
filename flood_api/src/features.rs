//! Rebuild the risk_model.py feature grid for any region: 100 m cells in EPSG:3035 with
//! elev, slope, relief, hand, dist_river, rp10, rp100, rp500 (+ population and facility distances).
use crate::osm::{self, Facility};
use crate::raster::{minimum_filter, read_window, Grid};
use anyhow::{anyhow, Context, Result};
use geo::{Contains, Coord, LineString, Polygon};
use proj4rs::{proj::Proj, transform::transform};
use rstar::{primitives::GeomWithData, RTree, RTreeObject, AABB};
use std::{collections::BTreeSet, path::Path};

pub const FEATURES: [&str; 8] = ["elev", "slope", "relief", "hand", "dist_river", "rp10", "rp100", "rp500"];
pub const GRID_M: f64 = 100.0;
const DEM_URL: &str = "https://copernicus-dem-30m.s3.amazonaws.com/{0}/{0}.tif";
const PAD_DEG: f64 = 0.02;

pub struct Laea(Proj, Proj);

impl Laea {
    pub fn new() -> Self {
        let wgs = Proj::from_proj_string("+proj=longlat +ellps=WGS84 +no_defs").unwrap();
        let laea = Proj::from_proj_string("+proj=laea +lat_0=52 +lon_0=10 +x_0=4321000 +y_0=3210000 +ellps=GRS80 +units=m +no_defs").unwrap();
        Laea(wgs, laea)
    }
    pub fn fwd(&self, lon: f64, lat: f64) -> (f64, f64) {
        let mut p = (lon.to_radians(), lat.to_radians(), 0.0);
        transform(&self.0, &self.1, &mut p).expect("EPSG:3035 forward");
        (p.0, p.1)
    }
    pub fn inv(&self, x: f64, y: f64) -> (f64, f64) {
        let mut p = (x, y, 0.0);
        transform(&self.1, &self.0, &mut p).expect("EPSG:3035 inverse");
        (p.0.to_degrees(), p.1.to_degrees())
    }
}

pub struct Region {
    pub xy: Vec<(f64, f64)>,
    pub lonlat: Vec<(f64, f64)>,
    pub feats: Vec<[f32; 8]>,
    pub pop_density: Vec<f32>,
    pub dist_fire: Vec<f32>,
    pub dist_hospital: Vec<f32>,
    pub facilities: Vec<Facility>,
    pub n_river_pts: usize,
}

struct Ring(Polygon<f64>);
impl RTreeObject for Ring {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        let pts: Vec<[f64; 2]> = self.0.exterior().coords().map(|c| [c.x, c.y]).collect();
        AABB::from_points(&pts)
    }
}

fn project(laea: &Laea, ring: &[(f64, f64)]) -> Vec<(f64, f64)> {
    ring.iter().map(|&(lon, lat)| laea.fwd(lon, lat)).collect()
}

fn dem_mosaic(dir: &Path, w: f64, s: f64, e: f64, n: f64) -> Result<Grid> {
    let res = 1.0 / 3600.0;
    let (width, height) = (((e - w) / res).round() as usize, ((n - s) / res).round() as usize);
    let mut tiles = Vec::new();
    for lat in s.floor() as i32..=n.floor() as i32 {
        for lon in w.floor() as i32..=e.floor() as i32 {
            let name = format!("Copernicus_DSM_COG_10_N{lat:02}_00_E{lon:03}_00_DEM");
            let path = dir.join(format!("{name}.tif"));
            if !path.exists() {
                let r = reqwest::blocking::get(DEM_URL.replace("{0}", &name))?;
                if !r.status().is_success() {
                    continue; // sea tiles do not exist
                }
                std::fs::create_dir_all(dir)?;
                let part = path.with_extension("part");
                std::fs::write(&part, r.bytes()?)?;
                std::fs::rename(&part, &path)?;
            }
            tiles.push(read_window(&path, w, s, e, n)?);
        }
    }
    // nearest-neighbour resample onto a 1" grid anchored at (w, n), like rasterio.merge in risk_model.py
    let mut data = vec![f32::NAN; width * height];
    for r in 0..height {
        let lat = n - (r as f64 + 0.5) * res;
        for c in 0..width {
            let lon = w + (c as f64 + 0.5) * res;
            if let Some(t) = tiles.iter().find(|t| t.contains(lon, lat)) {
                data[r * width + c] = t.at(lon, lat);
            }
        }
    }
    Ok(Grid { data, width, height, x0: w, y0: n, dx: res, dy: res })
}

/// np.gradient-based slope in degrees, as in risk_model.py
fn slope(dem: &Grid, mid_lat: f64) -> Grid {
    let dy = 111_320.0 / 3600.0;
    let dx = dy * mid_lat.to_radians().cos();
    let (w, h) = (dem.width, dem.height);
    let diff = |a: f32, b: f32, steps: f64, sp: f64| (a - b) as f64 / (steps * sp);
    let mut data = vec![0f32; w * h];
    for r in 0..h {
        for c in 0..w {
            let gy = match r {
                0 => diff(dem.get(1, c), dem.get(0, c), 1.0, dy),
                _ if r == h - 1 => diff(dem.get(r, c), dem.get(r - 1, c), 1.0, dy),
                _ => diff(dem.get(r + 1, c), dem.get(r - 1, c), 2.0, dy),
            };
            let gx = match c {
                0 => diff(dem.get(r, 1), dem.get(r, 0), 1.0, dx),
                _ if c == w - 1 => diff(dem.get(r, c), dem.get(r, c - 1), 1.0, dx),
                _ => diff(dem.get(r, c + 1), dem.get(r, c - 1), 2.0, dx),
            };
            data[r * w + c] = gx.hypot(gy).atan().to_degrees() as f32;
        }
    }
    dem.with_data(data)
}

/// Points along all lines every <= 50 m (shapely segmentize), snapped to 25 m and deduplicated.
fn river_points(lines: &[Vec<(f64, f64)>]) -> Vec<[f64; 2]> {
    let mut set = BTreeSet::new();
    for l in lines {
        for seg in l.windows(2) {
            let ((x0, y0), (x1, y1)) = (seg[0], seg[1]);
            let k = ((x1 - x0).hypot(y1 - y0) / 50.0).ceil().max(1.0) as usize;
            for i in 0..=k {
                let t = i as f64 / k as f64;
                let snap = |v: f64| ((v / 25.0).round() * 25.0) as i64;
                set.insert((snap(x0 + t * (x1 - x0)), snap(y0 + t * (y1 - y0))));
            }
        }
    }
    set.into_iter().map(|(x, y)| [x as f64, y as f64]).collect()
}

pub fn build(region_lonlat: &[(f64, f64)], data: &Path, cache: &Path) -> Result<Region> {
    let laea = Laea::new();
    let aoi = Polygon::new(LineString::from(project(&laea, region_lonlat)), vec![]);
    let (mut w, mut s, mut e, mut n) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(lon, lat) in region_lonlat {
        (w, s, e, n) = (w.min(lon), s.min(lat), e.max(lon), n.max(lat));
    }
    let (w, s, e, n) = (w - PAD_DEG, s - PAD_DEG, e + PAD_DEG, n + PAD_DEG);

    // Terrain
    let dem = dem_mosaic(&data.join("dem"), w, s, e, n).context("DEM")?;
    let slope = slope(&dem, (s + n) / 2.0);
    let mut relief = minimum_filter(&dem, 33);
    for (r, z) in relief.iter_mut().zip(&dem.data) {
        *r = z - *r;
    }
    let relief = dem.with_data(relief);
    let river_floor = dem.with_data(minimum_filter(&dem, 5));

    // Hydrography from OSM, projected
    let osm = osm::fetch(w, s, e, n, &data.join("osm"), cache).context("OSM")?;
    let water: Vec<Ring> = osm.water.iter()
        .map(|r| Ring(Polygon::new(LineString::from(project(&laea, r)), vec![])))
        .collect();
    let lines: Vec<Vec<(f64, f64)>> = osm.rivers.iter().chain(&osm.water).map(|l| project(&laea, l)).collect();
    let rpts = river_points(&lines);
    if rpts.is_empty() {
        return Err(anyhow!("no rivers found in OSM for this region"));
    }
    let river_z: Vec<f32> = rpts.iter().map(|p| { let (lon, lat) = laea.inv(p[0], p[1]); river_floor.at(lon, lat) }).collect();
    let rivers = RTree::bulk_load(rpts.iter().enumerate().map(|(i, p)| GeomWithData::new(*p, i)).collect());
    let water = RTree::bulk_load(water);

    // Hazard and exposure rasters
    let efas: Vec<Grid> = [10, 100, 500].iter()
        .map(|rp| read_window(&data.join(format!("cems-efas/Europe_RP{rp}_filled_depth.tif")), w, s, e, n))
        .collect::<Result<_>>()?;
    let pop = read_window(&data.join("ghs_pop/GHS_POP_E2020_GLOBE_R2023A_4326_30ss_V1_0/GHS_POP_E2020_GLOBE_R2023A_4326_30ss_V1_0.tif"), w, s, e, n)?;

    let fac_tree = |kind: &str| {
        RTree::bulk_load(osm.facilities.iter().filter(|f| f.kind == kind).map(|f| { let (x, y) = laea.fwd(f.lon, f.lat); [x, y] }).collect::<Vec<_>>())
    };
    let (fire, hosp) = (fac_tree("fire_station"), fac_tree("hospital"));
    let dist = |t: &RTree<[f64; 2]>, p: [f64; 2]| t.nearest_neighbor(&p).map_or(f32::NAN, |q| (q[0] - p[0]).hypot(q[1] - p[1]) as f32);

    // Grid of cell centres inside the region and outside water, as in Area.grid()
    let b = geo::BoundingRect::bounding_rect(&aoi).unwrap();
    let mut reg = Region { xy: vec![], lonlat: vec![], feats: vec![], pop_density: vec![], dist_fire: vec![], dist_hospital: vec![],
                           facilities: osm.facilities.clone(), n_river_pts: rpts.len() };
    let mut y = b.max().y - GRID_M / 2.0;
    while y > b.min().y {
        let mut x = b.min().x + GRID_M / 2.0;
        while x < b.max().x {
            let c = Coord { x, y };
            let in_water = || water.locate_in_envelope_intersecting(&AABB::from_point([x, y])).any(|r| r.0.contains(&c));
            if aoi.contains(&c) && !in_water() {
                let (lon, lat) = laea.inv(x, y);
                let elev = dem.at(lon, lat);
                let nn = rivers.nearest_neighbor(&[x, y]).unwrap();
                let depth = |g: &Grid| { let v = g.at(lon, lat); if v > 0.0 { v } else { 0.0 } };
                reg.feats.push([elev, slope.at(lon, lat), relief.at(lon, lat), elev - river_z[nn.data],
                                (nn.geom()[0] - x).hypot(nn.geom()[1] - y) as f32, depth(&efas[0]), depth(&efas[1]), depth(&efas[2])]);
                // GHS-POP people per ~1 km cell -> people per km²
                let cell_km2 = (pop.dx * 111.32) * (pop.dy * 111.32 * lat.to_radians().cos());
                reg.pop_density.push(pop.at(lon, lat).max(0.0) / cell_km2 as f32);
                reg.dist_fire.push(dist(&fire, [x, y]));
                reg.dist_hospital.push(dist(&hosp, [x, y]));
                reg.xy.push((x, y));
                reg.lonlat.push((lon, lat));
            }
            x += GRID_M;
        }
        y -= GRID_M;
    }
    Ok(reg)
}

#[cfg(test)]
mod tests {
    use super::Laea;

    #[test]
    fn laea_matches_pyproj() {
        let laea = Laea::new();
        // reference values from pyproj Transformer(4326 -> 3035, always_xy)
        for (lon, lat, x, y) in [(7.0, 50.5, 4108208.028, 3047480.024), (5.5, 50.6, 4002617.848, 3064031.318),
                                 (6.2, 51.3, 4056109.17, 3139007.769)] {
            let (px, py) = laea.fwd(lon, lat);
            assert!((px - x).abs() < 0.01 && (py - y).abs() < 0.01, "fwd {lon},{lat}: {px},{py}");
            let (qlon, qlat) = laea.inv(x, y);
            assert!((qlon - lon).abs() < 1e-7 && (qlat - lat).abs() < 1e-7, "inv {x},{y}: {qlon},{qlat}");
        }
    }
}
