//! Flood region API: POST an alerted region, get the model features for it (and, with the
//! Python scorer, flood risk plus a priority list of cells for allocating rescue resources).
mod features;
mod help;
mod jobs;
mod models;
mod osm;
mod raster;
mod region_check;
mod roads;
mod types;

use anyhow::{bail, Context, Result};
use axum::{http::{header, HeaderValue, Method, StatusCode}, routing::{get, post}, Json, Router};
use tower_http::cors::{AllowOrigin, CorsLayer};
use features::{FEATURES, GRID_M};
use types::{Factor, FactorKind, Files, Overall, PriorityCell, RegionResponse, RiskSummary, Scenario, ScenarioSummary, HIGH_RISK};
use geo::{Contains, LineString, Point, Polygon};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

struct Config {
    data: PathBuf,
    out: PathBuf,
    python: PathBuf,
    scorer: PathBuf,
}

fn config() -> Config {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let var = |k: &str, d: PathBuf| std::env::var(k).map(PathBuf::from).unwrap_or(d);
    Config {
        data: var("FLOOD_DATA", root.join("working_data")),
        out: var("FLOOD_OUT", root.join("flood_api/out")),
        python: var("FLOOD_PYTHON", root.join(".venv/bin/python")),
        scorer: var("FLOOD_SCORER", root.join("code/score_region.py")),
    }
}

#[derive(Deserialize)]
struct RegionRequest {
    /// Used as the output folder name, e.g. an alert id
    name: String,
    /// [west, south, east, north] in degrees
    bbox: Option<[f64; 4]>,
    /// GeoJSON Polygon geometry, or a Feature holding one (lon/lat); outer ring only
    polygon: Option<Value>,
    /// Same as the model routes: {"type":"bbox","bbox":[..]} or {"type":"polygon","geometry":{..}}
    region: Option<Value>,
    #[serde(default = "yes")]
    score: bool,
    /// Number of priority cells returned
    #[serde(default = "top_n")]
    top_n: usize,
    /// EFAS return periods summarised in risk.by_scenario; empty = all three
    #[serde(default)]
    scenarios: Vec<Scenario>,
}
fn yes() -> bool { true }
fn top_n() -> usize { 20 }
const MAX_TOP_N: usize = 500;

fn ring(req: &RegionRequest) -> Result<Vec<(f64, f64)>> {
    let (mut bbox, mut polygon) = (req.bbox, req.polygon.as_ref());
    if let Some(r) = &req.region {
        if bbox.is_some() || polygon.is_some() {
            bail!("give either region, bbox or polygon");
        }
        match r["type"].as_str() {
            Some("bbox") => bbox = Some(serde_json::from_value(r["bbox"].clone()).context("region.bbox must be [west, south, east, north]")?),
            Some("polygon") => polygon = Some(&r["geometry"]),
            Some("aoi") => bail!("/region takes a bbox or polygon; for a CEMS AOI use /flood, /timeline or /evacuation"),
            _ => bail!("region.type must be bbox or polygon"),
        }
    }
    if let Some([w, s, e, n]) = bbox {
        if !(w < e && s < n) {
            bail!("bbox must be [west, south, east, north] with west < east and south < north");
        }
        return Ok(vec![(w, s), (e, s), (e, n), (w, n), (w, s)]);
    }
    let Some(mut p) = polygon else { bail!("give either bbox or polygon") };
    if p["type"] == "Feature" {
        p = &p["geometry"];
    }
    if p["type"] != "Polygon" {
        bail!("polygon must be a GeoJSON Polygon geometry (or a Feature with one), got type {}", p["type"]);
    }
    let pts = p["coordinates"][0].as_array().context("polygon.coordinates[0] missing")?;
    pts.iter().map(|c| Ok((c[0].as_f64().context("lon")?, c[1].as_f64().context("lat")?))).collect()
}

