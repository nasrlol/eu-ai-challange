//! Endpoints for the Python models in code/. Region models (flood_tool, timeline, pipeline) run
//! on request as subprocesses, like the scorer in /region; models trained on the CEMS AOIs
//! (road_model, interp_model, evaluate, report) serve their stored outputs.
use anyhow::{bail, Context, Result};
use crate::extract::{JsonBody, PathArg};
use crate::{blocking, NotFound};
use axum::{body::Body, extract::State, http::{header, StatusCode}, response::{IntoResponse, Response}, routing::{get, post}, Json, Router};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::{path::PathBuf, process::Command};

#[derive(Clone)]
pub struct Models {
    pub python: PathBuf,
    /// code/ folder: scripts, and their results in code/outputs
    pub code: PathBuf,
    pub data: PathBuf,
}

const ROAD_DAMAGE_NOTE: &str = "Road damage model trained on CEMS grading maps, leave-one-area-out; \
    damage_prob is not calibrated. Impassable = EFAS depth >= 0.3 m.";

/// Output / zone names: letters, digits, _ or -
pub fn check_name(name: &str) -> Result<()> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)) {
        bail!("name must be letters, digits, _ or -");
    }
    Ok(())
}

/// Region as the Python tools take it: "w,s,e,n", a CEMS AOI name (e.g. EMSR517_AOI15_GRA), or a
/// GeoJSON polygon written to a file
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Region {
    Bbox { bbox: [f64; 4] },
    Aoi { aoi: String },
    Polygon { geometry: Value },
}

impl Models {
    fn out(&self) -> PathBuf {
        self.code.join("outputs")
    }

    fn public(&self, v: Value) -> Value {
        public(v, &self.out())
    }

    fn region_arg(&self, r: &Region, name: &str) -> Result<String> {
        Ok(match r {
            Region::Bbox { bbox: [w, s, e, n] } => {
                // same shape rule as /region; Python would silently reorder an inverted bbox
                if !([w, s, e, n].iter().all(|v| v.is_finite()) && w < e && s < n && w.abs() <= 180.0 && e.abs() <= 180.0 && s.abs() <= 90.0 && n.abs() <= 90.0) {
                    bail!("bbox must be [west, south, east, north] in degrees with west < east and south < north");
                }
                format!("{w},{s},{e},{n}")
            }
            Region::Aoi { aoi } => {
                self.cems_folder(aoi)?; // 422 if malformed, 404 if unknown, before Python
                aoi.clone()
            }
            Region::Polygon { geometry } => {
                let dir = self.out().join("api_regions");
                std::fs::create_dir_all(&dir)?;
                let path = dir.join(format!("{name}.geojson"));
                let fc = json!({"type": "FeatureCollection", "crs": {"type": "name", "properties": {"name": "EPSG:4326"}},
                                "features": [{"type": "Feature", "geometry": geometry, "properties": {}}]});
                std::fs::write(&path, fc.to_string())?;
                path.to_string_lossy().into()
            }
        })
    }

