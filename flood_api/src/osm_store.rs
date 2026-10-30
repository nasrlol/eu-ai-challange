//! Pre-built OSM store for the whole study region, so no request has to scan the Geofabrik
//! extracts (2 GB, minutes per new area). `flood_api build-osm-store` reads every extract once
//! (blobs decoded in parallel) and keeps only what the API uses: rivers, water bodies, water
//! relation outers, facilities, shelters and routable roads, with full node lists. They are
//! written as TILE_DEG tiles (bincode), a way in every tile its nodes touch.
//!
//! Queries load only the tiles covering the bbox (kept in memory up to CACHE_BYTES) and cut ways
//! to the bbox exactly like pbf::read / pbf::read_roads, so results are the same as reading the
//! extracts. A missing or stale store (extracts changed), or a bbox outside it, returns None and
//! the caller falls back to the extracts.
use super::pbf::{extracts, facility_kind, is_river, is_water, road_tags, Road};
use super::{rings, Facility, Osm};
use anyhow::{bail, Context, Result};
use osmpbf::{BlobDecode, BlobReader, Element, PrimitiveBlock, RelMemberType};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

const VERSION: u32 = 1;
const TILE_DEG: f64 = 0.1;
/// Decoded tiles kept in memory (by file size on disk)
const CACHE_BYTES: u64 = 1 << 30;

/// A kept way. Node lon is NaN where the node lies outside the store bbox: it can never be
/// inside a query bbox, but still splits the way into runs like a missing node does.
#[derive(Serialize, Deserialize, Clone)]
struct Way {
    id: i64,
    /// facility_kind() of the way's tags (emergency service or shelter)
    fac: Option<String>,
    name: Option<String>,
    river: bool,
    water: bool,
    /// Water relation this way is an outer of
    rel: Option<i64>,
    /// (highway, oneway) if routable
    road: Option<(String, i8)>,
    nodes: Vec<(i64, f64, f64)>,
}

#[derive(Serialize, Deserialize)]
struct Point {
    kind: String,
    name: Option<String>,
    lon: f64,
    lat: f64,
}

#[derive(Serialize, Deserialize, Default)]
struct Tile {
    ways: Vec<Way>,
    points: Vec<Point>,
}

#[derive(Serialize, Deserialize, PartialEq, Clone)]
struct Source {
    file: String,
    bytes: u64,
    modified: u64,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    bbox: [f64; 4],
    tile_deg: f64,
    sources: Vec<Source>,
    tiles: usize,
    ways: usize,
    points: usize,
    build_s: f64,
}

pub struct Store {
    dir: PathBuf,
    bbox: [f64; 4],
    cache: Mutex<(HashMap<(i32, i32), (Arc<Tile>, u64)>, VecDeque<(i32, i32)>, u64)>,
}

static STORE: OnceLock<Option<Store>> = OnceLock::new();

fn tile_of(lon: f64, lat: f64) -> (i32, i32) {
    ((lon / TILE_DEG).floor() as i32, (lat / TILE_DEG).floor() as i32)
}

fn sources(pbf_dir: &Path, bbox: [f64; 4]) -> Result<Vec<Source>> {
    let [w, s, e, n] = bbox;
    let mut out = vec![];
    for p in extracts(pbf_dir, w, s, e, n)? {
        let m = std::fs::metadata(&p)?;
        let modified = m.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_secs();
        out.push(Source { file: p.file_name().unwrap().to_string_lossy().into(), bytes: m.len(), modified });
    }
    out.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(out)
}

/// Open the store once (called at startup); prints why it is not used if it is missing or stale
pub fn init(dir: PathBuf, pbf_dir: &Path) {
    STORE.get_or_init(|| match open(&dir, pbf_dir) {
        Ok(s) => Some(s),
        Err(e) => {
            println!("osm store not used ({e:#}): new areas read the OSM extracts (slow); run `flood_api build-osm-store`");
            None
        }
    });
}