/// Factors of access_model.py that /region can fill from its grid; the rest need roads/buildings
fn factors(risk: f32, people: f32, dist_fire: f32, dist_hospital: f32) -> Vec<Factor> {
    let help_km = dist_fire.min(dist_hospital) / 1000.0; // NaN only if both are missing
    FactorKind::ALL.iter().map(|&k| match k {
        FactorKind::Likelihood => Factor::new(k, risk, format!("relative terrain risk {risk:.2} (a ranking, not a probability)")),
        FactorKind::People => Factor::new(k, people, format!("~{people:.0} people live in this 100 m cell (GHS-POP, approx.)")),
        FactorKind::HelpKm if help_km.is_nan() => Factor::new(k, f32::NAN, "no fire station or hospital found nearby in OSM".into()),
        FactorKind::HelpKm => Factor::new(k, help_km,
            format!("nearest fire station / hospital {help_km:.1} km straight-line (by road is longer)")),
        _ => Factor::new(k, f32::NAN, "needs roads and buildings: use POST /flood for this area".into()),
    }).collect()
}

fn run(req: RegionRequest) -> Result<Value> {
    let cfg = config();
    if req.name.is_empty() || !req.name.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)) {
        bail!("name must be letters, digits, _ or -");
    }
    if req.top_n > MAX_TOP_N {
        bail!("top_n must be at most {MAX_TOP_N}; the full grid is in files.risk");
    }
    let ring = ring(&req)?;
    let outputs = std::fs::canonicalize(cfg.scorer.parent().context("FLOOD_SCORER has no folder")?)?.join("outputs");
    // everything cheap first: an oversized or outside region must not reach the OSM / raster build
    let km2 = region_check::check(&ring, &outputs.join("region/study_region.gpkg"))?;
    let dir = outputs.join("api_region").join(&req.name); // served by GET /files
    std::fs::create_dir_all(&dir)?;
    let reg = features::build(&ring, &cfg.data, &cfg.out.join("cache"))?;

    let files = Files { features: dir.join("features.csv"), grid: dir.join("grid.npy"), facilities: dir.join("facilities.geojson"),
                            ..Default::default() };
    types::write_npy(&files.grid, &reg.xy, &reg.feats)?;
    let mut wr = csv::Writer::from_path(&files.features)?;
    let mut header = vec!["x", "y", "lon", "lat"];
    header.extend(FEATURES);
    header.extend(["pop_density", "dist_fire_station_m", "dist_hospital_m"]);
    wr.write_record(&header)?;
    for i in 0..reg.xy.len() {
        let mut row = vec![reg.xy[i].0.to_string(), reg.xy[i].1.to_string(), format!("{:.6}", reg.lonlat[i].0), format!("{:.6}", reg.lonlat[i].1)];
        row.extend(reg.feats[i].iter().map(|v| v.to_string()));
        row.extend([reg.pop_density[i], reg.dist_fire[i], reg.dist_hospital[i]].map(|v| v.to_string()));
        wr.write_record(&row)?;
    }
    wr.flush()?;
    // Facilities come from the padded bbox (needed for distances); counts are for the region itself
    let area = Polygon::new(LineString::from(ring.clone()), vec![]);
    let inside: Vec<bool> = reg.facilities.iter().map(|f| area.contains(&Point::new(f.lon, f.lat))).collect();
    let fc: Vec<Value> = reg.facilities.iter().zip(&inside).map(|(f, inside)| json!({"type": "Feature",
        "geometry": {"type": "Point", "coordinates": [f.lon, f.lat]}, "properties": {"kind": f.kind, "name": f.name, "inside": inside}})).collect();
    std::fs::write(&files.facilities, json!({"type": "FeatureCollection", "features": fc}).to_string())?;

    let (mut counts, mut nearby) = (BTreeMap::new(), BTreeMap::new());
    for (f, &inside) in reg.facilities.iter().zip(&inside) {
        *(if inside { &mut counts } else { &mut nearby }).entry(f.kind.clone()).or_insert(0) += 1;
    }
    let mut out = RegionResponse {
        name: req.name.clone(), cells: reg.xy.len(), area_km2: km2, river_points: reg.n_river_pts,
        facilities: counts, facilities_nearby_outside: nearby, risk: None, region_check: None, files,
        sources: types::SOURCES, caveats: types::CAVEATS.to_vec(),
    };
    if reg.n_river_pts < 100 {
        out.caveats.push("few OSM river points in this region: hand and dist_river are less reliable");
    }
    if !req.score {
        return Ok(serde_json::to_value(out)?);
    }

    // Score with the trained sklearn model (code/outputs/risk/model.joblib)
    let scored = dir.join("risk.csv");
    let st = Command::new(&cfg.python).arg(&cfg.scorer).arg(&out.files.grid).arg(&scored).status().context("run scorer")?;
    if !st.success() {
        bail!("scorer failed: {st}");
    }
    let mut rd = csv::Reader::from_path(&scored)?;
    let col = rd.headers()?.iter().position(|h| h == "risk").context("no risk column")?;
    let risk: Vec<f32> = rd.records().map(|r| Ok(r?[col].parse()?)).collect::<Result<_>>()?;
    if risk.len() != reg.xy.len() {
        bail!("scorer returned {} rows for {} cells", risk.len(), reg.xy.len());
    }
    let check = std::fs::read_to_string(dir.join("risk_check.json")).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
    if check.as_ref().and_then(|c| c["cells_outside_training_range_pct"].as_f64()).is_some_and(|p| p > 10.0) {
        out.caveats.push("over 10% of cells lie outside the training range of at least one feature: treat the risk with extra care");
    }
    out.region_check = check;
    out.files.cells = Some(write_cells(&dir.join("cells.geojson"), &reg, &risk)?);
    out.files.risk = Some(scored.clone());
    out.files.risk_tif = Some(scored.with_extension("tif"));

    let people: Vec<f32> = reg.pop_density.iter().map(|&d| types::people(d)).collect();
    let high: Vec<usize> = (0..risk.len()).filter(|&i| risk[i] >= HIGH_RISK).collect();
    let scenarios = if req.scenarios.is_empty() { Scenario::ALL.to_vec() } else { req.scenarios.clone() };
    let cell_km2 = GRID_M * GRID_M / 1e6;
    let by_scenario = scenarios.iter().map(|&sc| {
        let d: Vec<f32> = reg.feats.iter().map(|f| f[sc.feature()]).collect();
        let wet: Vec<usize> = (0..d.len()).filter(|&i| d[i] > 0.0).collect();
        ScenarioSummary { scenario: sc, flooded_km2: wet.len() as f64 * cell_km2,
                          deep_km2: d.iter().filter(|&&v| v >= 0.3).count() as f64 * cell_km2,
                          people_approx: wet.iter().map(|&i| people[i]).sum(), max_depth_m: d.iter().copied().fold(0.0, f32::max) }
    }).collect();
    // Priority = risk × people (where to look first), not expected casualties
    let mut order: Vec<usize> = (0..risk.len()).collect();
    order.sort_by(|&a, &b| (risk[b] * people[b]).total_cmp(&(risk[a] * people[a])));
    let opt = |v: f32| (!v.is_nan()).then_some(v);
    let priority_cells = order.iter().take(req.top_n).enumerate().map(|(rank, &i)| {
        let factors = factors(risk[i], people[i], reg.dist_fire[i], reg.dist_hospital[i]);
        PriorityCell { rank: rank + 1, lon: reg.lonlat[i].0, lat: reg.lonlat[i].1, risk: risk[i], people: people[i],
                       dist_fire_station_m: opt(reg.dist_fire[i]), dist_hospital_m: opt(reg.dist_hospital[i]),
                       overall: Overall::from_levels(&factors.iter().map(|f| f.level).collect::<Vec<_>>()), factors }
    }).collect();
    out.risk = Some(RiskSummary {
        mean: risk.iter().sum::<f32>() / risk.len() as f32,
        high_risk_km2: high.len() as f64 * cell_km2,
        people_in_high_risk_approx: high.iter().map(|&i| people[i]).sum(),
        risk_weighted_people_index: risk.iter().zip(&people).map(|(r, p)| r * p).sum(),
        by_scenario, priority_cells,
    });
    Ok(serde_json::to_value(out)?)
}