    /// Run a script in code/; on failure return only its last stderr line (the scripts exit with a
    /// one-line message; for an uncaught exception that is the exception line, not the traceback)
    fn python(&self, script: &str, args: &[String]) -> Result<String> {
        let out = Command::new(&self.python).arg(self.code.join(script)).args(args).current_dir(&self.code)
            .output().with_context(|| format!("run {script}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no error message");
            bail!("{script}: {}", last.trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into())
    }

    /// CEMS PRODUCT vector folder of an AOI with its product type, e.g. EMSR517_AOI15_GRA
    fn cems_folder(&self, aoi: &str) -> Result<PathBuf> {
        check_aoi(aoi)?;
        if aoi.split('_').count() != 3 {
            bail!("AOI {aoi:?} needs its product type here, e.g. {aoi}_DEL or {aoi}_GRA");
        }
        for d in std::fs::read_dir(&self.data)?.flatten().filter(|d| d.file_name().to_string_lossy().starts_with("ems_")) {
            for f in std::fs::read_dir(d.path())?.flatten() {
                let n = f.file_name().to_string_lossy().into_owned();
                if n.starts_with(&format!("{aoi}_PRODUCT_")) && n.ends_with("_vector") {
                    return Ok(f.path());
                }
            }
        }
        Err(NotFound(format!("unknown AOI {aoi}: no CEMS PRODUCT folder")).into())
    }
}

/// Replace every absolute path under `outputs` (code/outputs) in a response with its /files/ URL
pub fn public(mut v: Value, outputs: &std::path::Path) -> Value {
    fn walk(v: &mut Value, root: &str) {
        match v {
            Value::String(s) if s.starts_with(root) => *s = format!("/files/{}", s[root.len()..].trim_start_matches('/')),
            Value::Array(a) => a.iter_mut().for_each(|x| walk(x, root)),
            Value::Object(o) => o.values_mut().for_each(|x| walk(x, root)),
            _ => {}
        }
    }
    walk(&mut v, &outputs.to_string_lossy());
    v
}

/// CSV as a list of objects; numbers parsed, empty cells null
fn csv_rows(path: &std::path::Path, keep: impl Fn(&csv::StringRecord, &csv::StringRecord) -> bool) -> Result<Vec<Value>> {
    let mut rd = csv::Reader::from_path(path).with_context(|| format!("{} (run the model first)", path.display()))?;
    let head = rd.headers()?.clone();
    let mut rows = vec![];
    for rec in rd.records() {
        let rec = rec?;
        if !keep(&head, &rec) {
            continue;
        }
        let obj: Map<String, Value> = head.iter().zip(rec.iter()).map(|(k, v)| {
            let val = if v.is_empty() { Value::Null } else { v.parse::<f64>().map_or_else(|_| json!(v), |x| json!(x)) };
            (k.to_string(), val)
        }).collect();
        rows.push(Value::Object(obj));
    }
    Ok(rows)
}

/// AOI code: EMSR<3 digits>_AOI<2 digits>, optionally _DEL or _GRA
fn check_aoi(aoi: &str) -> Result<()> {
    let p: Vec<&str> = aoi.split('_').collect();
    let digits = |s: Option<&str>, n: usize| s.is_some_and(|s| s.len() == n && s.bytes().all(|b| b.is_ascii_digit()));
    let ok = (p.len() == 2 || p.len() == 3)
        && digits(p[0].strip_prefix("EMSR"), 3)
        && digits(p[1].strip_prefix("AOI"), 2)
        && p.get(2).map_or(true, |t| matches!(*t, "DEL" | "GRA"));
    if !ok {
        bail!("AOI {aoi:?} is not a code like EMSR517_AOI15 or EMSR517_AOI15_GRA");
    }
    Ok(())
}

/// Rows whose `col` is the AOI, or the AOI followed by `_` (product type, observation pair)
fn by_aoi<'a>(col: &'a str, aoi: &'a str) -> impl Fn(&csv::StringRecord, &csv::StringRecord) -> bool + 'a {
    move |h, r| h.iter().position(|c| c == col).and_then(|i| r.get(i))
        .is_some_and(|v| v == aoi || v.strip_prefix(aoi).is_some_and(|rest| rest.starts_with('_')))
}

#[derive(Deserialize)]
struct FloodReq {
    name: String,
    region: Region,
    /// rp10 | rp100 | rp500 | risk>=<t> | observed (AOI regions only)
    #[serde(default = "rp100")]
    floods: Vec<String>,
    #[serde(default)]
    allow_outside: bool,
}
fn rp100() -> Vec<String> { vec!["rp100".into()] }

fn flood(m: &Models, req: FloodReq) -> Result<Value> {
    check_name(&req.name)?;
    if !matches!(req.region, Region::Aoi { .. }) && req.floods.iter().any(|f| f == "observed") {
        bail!("flood 'observed' needs a CEMS AOI region (type aoi)");
    }
    for f in &req.floods {
        let ok = matches!(f.as_str(), "rp10" | "rp100" | "rp500" | "observed")
            || f.strip_prefix("risk>=").is_some_and(|t| t.parse::<f64>().is_ok_and(|t| (0.0..=1.0).contains(&t)));
        if !ok {
            bail!("flood {f:?}: use rp10, rp100, rp500, observed or risk>=<0..1>");
        }
    }
    let mut args = vec!["--region".into(), m.region_arg(&req.region, &req.name)?, "--name".into(), req.name.clone(), "--flood".into()];
    args.extend(req.floods.iter().cloned());
    if req.allow_outside {
        args.push("--allow-outside".into());
    }
    m.python("flood_tool.py", &args)?;
    let dir = m.out().join("regions").join(&req.name);
    let mut summary: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("summary.json"))?)?;
    // per-flood tables of THIS request (the folder may hold older runs under the same name):
    // danger_<label>.csv (factors), evacuation_<label>.csv (ranked cells); label as in
    // access_model.flood_label, e.g. risk>=0.5 → risk_ge_0.5
    let (mut danger, mut evac) = (Map::new(), Map::new());
    for f in &req.floods {
        let label = f.replace(">=", "_ge_");
        for (map, prefix) in [(&mut danger, "danger"), (&mut evac, "evacuation")] {
            let path = dir.join(format!("{prefix}_{label}.csv"));
            if path.exists() {
                map.insert(label.clone(), json!(path));
            }
        }
    }
    summary["files"] = json!({"dir": dir, "risk": dir.join("risk.tif"), "access_danger": dir.join("access_danger.gpkg"),
                              "danger": danger, "evacuation": evac, "summary": dir.join("summary.json")});
    Ok(summary)
}

