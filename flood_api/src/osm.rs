//! OpenStreetMap data for a region via Overpass: rivers and water bodies (stand-in for the CEMS
//! hydrography layers the model was trained on) and emergency facilities.
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::Path, time::Duration};

#[path = "pbf.rs"]
mod pbf;

const MIRRORS: [&str; 3] = [
    "https://overpass-api.de/api/interpreter",
    "https://overpass.private.coffee/api/interpreter",
    "https://maps.mail.ru/osm/tools/overpass/api/interpreter",
];
const USER_AGENT: &str = "eu-ai-challenge-flood-api/0.1";

#[derive(Serialize, Deserialize, Clone)]
pub struct Facility {
    pub kind: String,
    pub name: Option<String>,
    pub lon: f64,
    pub lat: f64,
}

#[derive(Serialize, Deserialize)]
pub struct Osm {
    /// River / stream / canal centrelines, lon/lat
    pub rivers: Vec<Vec<(f64, f64)>>,
    /// Closed outer rings of lakes, reservoirs, river areas and wetlands, lon/lat
    pub water: Vec<Vec<(f64, f64)>>,
    pub facilities: Vec<Facility>,
}

/// OSM data for the bbox: local Geofabrik extracts in `pbf_dir` if any cover it, else Overpass.
/// The parsed result is cached on disk by bbox.
pub fn fetch(w: f64, s: f64, e: f64, n: f64, pbf_dir: &Path, cache_dir: &Path) -> Result<Osm> {
    let bb = format!("{s:.4},{w:.4},{n:.4},{e:.4}");
    let cache = cache_dir.join(format!("osm_{}.json", bb.replace(',', "_")));
    if cache.exists() {
        return Ok(serde_json::from_str(&std::fs::read_to_string(&cache)?)?);
    }
    let mut osm = match pbf_dir.is_dir().then(|| pbf::read(pbf_dir, w, s, e, n)).transpose()?.flatten() {
        Some(osm) => osm,
        None => parse(&serde_json::from_str(&overpass(&bb)?)?)?,
    };
    // extracts overlap at borders: drop facilities found twice
    let mut seen = std::collections::HashSet::new();
    osm.facilities.retain(|f| seen.insert((f.kind.clone(), (f.lon * 1e5) as i64, (f.lat * 1e5) as i64)));
    std::fs::create_dir_all(cache_dir)?;
    std::fs::write(&cache, serde_json::to_string(&osm)?)?;
    Ok(osm)
}

fn overpass(bb: &str) -> Result<String> {
    {
        let q = format!(
            r#"[out:json][timeout:180];
(way["waterway"~"^(river|stream|canal|riverbank)$"]({bb});
 way["natural"~"^(water|wetland)$"]({bb});
 relation["natural"~"^(water|wetland)$"]({bb});
 relation["waterway"="riverbank"]({bb}););
out geom;
(nwr["amenity"~"^(fire_station|hospital|police)$"]({bb});
 nwr["emergency"="ambulance_station"]({bb}););
out center tags;"#
        );
        let client = reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(200))
            .build()?;
        let mut last = String::new();
        let mut got = None;
        for url in MIRRORS {
            match client.post(url).form(&[("data", &q)]).send() {
                Ok(r) if r.status().is_success() => {
                    got = Some(r.text()?);
                    break;
                }
                Ok(r) => last = format!("{url}: HTTP {}", r.status()),
                Err(err) => last = format!("{url}: {err}"),
            }
        }
        let Some(text) = got else { bail!("all Overpass mirrors failed, last: {last}") };
        Ok(text)
    }
}

fn line(geom: &Value) -> Vec<(f64, f64)> {
    geom.as_array()
        .map(|pts| pts.iter().filter_map(|p| Some((p["lon"].as_f64()?, p["lat"].as_f64()?))).collect())
        .unwrap_or_default()
}

/// Join open member ways end to end into closed rings (multipolygon outers are often split).
fn rings(mut parts: Vec<Vec<(f64, f64)>>) -> Vec<Vec<(f64, f64)>> {
    let mut out = Vec::new();
    while let Some(mut ring) = parts.pop() {
        while ring.first() != ring.last() {
            let end = *ring.last().unwrap();
            let Some(i) = parts.iter().position(|p| p.first() == Some(&end) || p.last() == Some(&end)) else { break };
            let mut next = parts.swap_remove(i);
            if next.first() != Some(&end) {
                next.reverse();
            }
            ring.extend(next.into_iter().skip(1));
        }
        if ring.len() >= 4 && ring.first() == ring.last() {
            out.push(ring);
        }
    }
    out
}

fn parse(v: &Value) -> Result<Osm> {
    let mut osm = Osm { rivers: vec![], water: vec![], facilities: vec![] };
    for el in v["elements"].as_array().into_iter().flatten() {
        let tags = &el["tags"];
        let tag = |k: &str| tags[k].as_str().unwrap_or("");
        let kind = match (tag("amenity"), tag("emergency")) {
            (a @ ("fire_station" | "hospital" | "police"), _) => Some(a),
            (_, "ambulance_station") => Some("ambulance_station"),
            _ => None,
        };
        if let Some(kind) = kind {
            let (lon, lat) = match el["type"].as_str() {
                Some("node") => (el["lon"].as_f64(), el["lat"].as_f64()),
                _ => (el["center"]["lon"].as_f64(), el["center"]["lat"].as_f64()),
            };
            if let (Some(lon), Some(lat)) = (lon, lat) {
                let name = tags["name"].as_str().map(str::to_owned);
                osm.facilities.push(Facility { kind: kind.into(), name, lon, lat });
            }
            continue;
        }
        match el["type"].as_str() {
            Some("way") if matches!(tag("waterway"), "river" | "stream" | "canal") => osm.rivers.push(line(&el["geometry"])),
            Some("way") => osm.water.extend(rings(vec![line(&el["geometry"])])),
            Some("relation") => {
                let outers = el["members"].as_array().into_iter().flatten()
                    .filter(|m| m["role"] == "outer")
                    .map(|m| line(&m["geometry"]))
                    .collect();
                osm.water.extend(rings(outers));
            }
            _ => {}
        }
    }
    Ok(osm)
}
