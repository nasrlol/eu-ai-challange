//! Offline fallback for Overpass: read the same rivers, water bodies and facilities from Geofabrik
//! .osm.pbf extracts in working_data/osm/. Two passes per file: (1) nodes inside the bbox and
//! water relations, (2) ways. Parts of ways outside the bbox are cut off.
use super::{rings, shelter_kind, Facility, Osm};
use anyhow::Result;
use osmpbf::{BlobDecode, BlobReader, Element, ElementReader, RelMemberType};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

fn facility_kind<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>) -> (Option<&'static str>, Option<String>) {
    let (mut kind, mut name) = (None, None);
    for (k, v) in tags {
        match (k, v) {
            ("amenity", "fire_station") => kind = Some("fire_station"),
            ("amenity", "hospital") => kind = Some("hospital"),
            ("amenity", "police") => kind = Some("police"),
            ("emergency", "ambulance_station") => kind = Some("ambulance_station"),
            ("name", n) => name = Some(n.to_owned()),
            _ => kind = kind.or(shelter_kind(k, v)), // emergency services win over shelters
        }
    }
    (kind, name)
}

fn is_river(tags: &[(&str, &str)]) -> bool {
    tags.iter().any(|&(k, v)| k == "waterway" && matches!(v, "river" | "stream" | "canal"))
}

fn is_water(tags: &[(&str, &str)]) -> bool {
    tags.iter().any(|&(k, v)| (k == "natural" && matches!(v, "water" | "wetland")) || (k == "waterway" && v == "riverbank"))
}

/// Extracts whose header bbox overlaps the region
fn extracts(dir: &Path, w: f64, s: f64, e: f64, n: f64) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if !path.to_string_lossy().ends_with(".osm.pbf") {
            continue;
        }
        let overlaps = match BlobReader::from_path(&path)?.next() {
            Some(Ok(blob)) => match blob.decode()? {
                BlobDecode::OsmHeader(h) => h.bbox().map_or(true, |b| b.left < e && b.right > w && b.bottom < n && b.top > s),
                _ => true,
            },
            _ => true,
        };
        if overlaps {
            out.push(path);
        }
    }
    Ok(out)
}

pub fn read(dir: &Path, w: f64, s: f64, e: f64, n: f64) -> Result<Option<Osm>> {
    let files = extracts(dir, w, s, e, n)?;
    if files.is_empty() {
        return Ok(None);
    }
    let inside = |lon: f64, lat: f64| lon >= w && lon <= e && lat >= s && lat <= n;
    let mut osm = Osm { rivers: vec![], water: vec![], facilities: vec![] };
    for path in files {
        // Pass 1: node coordinates in the bbox, facility nodes, outer ways of water relations
        let mut coords: HashMap<i64, (f64, f64)> = HashMap::new();
        let mut rel_outer: HashMap<i64, i64> = HashMap::new(); // way id -> relation id
        let mut node = |id: i64, lon: f64, lat: f64, kind: Option<&str>, name: Option<String>, osm: &mut Osm| {
            if inside(lon, lat) {
                coords.insert(id, (lon, lat));
                if let Some(kind) = kind {
                    osm.facilities.push(Facility { kind: kind.into(), name, lon, lat });
                }
            }
        };
        ElementReader::from_path(&path)?.for_each(|el| match el {
            Element::Node(nd) => {
                let (k, name) = facility_kind(nd.tags());
                node(nd.id(), nd.lon(), nd.lat(), k, name, &mut osm);
            }
            Element::DenseNode(nd) => {
                let (k, name) = facility_kind(nd.tags());
                node(nd.id(), nd.lon(), nd.lat(), k, name, &mut osm);
            }
            Element::Relation(r) => {
                if is_water(&r.tags().collect::<Vec<_>>()) {
                    for m in r.members() {
                        if m.member_type == RelMemberType::Way && m.role().is_ok_and(|role| role == "outer") {
                            rel_outer.insert(m.member_id, r.id());
                        }
                    }
                }
            }
            Element::Way(_) => {}
        })?;

        // Pass 2: ways, cut to the runs of nodes that fall inside the bbox
        let mut rel_parts: HashMap<i64, Vec<Vec<(f64, f64)>>> = HashMap::new();
        let mut seen_facility_ways = HashSet::new();
        ElementReader::from_path(&path)?.for_each(|el| {
            let Element::Way(way) = el else { return };
            let tags: Vec<(&str, &str)> = way.tags().collect();
            let (kind, name) = facility_kind(tags.iter().copied());
            let in_rel = rel_outer.get(&way.id());
            if !is_river(&tags) && !is_water(&tags) && kind.is_none() && in_rel.is_none() {
                return;
            }
            let mut runs: Vec<Vec<(f64, f64)>> = vec![vec![]];
            for id in way.refs() {
                match coords.get(&id) {
                    Some(&p) => runs.last_mut().unwrap().push(p),
                    None if !runs.last().unwrap().is_empty() => runs.push(vec![]),
                    None => {}
                }
            }
            runs.retain(|r| !r.is_empty());
            if runs.is_empty() {
                return;
            }
            if let Some(kind) = kind {
                if seen_facility_ways.insert(way.id()) {
                    let pts: Vec<_> = runs.concat();
                    let (lon, lat) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
                    let k = pts.len() as f64;
                    osm.facilities.push(Facility { kind: kind.into(), name, lon: lon / k, lat: lat / k });
                }
            } else if is_river(&tags) {
                osm.rivers.extend(runs.into_iter().filter(|r| r.len() >= 2));
            } else if let Some(rel) = in_rel {
                if runs.len() == 1 {
                    rel_parts.entry(*rel).or_default().extend(runs);
                }
            } else if runs.len() == 1 && runs[0].first() == runs[0].last() {
                osm.water.extend(rings(runs));
            } else {
                // water outline crossing the bbox edge: keep its shore as river points only
                osm.rivers.extend(runs.into_iter().filter(|r| r.len() >= 2));
            }
        })?;
        for parts in rel_parts.into_values() {
            osm.water.extend(rings(parts));
        }
    }
    Ok(Some(osm))
}