/// rp10 | rp100 | rp500 | T=<years>, plus `observed` (AOI regions only) and `none` where allowed
fn flood_stage(f: &str, observed: bool, none: bool) -> Result<()> {
    let ok = matches!(f, "rp10" | "rp100" | "rp500")
        || (observed && f == "observed")
        || (none && f == "none")
        || f.strip_prefix("T=").is_some_and(|t| t.parse::<f64>().is_ok_and(|t| t.is_finite() && t >= 1.0));
    if !ok {
        bail!("flood stage {f:?}: use rp10, rp100, rp500, T=<years>{}{}", if observed { ", observed" } else { "" }, if none { ", none" } else { "" });
    }
    Ok(())
}

const WEATHER: [&str; 5] = ["dry", "rain", "heavy_rain", "fog", "snow"]; // keys of WEATHER in evac_sim.py

#[derive(Deserialize)]
struct EvacReq {
    name: String,
    region: Region,
    #[serde(default = "rp100_one")]
    flood: String,
    /// Flood stage closing roads while people leave (default: same as `flood`; `none` = all roads open)
    roads_flood: Option<String>,
    #[serde(default = "hours")]
    hours: Vec<u32>,
    #[serde(default)]
    weekend: bool,
    #[serde(default = "dry")]
    weather: Vec<String>,
}
fn rp100_one() -> String { "rp100".into() }
fn hours() -> Vec<u32> { vec![3, 8, 13, 17] }
fn dry() -> Vec<String> { vec!["dry".into()] }