/// Cells worth drawing (model risk >= 0.2 or wet in any EFAS scenario) as 100 m squares in lon/lat,
/// properties kept short for the browser: r = risk, d10 / d100 / d500 = EFAS depth in m, pop =
/// people in the cell (GHS-POP, as in the risk summary), elev = DEM metres (the app's "Animate this
/// area" fills from low to high ground)
fn write_cells(path: &std::path::Path, reg: &features::Region, risk: &[f32]) -> Result<PathBuf> {
    let laea = features::Laea::new();
    let h = GRID_M / 2.0;
    let r = |v: f64, k: f64| (v * k).round() / k;
    let feats: Vec<Value> = (0..risk.len()).filter(|&i| risk[i] >= 0.2 || reg.feats[i][7] > 0.0).map(|i| {
        let (x, y) = reg.xy[i];
        let ring: Vec<[f64; 2]> = [(-h, -h), (h, -h), (h, h), (-h, h), (-h, -h)].iter()
            .map(|&(dx, dy)| { let (lon, lat) = laea.inv(x + dx, y + dy); [r(lon, 1e5), r(lat, 1e5)] }).collect();
        let f = &reg.feats[i];
        json!({"type": "Feature", "geometry": {"type": "Polygon", "coordinates": [ring]},
               "properties": {"r": r(risk[i] as f64, 100.0), "d10": r(f[5] as f64, 100.0), "d100": r(f[6] as f64, 100.0), "d500": r(f[7] as f64, 100.0),
                              "pop": r(types::people(reg.pop_density[i]) as f64, 10.0), "elev": r(f[0] as f64, 10.0)}})
    }).collect();
    std::fs::write(path, json!({"type": "FeatureCollection", "features": feats}).to_string())?;
    Ok(path.to_path_buf())
}

