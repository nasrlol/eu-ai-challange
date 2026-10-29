//! Flood region API: POST an alerted region, get the model features for it (and, with the
//! Python scorer, flood risk plus a priority list of cells for allocating rescue resources).
mod features;
mod help;
mod models;
mod osm;
mod raster;
mod roads;
#[allow(dead_code)] // wired in step by step, see notes/08
mod types;

use anyhow::{bail, Context, Result};
use axum::{http::StatusCode, routing::{get, post}, Json, Router};
use features::{FEATURES, GRID_M};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{path::PathBuf, process::Command};

const MAX_KM2: f64 = 3000.0; // largest training AOI is ~2,900 km²
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
    /// GeoJSON Polygon geometry (lon/lat); outer ring only
    polygon: Option<Value>,
    #[serde(default = "yes")]
    score: bool,
}
fn yes() -> bool { true }

fn ring(req: &RegionRequest) -> Result<Vec<(f64, f64)>> {
    if let Some([w, s, e, n]) = req.bbox {
        return Ok(vec![(w, s), (e, s), (e, n), (w, n), (w, s)]);
    }
    let Some(p) = &req.polygon else { bail!("give either bbox or polygon") };
    let pts = p["coordinates"][0].as_array().context("polygon.coordinates[0] missing")?;
    pts.iter().map(|c| Ok((c[0].as_f64().context("lon")?, c[1].as_f64().context("lat")?))).collect()
}

fn run(req: RegionRequest) -> Result<Value> {
    let cfg = config();
    if req.name.is_empty() || !req.name.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)) {
        bail!("name must be letters, digits, _ or -");
    }
    let ring = ring(&req)?;
    let dir = cfg.out.join(&req.name);
    std::fs::create_dir_all(&dir)?;
    let reg = features::build(&ring, &cfg.data, &cfg.out.join("cache"))?;
    let km2 = reg.xy.len() as f64 * GRID_M * GRID_M / 1e6;
    if km2 > MAX_KM2 {
        bail!("region is {km2:.0} km², limit {MAX_KM2} km²");
    }

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
    let fc: Vec<Value> = reg.facilities.iter().map(|f| json!({"type": "Feature",
        "geometry": {"type": "Point", "coordinates": [f.lon, f.lat]}, "properties": {"kind": f.kind, "name": f.name}})).collect();
    std::fs::write(dir.join("facilities.geojson"), json!({"type": "FeatureCollection", "features": fc}).to_string())?;

    let mut counts = std::collections::BTreeMap::new();
    for f in &reg.facilities {
        *counts.entry(f.kind.clone()).or_insert(0) += 1;
    }
    let mut out = json!({
        "name": req.name, "cells": reg.xy.len(), "area_km2": km2, "river_points": reg.n_river_pts,
        "facilities": counts, "files": {"features": csv_path, "facilities": dir.join("facilities.geojson")},
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

/// Runs blocking work (Python, SQLite, graph building) off the async runtime; errors become JSON
async fn blocking(f: impl FnOnce() -> Result<Value> + Send + 'static) -> (StatusCode, Json<Value>) {
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(v)) => (StatusCode::OK, Json(v)),
        Ok(Err(e)) => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error": format!("{e:#}")}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let addr = std::env::var("FLOOD_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let cfg = config();
    std::fs::create_dir_all(&cfg.out)?;
    let help = help::Help::open(&cfg.out.join("help.sqlite"), cfg.data.clone())?;
    let code = std::fs::canonicalize(cfg.scorer.parent().context("FLOOD_SCORER has no folder")?)?;
    let models = models::Models { python: cfg.python, code, data: cfg.data };
    let app = Router::new().route("/health", get(|| async { "ok" }))
        .route("/region", post(|Json(req): Json<RegionRequest>| blocking(move || run(req))))
        .merge(help::router(help)).merge(models::router(models));
    println!("flood_api listening on http://{addr}");
    axum::serve(tokio::net::TcpListener::bind(&addr).await?, app).await?;
    Ok(())
}