fn evacuation(m: &Models, mut req: EvacReq) -> Result<Value> {
    check_name(&req.name)?;
    flood_stage(&req.flood, true, false)?;
    if let Some(r) = &req.roads_flood {
        flood_stage(r, true, true)?;
    }
    let is_aoi = matches!(req.region, Region::Aoi { .. });
    if !is_aoi && (req.flood == "observed" || req.roads_flood.as_deref() == Some("observed")) {
        bail!("flood 'observed' needs a CEMS AOI region (type aoi)");
    }
    // duplicate hours / weathers would run twice and collide on output names
    let mut seen = std::collections::HashSet::new();
    req.hours.retain(|h| seen.insert(*h));
    let mut seen = std::collections::HashSet::new();
    req.weather.retain(|w| seen.insert(w.clone()));
    if req.hours.is_empty() || req.hours.iter().any(|&h| h > 23) {
        bail!("hours must be 0..23");
    }
    if req.weather.is_empty() || req.weather.iter().any(|w| !WEATHER.contains(&w.as_str())) {
        bail!("weather must be from {WEATHER:?}");
    }
    let mut args = vec!["--region".into(), m.region_arg(&req.region, &req.name)?, "--name".into(), req.name.clone(),
                        "--flood".into(), req.flood.clone(), "--hours".into()];
    args.extend(req.hours.iter().map(|h| h.to_string()));
    args.push("--weather".into());
    args.extend(req.weather.iter().cloned());
    if let Some(r) = &req.roads_flood {
        args.extend(["--roads-flood".into(), r.clone()]);
    }
    if req.weekend {
        args.push("--weekend".into());
    }
    m.python("evac_sim.py", &args)?;
    let path = m.out().join("evacuations").join(&req.name).join("summary.json");
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Driver {
    /// What-if hydrograph: rises to a 1-in-`peak_rp`-year flood, then recedes
    Whatif { peak_rp: f64, #[serde(default = "h24")] rise_h: f64, #[serde(default = "h72")] fall_h: f64, #[serde(default = "h6")] step_h: f64 },
    /// GloFAS discharge from Open-Meteo at a river point (default: region centre)
    OpenMeteo { point: Option<[f64; 2]>, start: String, end: String },
    /// Own series of return periods
    Series { steps: Vec<SeriesStep> },
}
#[derive(Deserialize)]
struct SeriesStep {
    time: String,
    return_period: f64,
}
fn h24() -> f64 { 24.0 }
fn h72() -> f64 { 72.0 }
fn h6() -> f64 { 6.0 }

#[derive(Deserialize)]
struct TimelineReq {
    name: String,
    region: Region,
    driver: Driver,
}

fn timeline(m: &Models, req: TimelineReq) -> Result<Value> {
    check_name(&req.name)?;
    let mut args = vec!["--region".into(), m.region_arg(&req.region, &req.name)?, "--name".into(), req.name.clone()];
    let date = |d: &str| d.len() == 10 && d.chars().all(|c| c.is_ascii_digit() || c == '-');
    match &req.driver {
        Driver::Whatif { peak_rp, rise_h, fall_h, step_h } => {
            if *peak_rp < 2.0 || *step_h <= 0.0 {
                bail!("peak_rp must be >= 2 and step_h > 0");
            }
            args.extend(["--peak-rp", &peak_rp.to_string(), "--rise-h", &rise_h.to_string(),
                         "--fall-h", &fall_h.to_string(), "--step-h", &step_h.to_string()].map(String::from));
        }
        Driver::OpenMeteo { point, start, end } => {
            if !date(start) || !date(end) {
                bail!("start and end must be YYYY-MM-DD");
            }
            args.push("--open-meteo".into());
            if let Some([lon, lat]) = point {
                args.push(format!("{lon},{lat}"));
            }
            args.extend(["--start".into(), start.clone(), "--end".into(), end.clone()]);
        }
        Driver::Series { steps } => {
            let dir = m.out().join("api_regions");
            std::fs::create_dir_all(&dir)?;
            let path = dir.join(format!("{}_series.csv", req.name));
            let mut wr = csv::Writer::from_path(&path)?;
            wr.write_record(["time", "return_period"])?;
            for s in steps {
                wr.write_record([s.time.as_str(), &s.return_period.to_string()])?;
            }
            wr.flush()?;
            args.extend(["--series".into(), path.to_string_lossy().into()]);
        }
    }
    m.python("timeline.py", &args)?;
    let dir = m.out().join("timelines").join(&req.name);
    Ok(json!({"name": req.name, "steps": csv_rows(&dir.join("steps.csv"), |_, _| true)?,
              "files": {"dir": dir, "viewer": dir.join("viewer.html"), "cells": dir.join("cells.gpkg")}}))
}

#[derive(Deserialize)]
struct ImpactReq {
    /// CEMS AOI, e.g. EMSR517_AOI15_GRA: the observed flood extent is the hazard
    aoi: String,
    #[serde(default = "cost")]
    cost_eur_m2: f64,
    #[serde(default = "max_depth")]
    max_depth_m: f64,
}
fn cost() -> f64 { 1500.0 }
fn max_depth() -> f64 { 6.0 }

fn impact(m: &Models, req: ImpactReq) -> Result<Value> {
    if !(req.cost_eur_m2.is_finite() && req.cost_eur_m2 > 0.0) || !(req.max_depth_m.is_finite() && req.max_depth_m > 0.0) {
        bail!("cost_eur_m2 and max_depth_m must be positive numbers");
    }
    let folder = m.cems_folder(&req.aoi)?;
    let dir = m.out().join("impact");
    std::fs::create_dir_all(&dir)?;
    let json_path = dir.join(format!("{}.json", req.aoi));
    let log = m.python("pipeline.py", &[folder.to_string_lossy().into(), "--cost".into(), req.cost_eur_m2.to_string(),
                                       "--max-depth".into(), req.max_depth_m.to_string(), "--json".into(), json_path.to_string_lossy().into()])?;
    if log.contains("No building data available") {
        bail!("no GHS-OBAT building data for {}", req.aoi);
    }
    let summaries: Value = serde_json::from_str(&std::fs::read_to_string(&json_path)?)?;
    Ok(json!({"aoi": req.aoi, "floods": summaries,
              "caveat": "Placeholder replacement cost; JRC Europe depth-damage curves; GHS-OBAT use codes assumed (1 residential, 2 non-residential)."}))
}

fn models_list(m: &Models) -> Result<Value> {
    let eval = csv_rows(&m.out().join("eval/summary.csv"), |_, _| true).unwrap_or_default();
    Ok(json!({
        "models": [
            {"name": "risk", "script": "risk_model.py", "what": "relative flood susceptibility per 100 m cell from terrain + EFAS",
             "endpoints": ["POST /region", "POST /flood"], "runs_on": "any region in the study area"},
            {"name": "access", "script": "access_model.py (via flood_tool.py)", "what": "cut-off buildings, danger factors, evacuation levels per 1 km cell",
             "endpoints": ["POST /flood"], "runs_on": "any region in the study area"},
            {"name": "timeline", "script": "timeline.py", "what": "flood development in time: depth, closed roads, people to evacuate per step",
             "endpoints": ["POST /timeline"], "runs_on": "any region in the study area"},
            {"name": "evacuation", "script": "evac_sim.py", "what": "human evacuation flow: evacuees, cars, congested roads, time to safety per hour/weather",
             "endpoints": ["POST /evacuation"], "runs_on": "any region in the study area"},
            {"name": "impact", "script": "pipeline.py", "what": "building damage (€) and displaced people from an observed flood",
             "endpoints": ["POST /impact", "GET /impact"], "runs_on": "CEMS AOIs with GHS-OBAT buildings"},
            {"name": "roads", "script": "road_model.py", "what": "road km impassable per scenario, road damage model, closure timeline",
             "endpoints": ["GET /roads/{aoi}"], "runs_on": "precomputed for the CEMS AOIs"},
            {"name": "interp", "script": "interp_model.py", "what": "flood extent between two CEMS observations",
             "endpoints": ["GET /interp/{aoi}"], "runs_on": "precomputed for CEMS AOIs with PRODUCT + MONIT01"},
            {"name": "help_a_friend", "script": "flood_api/src/help.rs", "what": "match helpers to people with a disability, flood-aware routes",
             "endpoints": ["POST /help/zones", "POST /helpers", "POST /help-requests"], "runs_on": "any zone covered by the OSM extracts"},
        ],
        "evaluation": eval,
    }))
}

fn roads(m: &Models, aoi: &str) -> Result<Value> {
    check_aoi(aoi)?;
    let dir = m.out().join("roads");
    let exposure = csv_rows(&dir.join("exposure_by_area.csv"), by_aoi("area", aoi))?;
    if exposure.is_empty() {
        return Err(NotFound(format!("no road results for {aoi}")).into());
    }
    Ok(json!({
        "aoi": aoi, "exposure_km": exposure,
        "damage_validation": csv_rows(&dir.join("damage_validation.csv"), by_aoi("area", aoi))?,
        "closure_timeline": csv_rows(&dir.join("closure_timeline.csv"), by_aoi("pair", aoi))?,
        "files": {"roads_at_risk": dir.join("roads_at_risk.gpkg")}, "caveat": ROAD_DAMAGE_NOTE,
    }))
}

fn interp(m: &Models, aoi: &str) -> Result<Value> {
    check_aoi(aoi)?;
    let dir = m.out().join("interp");
    let rows = csv_rows(&dir.join("timeseries.csv"), by_aoi("pair", aoi))?;
    if rows.is_empty() {
        return Err(NotFound(format!("no interpolated extents for {aoi} (needs PRODUCT + MONIT01 that overlap)")).into());
    }
    Ok(json!({"aoi": aoi, "timeseries": rows, "files": {"extents": dir.join("extents.gpkg")}}))
}

fn impact_all(m: &Models) -> Result<Value> {
    Ok(json!({"aois": csv_rows(&m.out().join("report/flood_summary.csv"), |_, _| true)?,
              "files": {"pdf": m.out().join("report/flood_summary.pdf")}}))
}

/// Read-only download of a result file (or a folder listing) under code/outputs
async fn files(State(m): State<Models>, PathArg(rel): PathArg<String>) -> Response {
    let not_found = || (StatusCode::NOT_FOUND, Json(json!({"error": "no such file under outputs"}))).into_response();
    let Ok(root) = std::fs::canonicalize(m.out()) else { return not_found() };
    // canonicalize resolves .. and symlinks, so anything outside the root is rejected here
    let Ok(path) = std::fs::canonicalize(root.join(&rel)) else { return not_found() };
    if !path.starts_with(&root) {
        return not_found();
    }
    if path.is_dir() {
        let mut names: Vec<String> = std::fs::read_dir(&path).into_iter().flatten().flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned() + if e.path().is_dir() { "/" } else { "" }).collect();
        names.sort();
        let base = format!("/files/{}", path.strip_prefix(&root).unwrap().to_string_lossy());
        let entries: Vec<Value> = names.iter().map(|n| json!({"name": n, "url": format!("{}/{n}", base.trim_end_matches('/'))})).collect();
        return Json(json!({"folder": base, "entries": entries})).into_response();
    }
    let ctype = match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "json" => "application/json",
        "geojson" => "application/geo+json",
        "csv" => "text/csv; charset=utf-8",
        "tif" | "tiff" => "image/tiff",
        "png" => "image/png",
        "pdf" => "application/pdf",
        "gpkg" => "application/geopackage+sqlite3",
        _ => "application/octet-stream",
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, ctype)], Body::from(bytes)).into_response(),
        Err(_) => not_found(),
    }
}