#[derive(Deserialize)]
struct PointQuery {
    lon: f64,
    lat: f64,
}

/// Danger at one place from the EFAS flood maps, per scenario (fast: no OSM or model run). Depth
/// thresholds as access_model.py: >= 1 m no refuge on the ground floor, >= 0.3 m flooded / roads
/// impassable. EFAS is 90 m, so water within NEAR_M also counts: the place is at risk, not safe.
const NEAR_M: f64 = 100.0;
const LOOK_DEG: f64 = 0.003; // ~300 m search window for the nearest water

fn point(q: PointQuery) -> Result<Value> {
    let cfg = config();
    let outputs = std::fs::canonicalize(cfg.scorer.parent().context("FLOOD_SCORER has no folder")?)?.join("outputs");
    let (lon, lat, e) = (q.lon, q.lat, LOOK_DEG);
    // the search box gets the same shape / study-region check as /region
    region_check::check(&[(lon - e, lat - e), (lon + e, lat - e), (lon + e, lat + e), (lon - e, lat + e), (lon - e, lat - e)],
                        &outputs.join("region/study_region.gpkg"))?;
    let (my, mx) = (111_320.0, 111_320.0 * lat.to_radians().cos());
    let r2 = |v: f32| (v as f64 * 100.0).round() / 100.0;
    let mut by = vec![];
    let mut worst = "safe";
    for sc in Scenario::ALL {
        let g = raster::read_window(&cfg.data.join(format!("cems-efas/Europe_RP{}_filled_depth.tif", sc.years())), lon - e, lat - e, lon + e, lat + e)?;
        let here = g.at(lon, lat).max(0.0);
        // nearest pixel (centre) with >= 0.3 m of water
        let mut near: Option<(f64, f32)> = None;
        for row in 0..g.height {
            for col in 0..g.width {
                let d = g.get(row, col);
                if d >= 0.3 {
                    let (plon, plat) = (g.x0 + (col as f64 + 0.5) * g.dx, g.y0 - (row as f64 + 0.5) * g.dy);
                    let m = ((plon - lon) * mx).hypot((plat - lat) * my);
                    if near.map_or(true, |(nm, _)| m < nm) {
                        near = Some((m, d));
                    }
                }
            }
        }
        let (level, text) = if here >= 1.0 { ("danger", "deep water here (≥ 1 m): no safe place on the ground floor, leave early".to_string()) }
            else if here >= 0.3 { ("danger", "flooded here (≥ 0.3 m): roads impassable, buildings take water".to_string()) }
            else if let Some((m, d)) = near.filter(|&(m, _)| m <= NEAR_M) { ("at_risk", format!("flood water {d:.1} m deep about {m:.0} m away: roads nearby may close")) }
            else if here > 0.0 { ("at_risk", "shallow water here (< 0.3 m)".to_string()) }
            else if let Some((m, _)) = near { ("safe", format!("dry here; nearest flood water about {m:.0} m away")) }
            else { ("safe", "dry here and within ~300 m".to_string()) };
        if level == "danger" || (level == "at_risk" && worst == "safe") {
            worst = level;
        }
        by.push(json!({"scenario": sc, "efas_depth_m": r2(here), "level": level, "text": text,
                       "nearest_water_m": near.map(|(m, _)| m.round()), "nearest_water_depth_m": near.map(|(_, d)| r2(d))}));
    }
    Ok(json!({"lon": lon, "lat": lat, "level": worst, "by_scenario": by,
              "caveats": ["EFAS flood maps are 90 m and model river floods only (no heavy-rain runoff, no dam breaks)",
                          "depth is the raw EFAS value at this pixel, not checked against the local ground height",
                          "for buildings cut off by water and evacuation advice, run the flood check on the area"]}))
}