fn open(dir: &Path, pbf_dir: &Path) -> Result<Store> {
    let m: Manifest = serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).context("no manifest.json")?)?;
    if m.version != VERSION || m.tile_deg != TILE_DEG {
        bail!("store version {} / tile {}° is not {VERSION} / {TILE_DEG}°", m.version, m.tile_deg);
    }
    if sources(pbf_dir, m.bbox)? != m.sources {
        bail!("OSM extracts changed since the store was built");
    }
    println!("osm store: {} tiles, {} ways, {} points, bbox {:?}", m.tiles, m.ways, m.points, m.bbox);
    Ok(Store { dir: dir.to_path_buf(), bbox: m.bbox, cache: Mutex::new((HashMap::new(), VecDeque::new(), 0)) })
}

pub fn get() -> Option<&'static Store> {
    STORE.get().and_then(Option::as_ref)
}

impl Store {
    fn tile(&self, t: (i32, i32)) -> Result<Option<Arc<Tile>>> {
        if let Some((tile, _)) = self.cache.lock().unwrap().0.get(&t) {
            return Ok(Some(tile.clone()));
        }
        let path = self.dir.join(format!("{}_{}.bin", t.0, t.1));
        let Ok(bytes) = std::fs::read(&path) else { return Ok(None) }; // no OSM content there
        let tile: Arc<Tile> = Arc::new(bincode::deserialize(&bytes).with_context(|| format!("{}", path.display()))?);
        let mut c = self.cache.lock().unwrap();
        let (map, order, total) = &mut *c;
        if !map.contains_key(&t) {
            map.insert(t, (tile.clone(), bytes.len() as u64));
            order.push_back(t);
            *total += bytes.len() as u64;
            while *total > CACHE_BYTES && order.len() > 1 {
                let old = order.pop_front().unwrap();
                *total -= map.remove(&old).map_or(0, |x| x.1);
            }
        }
        Ok(Some(tile))
    }

    /// Tiles of the bbox, read in parallel; None if the bbox is not inside the store
    fn tiles(&self, w: f64, s: f64, e: f64, n: f64) -> Result<Option<Vec<Arc<Tile>>>> {
        let [bw, bs, be, bn] = self.bbox;
        if w < bw || s < bs || e > be || n > bn {
            return Ok(None);
        }
        let ((x0, y0), (x1, y1)) = (tile_of(w, s), tile_of(e, n));
        let keys: Vec<(i32, i32)> = (x0..=x1).flat_map(|x| (y0..=y1).map(move |y| (x, y))).collect();
        let tiles: Vec<Option<Arc<Tile>>> = keys.par_iter().map(|&t| self.tile(t)).collect::<Result<_>>()?;
        Ok(Some(tiles.into_iter().flatten().collect()))
    }

    /// Same result as pbf::read for the bbox
    pub fn osm(&self, w: f64, s: f64, e: f64, n: f64) -> Result<Option<Osm>> {
        let Some(tiles) = self.tiles(w, s, e, n)? else { return Ok(None) };
        let inside = |lon: f64, lat: f64| lon >= w && lon <= e && lat >= s && lat <= n;
        let mut osm = Osm { rivers: vec![], water: vec![], facilities: vec![] };
        let mut rel_parts: HashMap<i64, Vec<Vec<(f64, f64)>>> = HashMap::new();
        let mut seen = HashSet::new();
        for t in &tiles {
            osm.facilities.extend(t.points.iter().filter(|p| inside(p.lon, p.lat))
                .map(|p| Facility { kind: p.kind.clone(), name: p.name.clone(), lon: p.lon, lat: p.lat }));
            for way in &t.ways {
                if !(way.fac.is_some() || way.river || way.water || way.rel.is_some()) || !seen.insert(way.id) {
                    continue;
                }
                let runs = runs(&way.nodes, &inside);
                if runs.is_empty() {
                    continue;
                }
                if let Some(kind) = &way.fac {
                    let pts: Vec<_> = runs.concat();
                    let (lon, lat) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
                    let k = pts.len() as f64;
                    osm.facilities.push(Facility { kind: kind.clone(), name: way.name.clone(), lon: lon / k, lat: lat / k });
                } else if way.river {
                    osm.rivers.extend(runs.into_iter().filter(|r| r.len() >= 2));
                } else if let Some(rel) = way.rel {
                    if runs.len() == 1 {
                        rel_parts.entry(rel).or_default().extend(runs);
                    }
                } else if runs.len() == 1 && runs[0].first() == runs[0].last() {
                    osm.water.extend(rings(runs));
                } else {
                    osm.rivers.extend(runs.into_iter().filter(|r| r.len() >= 2));
                }
            }
        }
        for parts in rel_parts.into_values() {
            osm.water.extend(rings(parts));
        }
        Ok(Some(osm))
    }