pub fn router(models: Models) -> Router {
    Router::new()
        .route("/models", get(|State(m): State<Models>| blocking(move || models_list(&m).map(|v| m.public(v)))))
        .route("/flood", post(|State(m): State<Models>, JsonBody(r): JsonBody<FloodReq>| blocking(move || flood(&m, r).map(|v| m.public(v)))))
        .route("/evacuation", post(|State(m): State<Models>, JsonBody(r): JsonBody<EvacReq>| blocking(move || evacuation(&m, r).map(|v| m.public(v)))))
        .route("/timeline", post(|State(m): State<Models>, JsonBody(r): JsonBody<TimelineReq>| blocking(move || timeline(&m, r).map(|v| m.public(v)))))
        .route("/impact", get(|State(m): State<Models>| blocking(move || impact_all(&m).map(|v| m.public(v))))
            .post(|State(m): State<Models>, JsonBody(r): JsonBody<ImpactReq>| blocking(move || impact(&m, r).map(|v| m.public(v)))))
        .route("/roads/:aoi", get(|State(m): State<Models>, PathArg(a): PathArg<String>| blocking(move || roads(&m, &a).map(|v| m.public(v)))))
        .route("/interp/:aoi", get(|State(m): State<Models>, PathArg(a): PathArg<String>| blocking(move || interp(&m, &a).map(|v| m.public(v)))))
        .route("/files/*path", get(files))
        .with_state(models)
}