/// Errors with their own status: NotFound (404) for a well-formed request about something that
/// does not exist, Conflict (409) when the current state forbids it. Everything else is 422.
macro_rules! status_error {
    ($name:ident) => {
        #[derive(Debug)]
        pub struct $name(pub String);
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl std::error::Error for $name {}
    };
}
status_error!(NotFound);
status_error!(Conflict);

/// Json / Query / Path extractors whose rejections are JSON `{"error"}` like every other error
/// (axum's own are plain text): 400 for unparseable JSON, 422 for valid JSON of the wrong shape.
pub mod extract {
    use axum::{async_trait, extract::{FromRequest, FromRequestParts, Request}, http::{request::Parts, StatusCode}, Json};
    use serde::de::DeserializeOwned;
    use serde_json::{json, Value};

    type Rejection = (StatusCode, Json<Value>);
    fn reject(status: StatusCode, msg: String) -> Rejection {
        let status = if status == StatusCode::BAD_REQUEST || status == StatusCode::UNSUPPORTED_MEDIA_TYPE { status }
                     else { StatusCode::UNPROCESSABLE_ENTITY };
        (status, Json(json!({"error": msg})))
    }

    pub struct JsonBody<T>(pub T);
    #[async_trait]
    impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for JsonBody<T> {
        type Rejection = Rejection;
        async fn from_request(req: Request, state: &S) -> Result<Self, Rejection> {
            Json::<T>::from_request(req, state).await.map(|Json(v)| JsonBody(v)).map_err(|e| reject(e.status(), e.body_text()))
        }
    }

    pub struct QueryArgs<T>(pub T);
    #[async_trait]
    impl<T: DeserializeOwned, S: Send + Sync> FromRequestParts<S> for QueryArgs<T> {
        type Rejection = Rejection;
        async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Rejection> {
            axum::extract::Query::<T>::from_request_parts(parts, state).await.map(|q| QueryArgs(q.0))
                .map_err(|e| reject(StatusCode::UNPROCESSABLE_ENTITY, e.body_text()))
        }
    }