    /// Same result as pbf::read_roads for the bbox
    pub fn roads(&self, w: f64, s: f64, e: f64, n: f64) -> Result<Option<(Vec<Road>, Vec<Facility>)>> {
        let Some(tiles) = self.tiles(w, s, e, n)? else { return Ok(None) };
        let inside = |lon: f64, lat: f64| lon >= w && lon <= e && lat >= s && lat <= n;
        let (mut roads, mut shelters, mut seen) = (vec![], vec![], HashSet::new());
        for t in &tiles {
            shelters.extend(t.points.iter().filter(|p| p.kind.ends_with("shelter") && inside(p.lon, p.lat))
                .map(|p| Facility { kind: p.kind.clone(), name: p.name.clone(), lon: p.lon, lat: p.lat }));
            for way in &t.ways {
                if !(way.road.is_some() || way.fac.as_deref().is_some_and(|k| k.ends_with("shelter"))) || !seen.insert(way.id) {
                    continue;
                }
                if let Some(kind) = way.fac.as_deref().filter(|k| k.ends_with("shelter")) {
                    let pts: Vec<_> = way.nodes.iter().filter(|p| inside(p.1, p.2)).collect();
                    if !pts.is_empty() {
                        let k = pts.len() as f64;
                        let (lon, lat) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.1 / k, a.1 + p.2 / k));
                        shelters.push(Facility { kind: kind.into(), name: way.name.clone(), lon, lat });
                    }
                    continue;
                }
                let (highway, oneway) = way.road.clone().unwrap();
                let mut runs: Vec<Vec<(i64, f64, f64)>> = vec![vec![]];
                for &(id, lon, lat) in &way.nodes {
                    if inside(lon, lat) {
                        runs.last_mut().unwrap().push((id, lon, lat));
                    } else if !runs.last().unwrap().is_empty() {
                        runs.push(vec![]);
                    }
                }
                for nodes in runs.into_iter().filter(|r| r.len() >= 2) {
                    roads.push(Road { highway: highway.clone(), name: way.name.clone(), oneway, nodes });
                }
            }
        }
        let mut keys = HashSet::new();
        shelters.retain(|f: &Facility| keys.insert(((f.lon * 1e5) as i64, (f.lat * 1e5) as i64)));
        Ok(Some((roads, shelters)))
    }
}

/// Runs of consecutive nodes inside the bbox (NaN = outside the store, so outside too)
fn runs(nodes: &[(i64, f64, f64)], inside: &impl Fn(f64, f64) -> bool) -> Vec<Vec<(f64, f64)>> {
    let mut runs: Vec<Vec<(f64, f64)>> = vec![vec![]];
    for &(_, lon, lat) in nodes {
        if inside(lon, lat) {
            runs.last_mut().unwrap().push((lon, lat));
        } else if !runs.last().unwrap().is_empty() {
            runs.push(vec![]);
        }
    }
    runs.retain(|r| !r.is_empty());
    runs
}

// ---------- build ----------

