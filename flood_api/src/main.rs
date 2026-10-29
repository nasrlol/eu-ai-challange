//! Flood region API: POST an alerted region, get the model features for it (and, with the
//! Python scorer, flood risk plus a priority list of cells for allocating rescue resources).
mod features;
mod help;
mod models;
mod osm;
mod raster;
mod region_check;
mod roads;
#[allow(dead_code)] // wired in step by step, see notes/08
mod types;

use anyhow::{bail, Context, Result};
use axum::{http::{header, HeaderValue, Method, StatusCode}, routing::{get, post}, Json, Router};
use tower_http::cors::{AllowOrigin, CorsLayer};
use features::{FEATURES, GRID_M};
use geo::{Contains, LineString, Point, Polygon};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{path::PathBuf, process::Command};

const HIGH_RISK: f32 = 0.5; // same threshold as risk_model.py / road_model.py

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
    #[serde(default = "yes")]
    score: bool,
}
fn yes() -> bool { true }

fn ring(req: &RegionRequest) -> Result<Vec<(f64, f64)>> {
    if let Some([w, s, e, n]) = req.bbox {
        if !(w < e && s < n) {
            bail!("bbox must be [west, south, east, north] with west < east and south < north");
        }
        return Ok(vec![(w, s), (e, s), (e, n), (w, n), (w, s)]);
    }
    let Some(mut p) = req.polygon.as_ref() else { bail!("give either bbox or polygon") };
    if p["type"] == "Feature" {
        p = &p["geometry"];
    }
    if p["type"] != "Polygon" {
        bail!("polygon must be a GeoJSON Polygon geometry (or a Feature with one), got type {}", p["type"]);
    }
    let pts = p["coordinates"][0].as_array().context("polygon.coordinates[0] missing")?;
    pts.iter().map(|c| Ok((c[0].as_f64().context("lon")?, c[1].as_f64().context("lat")?))).collect()
}

fn run(req: RegionRequest) -> Result<Value> {
    let cfg = config();
    if req.name.is_empty() || !req.name.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)) {
        bail!("name must be letters, digits, _ or -");
    }
    let ring = ring(&req)?;
    let outputs = std::fs::canonicalize(cfg.scorer.parent().context("FLOOD_SCORER has no folder")?)?.join("outputs");
    // everything cheap first: an oversized or outside region must not reach the OSM / raster build
    let km2 = region_check::check(&ring, &outputs.join("region/study_region.gpkg"))?;
    let dir = outputs.join("api_region").join(&req.name); // served by GET /files
    std::fs::create_dir_all(&dir)?;
    let reg = features::build(&ring, &cfg.data, &cfg.out.join("cache"))?;

    let csv_path = dir.join("features.csv");
    let mut wr = csv::Writer::from_path(&csv_path)?;
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
    std::fs::write(dir.join("facilities.geojson"), json!({"type": "FeatureCollection", "features": fc}).to_string())?;

    let (mut counts, mut nearby) = (std::collections::BTreeMap::new(), std::collections::BTreeMap::new());
    for (f, &inside) in reg.facilities.iter().zip(&inside) {
        *(if inside { &mut counts } else { &mut nearby }).entry(f.kind.clone()).or_insert(0) += 1;
    }
    let mut out = json!({
        "name": req.name, "cells": reg.xy.len(), "area_km2": km2, "river_points": reg.n_river_pts,
        "facilities": counts, "facilities_nearby_outside": nearby,
        "files": {"features": csv_path, "facilities": dir.join("facilities.geojson")},
    });
    if !req.score {
        return Ok(out);
    }

    // Score with the trained sklearn model (code/risk/model.joblib)
    let scored = dir.join("risk.csv");
    let st = Command::new(&cfg.python).arg(&cfg.scorer).arg(&csv_path).arg(&scored).status().context("run scorer")?;
    if !st.success() {
        bail!("scorer failed: {st}");
    }
    let mut rd = csv::Reader::from_path(&scored)?;
    let col = rd.headers()?.iter().position(|h| h == "risk").context("no risk column")?;
    let risk: Vec<f32> = rd.records().map(|r| Ok(r?[col].parse()?)).collect::<Result<_>>()?;

    // People per 100 m cell (approx., GHS-POP is 1 km); priority = expected people flooded
    let people: Vec<f32> = reg.pop_density.iter().map(|d| d * (GRID_M * GRID_M / 1e6) as f32).collect();
    let high: Vec<usize> = (0..risk.len()).filter(|&i| risk[i] >= HIGH_RISK).collect();
    let mut order: Vec<usize> = (0..risk.len()).collect();
    order.sort_by(|&a, &b| (risk[b] * people[b]).total_cmp(&(risk[a] * people[a])));
    let top: Vec<Value> = order.iter().take(20).map(|&i| json!({
        "lon": reg.lonlat[i].0, "lat": reg.lonlat[i].1, "risk": risk[i], "people": people[i],
        "dist_fire_station_m": reg.dist_fire[i], "dist_hospital_m": reg.dist_hospital[i],
    })).collect();
    out["files"]["risk"] = json!(scored);
    out["risk"] = json!({
        "mean": risk.iter().sum::<f32>() / risk.len() as f32,
        "high_risk_km2": high.len() as f64 * GRID_M * GRID_M / 1e6,
        "people_in_high_risk_approx": high.iter().map(|&i| people[i]).sum::<f32>(),
        "expected_people_flooded_approx": risk.iter().zip(&people).map(|(r, p)| r * p).sum::<f32>(),
        "priority_cells": top,
    });
    Ok(out)
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
        .allow_methods([Method::GET, Method::POST, Method::PATCH])
        .allow_headers([header::CONTENT_TYPE]))
}

#[tokio::main]
async fn main() -> Result<()> {
    let addr = std::env::var("FLOOD_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let cfg = config();
    std::fs::create_dir_all(&cfg.out)?;
    let code = std::fs::canonicalize(cfg.scorer.parent().context("FLOOD_SCORER has no folder")?)?;
    let outputs = code.join("outputs");
    let help = help::Help::open(&cfg.out.join("help.sqlite"), cfg.data.clone(), outputs.join("region/study_region.gpkg"))?;
    let models = models::Models { python: cfg.python, code, data: cfg.data };
    let app = Router::new().route("/health", get(|| async { "ok" }))
        .route("/region", post(move |extract::JsonBody(req): extract::JsonBody<RegionRequest>| {
            let outputs = outputs.clone();
            blocking(move || run(req).map(|v| models::public(v, &outputs)))
        }))
        .merge(help::router(help)).merge(models::router(models))
        .layer(cors()?);
    println!("flood_api listening on http://{addr}");
    axum::serve(tokio::net::TcpListener::bind(&addr).await?, app).await?;
    Ok(())
}