    pub struct PathArg<T>(pub T);
    #[async_trait]
    impl<T: DeserializeOwned + Send, S: Send + Sync> FromRequestParts<S> for PathArg<T> {
        type Rejection = Rejection;
        async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Rejection> {
            axum::extract::Path::<T>::from_request_parts(parts, state).await.map(|p| PathArg(p.0))
                .map_err(|e| reject(StatusCode::UNPROCESSABLE_ENTITY, e.body_text()))
        }
    }
}

/// Runs blocking work (Python, SQLite, graph building) off the async runtime; errors become JSON
async fn blocking(f: impl FnOnce() -> Result<Value> + Send + 'static) -> (StatusCode, Json<Value>) {
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(v)) => (StatusCode::OK, Json(v)),
        Ok(Err(e)) if e.downcast_ref::<NotFound>().is_some() => (StatusCode::NOT_FOUND, Json(json!({"error": format!("{e:#}")}))),
        Ok(Err(e)) if e.downcast_ref::<Conflict>().is_some() => (StatusCode::CONFLICT, Json(json!({"error": format!("{e:#}")}))),
        Ok(Err(e)) => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error": format!("{e:#}")}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))),
    }
}

/// CORS for browser frontends on every route: origins from FLOOD_CORS_ORIGINS (comma list,
/// default "*" = any; tighten it if the API is ever public)
fn cors() -> Result<CorsLayer> {
    let origins = std::env::var("FLOOD_CORS_ORIGINS").unwrap_or_else(|_| "*".into());
    let allow = if origins.trim() == "*" {
        AllowOrigin::any()
    } else {
        let list = origins.split(',').map(str::trim).filter(|o| !o.is_empty())
            .map(|o| HeaderValue::from_str(o).with_context(|| format!("FLOOD_CORS_ORIGINS: bad origin {o:?}")))
            .collect::<Result<Vec<_>>>()?;
        AllowOrigin::list(list)
    };
    Ok(CorsLayer::new().allow_origin(allow)
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE]))
}

/// OSM store folder: FLOOD_OSM_STORE, default <FLOOD_OUT>/osm_store
fn store_dir(cfg: &Config) -> PathBuf {
    std::env::var("FLOOD_OSM_STORE").map(PathBuf::from).unwrap_or_else(|_| cfg.out.join("osm_store"))
}

/// `flood_api build-osm-store`: study-region bbox + STORE_PAD_DEG (covers the 0.02° feature pad and
/// regions up to 1 % outside the hull). `flood_api check-osm-store w s e n`: store vs extracts.
const STORE_PAD_DEG: f64 = 0.15;