/// A road (OSM `highway=*` way) cut to the bbox, keeping node ids so ways join at shared nodes
pub struct Road {
    pub highway: String,
    pub name: Option<String>,
    /// `oneway=yes|1` (or motorway / roundabout) = 1, `oneway=-1` = -1, else 0
    pub oneway: i8,
    pub nodes: Vec<(i64, f64, f64)>,
}

const HIGHWAYS: [&str; 22] = ["motorway", "trunk", "primary", "secondary", "tertiary", "motorway_link", "trunk_link",
    "primary_link", "secondary_link", "tertiary_link", "unclassified", "residential", "living_street", "service",
    "road", "track", "path", "footway", "cycleway", "pedestrian", "steps", "bridleway"];

/// Roads and shelters (see `shelter_kind`) in the bbox from the extracts that cover it; None if no extract does.
pub fn read_roads(dir: &Path, w: f64, s: f64, e: f64, n: f64) -> Result<Option<(Vec<Road>, Vec<Facility>)>> {
    let files = extracts(dir, w, s, e, n)?;
    if files.is_empty() {
        return Ok(None);
    }
    let inside = |lon: f64, lat: f64| lon >= w && lon <= e && lat >= s && lat <= n;
    let shelter = |(k, name): (Option<&'static str>, Option<String>)| k.filter(|k| k.ends_with("shelter")).map(|k| (k, name));
    let (mut roads, mut shelters, mut seen) = (Vec::new(), Vec::new(), HashSet::new()); // extracts overlap at borders
    for path in files {
        let mut coords: HashMap<i64, (f64, f64)> = HashMap::new();
        let mut node = |id: i64, lon: f64, lat: f64, fac: (Option<&'static str>, Option<String>), shelters: &mut Vec<Facility>| {
            if inside(lon, lat) {
                coords.insert(id, (lon, lat));
                if let Some((kind, name)) = shelter(fac) {
                    shelters.push(Facility { kind: kind.into(), name, lon, lat });
                }
            }
        };
        ElementReader::from_path(&path)?.for_each(|el| match el {
            Element::Node(nd) => node(nd.id(), nd.lon(), nd.lat(), facility_kind(nd.tags()), &mut shelters),
            Element::DenseNode(nd) => node(nd.id(), nd.lon(), nd.lat(), facility_kind(nd.tags()), &mut shelters),
            _ => {}
        })?;
        ElementReader::from_path(&path)?.for_each(|el| {
            let Element::Way(way) = el else { return };
            if !seen.insert(way.id()) {
                return;
            }
            let pts: Vec<(i64, f64, f64)> = way.refs().filter_map(|id| coords.get(&id).map(|&(lon, lat)| (id, lon, lat))).collect();
            if let Some((kind, name)) = shelter(facility_kind(way.tags())) {
                if !pts.is_empty() {
                    let k = pts.len() as f64;
                    let (lon, lat) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.1 / k, a.1 + p.2 / k));
                    shelters.push(Facility { kind: kind.into(), name, lon, lat });
                }
                return;
            }
            let (mut highway, mut name, mut oneway, mut roundabout) = (None, None, 0, false);
            for (k, v) in way.tags() {
                match (k, v) {
                    ("highway", h) => highway = HIGHWAYS.iter().find(|&&x| x == h).copied(),
                    ("name", n) => name = Some(n.to_owned()),
                    ("oneway", "yes" | "1" | "true") => oneway = 1,
                    ("oneway", "-1") => oneway = -1,
                    ("junction", "roundabout") => roundabout = true,
                    _ => {}
                }
            }
            let Some(highway) = highway else { return };
            if oneway == 0 && (roundabout || highway == "motorway") {
                oneway = 1;
            }
            let mut runs: Vec<Vec<(i64, f64, f64)>> = vec![vec![]];
            for id in way.refs() {
                match coords.get(&id) {
                    Some(&(lon, lat)) => runs.last_mut().unwrap().push((id, lon, lat)),
                    None if !runs.last().unwrap().is_empty() => runs.push(vec![]),
                    None => {}
                }
            }
            for nodes in runs.into_iter().filter(|r| r.len() >= 2) {
                roads.push(Road { highway: highway.into(), name: name.clone(), oneway, nodes });
            }
        })?;
    }
    // same node-mapped shelter can come from two extracts
    let mut keys = HashSet::new();
    shelters.retain(|f: &Facility| keys.insert(((f.lon * 1e5) as i64, (f.lat * 1e5) as i64)));
    Ok(Some((roads, shelters)))
}