/// One parallel pass over the data blocks of an extract
fn par_blocks<T: Send>(path: &Path, f: impl Fn(&PrimitiveBlock) -> T + Sync + Send) -> Result<Vec<T>> {
    BlobReader::from_path(path)?.par_bridge()
        .map(|blob| Ok(match blob?.decode()? { BlobDecode::OsmData(b) => Some(f(&b)), _ => None }))
        .collect::<Result<Vec<_>>>().map(|v| v.into_iter().flatten().collect())
}

/// What the API uses from a way's tags: (facility kind, name, river, water, (highway, oneway))
fn keep(tags: &[(&str, &str)]) -> (Option<String>, Option<String>, bool, bool, Option<(String, i8)>) {
    let (fac, name) = facility_kind(tags.iter().copied());
    let road = road_tags(tags.iter().copied()).map(|(h, _, o)| (h.to_string(), o));
    (fac.map(String::from), name, is_river(tags), is_water(tags), road)
}

/// Build the store for `bbox` from the extracts in `pbf_dir` into `dir` (replaced atomically)
pub fn build(pbf_dir: &Path, dir: &Path, bbox: [f64; 4]) -> Result<()> {
    let t0 = std::time::Instant::now();
    let [w, s, e, n] = bbox;
    let inside = |lon: f64, lat: f64| lon >= w && lon <= e && lat >= s && lat <= n;
    let srcs = sources(pbf_dir, bbox)?;
    if srcs.is_empty() {
        bail!("no OSM extract in {} covers {bbox:?}", pbf_dir.display());
    }
    let mut ways: HashMap<i64, Way> = HashMap::new();
    let mut points: Vec<Point> = vec![];
    for src in &srcs {
        let path = pbf_dir.join(&src.file);
        let t = std::time::Instant::now();
        // 1. outer ways of water relations
        let rel_outer: HashMap<i64, i64> = par_blocks(&path, |b| {
            let mut v = vec![];
            for el in b.elements() {
                let Element::Relation(r) = el else { continue };
                if is_water(&r.tags().collect::<Vec<_>>()) {
                    v.extend(r.members().filter(|m| m.member_type == RelMemberType::Way && m.role().is_ok_and(|x| x == "outer"))
                        .map(|m| (m.member_id, r.id())));
                }
            }
            v
        })?.into_iter().flatten().collect();
        // 2. node ids used by the ways we keep
        let mut need: Vec<i64> = par_blocks(&path, |b| {
            let mut v = vec![];
            for el in b.elements() {
                let Element::Way(way) = el else { continue };
                let tags: Vec<_> = way.tags().collect();
                let (fac, _, river, water, road) = keep(&tags);
                if fac.is_some() || river || water || road.is_some() || rel_outer.contains_key(&way.id()) {
                    v.extend(way.refs());
                }
            }
            v
        })?.concat();
        need.par_sort_unstable();
        need.dedup();
        // 3. coordinates of those nodes inside the bbox, and facility nodes
        let (coords, pts): (Vec<Vec<(i64, f64, f64)>>, Vec<Vec<Point>>) = par_blocks(&path, |b| {
            let (mut c, mut p) = (vec![], vec![]);
            let mut node = |id: i64, lon: f64, lat: f64, fac: (Option<&str>, Option<String>)| {
                if !inside(lon, lat) {
                    return;
                }
                if need.binary_search(&id).is_ok() {
                    c.push((id, lon, lat));
                }
                if let (Some(kind), name) = fac {
                    p.push(Point { kind: kind.into(), name, lon, lat });
                }
            };
            for el in b.elements() {
                match el {
                    Element::Node(nd) => node(nd.id(), nd.lon(), nd.lat(), facility_kind(nd.tags())),
                    Element::DenseNode(nd) => node(nd.id(), nd.lon(), nd.lat(), facility_kind(nd.tags())),
                    _ => {}
                }
            }
            (c, p)
        })?.into_iter().unzip();
        drop(need);
        let coords: HashMap<i64, (f64, f64)> = coords.into_iter().flatten().map(|(id, lon, lat)| (id, (lon, lat))).collect();
        points.extend(pts.into_iter().flatten());
        // 4. the kept ways with their node coordinates (extracts overlap at borders: first copy wins)
        let found: Vec<Way> = par_blocks(&path, |b| {
            let mut v = vec![];
            for el in b.elements() {
                let Element::Way(way) = el else { continue };
                let tags: Vec<_> = way.tags().collect();
                let rel = rel_outer.get(&way.id()).copied();
                let (fac, name, river, water, road) = keep(&tags);
                if !(fac.is_some() || river || water || road.is_some() || rel.is_some()) {
                    continue;
                }
                let nodes: Vec<(i64, f64, f64)> = way.refs()
                    .map(|id| coords.get(&id).map_or((id, f64::NAN, f64::NAN), |&(lon, lat)| (id, lon, lat))).collect();
                if nodes.iter().any(|p| !p.1.is_nan()) {
                    v.push(Way { id: way.id(), fac, name, river, water, rel, road, nodes });
                }
            }
            v
        })?.into_iter().flatten().collect();
        let before = ways.len();
        for way in found {
            ways.entry(way.id).or_insert(way);
        }
        println!("  {}: {} new ways, {:.0} s", src.file, ways.len() - before, t.elapsed().as_secs_f64());
    }
    // the same mapped node can come from two extracts
    let mut keys = HashSet::new();
    points.retain(|p| keys.insert((p.kind.clone(), (p.lon * 1e7) as i64, (p.lat * 1e7) as i64)));

    // Tiles: a way in every tile its (known) nodes' bbox touches, a point in its own tile
    let mut tiles: HashMap<(i32, i32), Tile> = HashMap::new();
    let n_ways = ways.len();
    for way in ways.into_values() {
        let pts = way.nodes.iter().filter(|p| !p.1.is_nan());
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in pts {
            (x0, y0, x1, y1) = (x0.min(p.1), y0.min(p.2), x1.max(p.1), y1.max(p.2));
        }
        let ((tx0, ty0), (tx1, ty1)) = (tile_of(x0, y0), tile_of(x1, y1));
        let keys: Vec<(i32, i32)> = (tx0..=tx1).flat_map(|x| (ty0..=ty1).map(move |y| (x, y))).collect();
        let last = keys.len() - 1;
        let mut way = Some(way);
        for (i, k) in keys.into_iter().enumerate() {
            let w = if i == last { way.take().unwrap() } else { way.clone().unwrap() };
            tiles.entry(k).or_default().ways.push(w);
        }
    }
    let n_points = points.len();
    for p in points {
        tiles.entry(tile_of(p.lon, p.lat)).or_default().points.push(p);
    }

    // Write to a temp folder, then swap it in
    let tmp = dir.with_extension("building");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let n_tiles = tiles.len();
    tiles.into_par_iter().try_for_each(|((x, y), t)| -> Result<()> {
        std::fs::write(tmp.join(format!("{x}_{y}.bin")), bincode::serialize(&t)?)?;
        Ok(())
    })?;
    let m = Manifest { version: VERSION, bbox, tile_deg: TILE_DEG, sources: srcs, tiles: n_tiles, ways: n_ways, points: n_points,
                       build_s: t0.elapsed().as_secs_f64() };
    std::fs::write(tmp.join("manifest.json"), serde_json::to_string_pretty(&m)?)?;
    let _ = std::fs::remove_dir_all(dir);
    std::fs::rename(&tmp, dir)?;
    println!("osm store: {n_tiles} tiles, {n_ways} ways, {n_points} points in {:.0} s → {}", m.build_s, dir.display());
    Ok(())
}

/// Open a store directly (for the build check), bypassing the startup state
pub fn open_at(dir: &Path, pbf_dir: &Path) -> Result<Store> {
    open(dir, pbf_dir)
}