fn cli(cmd: &str, args: &[String]) -> Result<()> {
    let cfg = config();
    let (pbf_dir, dir) = (cfg.data.join("osm"), store_dir(&cfg));
    let code = std::fs::canonicalize(cfg.scorer.parent().context("FLOOD_SCORER has no folder")?)?;
    match cmd {
        "build-osm-store" => {
            let [w, s, e, n] = region_check::hull_bbox(&code.join("outputs/region/study_region.gpkg"))?;
            let p = STORE_PAD_DEG;
            osm::store::build(&pbf_dir, &dir, [w - p, s - p, e + p, n + p])
        }
        "check-osm-store" => {
            let v: Vec<f64> = args.iter().map(|a| a.parse()).collect::<std::result::Result<_, _>>().context("give w s e n")?;
            let [w, s, e, n] = v[..] else { bail!("give w s e n") };
            let st = osm::store::open_at(&dir, &pbf_dir)?;
            let t = std::time::Instant::now();
            let (a, ra) = (st.osm(w, s, e, n)?.context("bbox outside the store")?, st.roads(w, s, e, n)?.context("bbox outside the store")?);
            let ts = t.elapsed().as_secs_f64();
            let t = std::time::Instant::now();
            let (b, rb) = (osm::pbf_read(&pbf_dir, w, s, e, n)?.context("no extract")?, osm::pbf_read_roads(&pbf_dir, w, s, e, n)?.context("no extract")?);
            let tp = t.elapsed().as_secs_f64();
            let pts = |o: &osm::Osm| (o.rivers.iter().map(Vec::len).sum::<usize>(), o.water.iter().map(Vec::len).sum::<usize>());
            let nodes = |r: &[osm::Road]| r.iter().map(|x| x.nodes.len()).sum::<usize>();
            let fac = |o: &osm::Osm| { let mut v: Vec<String> = o.facilities.iter().map(|f| format!("{} {:.6} {:.6}", f.kind, f.lon, f.lat)).collect(); v.sort(); v.dedup(); v };
            // the extracts overlap at borders and pbf::read keeps both copies of a water way; compare unique shapes
            // vertices sorted: a ring stitched from relation parts may start elsewhere or run the other way
            let uniq = |ls: &[Vec<(f64, f64)>]| ls.iter().map(|l| {
                let mut v: Vec<_> = l.iter().map(|p| ((p.0 * 1e7) as i64, (p.1 * 1e7) as i64)).collect(); v.sort(); v.dedup(); v
            }).collect::<std::collections::BTreeSet<_>>();
            println!("store {ts:.2} s vs extracts {tp:.1} s");
            println!("unique rivers same: {} ({} vs {}) · unique water rings same: {} ({} vs {})", uniq(&a.rivers) == uniq(&b.rivers), uniq(&a.rivers).len(),
                     uniq(&b.rivers).len(), uniq(&a.water) == uniq(&b.water), uniq(&a.water).len(), uniq(&b.water).len());
            println!("river pts {:?} vs {:?} · water rings {} vs {} · facilities {} vs {} (same set: {})", pts(&a), pts(&b), a.water.len(), b.water.len(),
                     fac(&a).len(), fac(&b).len(), fac(&a) == fac(&b));
            println!("roads {} vs {} · road nodes {} vs {} · shelters {} vs {}", ra.0.len(), rb.0.len(), nodes(&ra.0), nodes(&rb.0), ra.1.len(), rb.1.len());
            Ok(())
        }
        _ => bail!("unknown command {cmd}; use build-osm-store or check-osm-store w s e n"),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(cmd) = args.get(1) {
        return cli(cmd, &args[2..]);
    }
    let addr = std::env::var("FLOOD_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let cfg = config();
    std::fs::create_dir_all(&cfg.out)?;
    osm::store::init(store_dir(&cfg), &cfg.data.join("osm"));
    let code = std::fs::canonicalize(cfg.scorer.parent().context("FLOOD_SCORER has no folder")?)?;
    let outputs = code.join("outputs");
    let help = help::Help::open(&cfg.out.join("help.sqlite"), cfg.data.clone(), outputs.join("region/study_region.gpkg"))?;
    let models = models::Models { python: cfg.python, code, data: cfg.data };
    let jobs = jobs::Jobs::default();
    let app = Router::new().route("/health", get(|| async { "ok" }))
        .route("/region", post(move |extract::JsonBody(req): extract::JsonBody<RegionRequest>| {
            let outputs = outputs.clone();
            blocking(move || run(req).map(|v| models::public(v, &outputs)))
        }))
        .route("/point", get(|extract::QueryArgs(q): extract::QueryArgs<PointQuery>| blocking(move || point(q))))
        .merge(help::router(help)).merge(models::router(models))
        .route("/jobs/:id", get(jobs::get).with_state(jobs.clone()))
        .layer(axum::middleware::from_fn_with_state(jobs, jobs::layer))
        .layer(cors()?);
    println!("flood_api listening on http://{addr}");
    axum::serve(tokio::net::TcpListener::bind(&addr).await?, app).await?;
    Ok(())
}
