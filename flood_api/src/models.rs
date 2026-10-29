//! Endpoints for the Python models in code/. Region models (flood_tool, timeline, pipeline) run
//! on request as subprocesses, like the scorer in /region; models trained on the CEMS AOIs
//! (road_model, interp_model, evaluate, report) serve their stored outputs.
use anyhow::{bail, Context, Result};
use crate::blocking;
use axum::{extract::{Path, State}, routing::{get, post}, Json, Router};
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

fn check_name(name: &str) -> Result<()> {
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

    fn region_arg(&self, r: &Region, name: &str) -> Result<String> {
        Ok(match r {
            Region::Bbox { bbox: [w, s, e, n] } => format!("{w},{s},{e},{n}"),
            Region::Aoi { aoi } => {
                check_name(aoi)?;
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

    /// Run a script in code/; on failure return its last error lines (the scripts exit with a message)
    fn python(&self, script: &str, args: &[String]) -> Result<String> {
        let out = Command::new(&self.python).arg(self.code.join(script)).args(args).current_dir(&self.code)
            .output().with_context(|| format!("run {script}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let tail: Vec<&str> = err.lines().rev().take(5).collect();
            bail!("{script} failed: {}", tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into())
    }

    /// CEMS PRODUCT vector folder of an AOI, e.g. EMSR517_AOI15_GRA
    fn cems_folder(&self, aoi: &str) -> Result<PathBuf> {
        check_name(aoi)?;
        for d in std::fs::read_dir(&self.data)?.flatten().filter(|d| d.file_name().to_string_lossy().starts_with("ems_")) {
            for f in std::fs::read_dir(d.path())?.flatten() {
                let n = f.file_name().to_string_lossy().into_owned();
                if n.starts_with(&format!("{aoi}_PRODUCT_")) && n.ends_with("_vector") {
                    return Ok(f.path());
                }
            }
        }
        bail!("no CEMS PRODUCT folder for {aoi}")
    }
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

/// Rows whose `col` starts with the AOI name (e.g. EMSR517_AOI15)
fn by_aoi<'a>(col: &'a str, aoi: &'a str) -> impl Fn(&csv::StringRecord, &csv::StringRecord) -> bool + 'a {
    move |h, r| h.iter().position(|c| c == col).and_then(|i| r.get(i)).is_some_and(|v| v.starts_with(aoi))
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
    summary["files"] = json!({"dir": dir, "risk": dir.join("risk.tif"), "access_danger": dir.join("access_danger.gpkg")});
    Ok(summary)
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
    check_name(aoi)?;
    let dir = m.out().join("roads");
    let exposure = csv_rows(&dir.join("exposure_by_area.csv"), by_aoi("area", aoi))?;
    if exposure.is_empty() {
        bail!("no road results for {aoi}; AOI names look like EMSR517_AOI15");
    }
    Ok(json!({
        "aoi": aoi, "exposure_km": exposure,
        "damage_validation": csv_rows(&dir.join("damage_validation.csv"), by_aoi("area", aoi))?,
        "closure_timeline": csv_rows(&dir.join("closure_timeline.csv"), by_aoi("pair", aoi))?,
        "files": {"roads_at_risk": dir.join("roads_at_risk.gpkg")}, "caveat": ROAD_DAMAGE_NOTE,
    }))
}

fn interp(m: &Models, aoi: &str) -> Result<Value> {
    check_name(aoi)?;
    let dir = m.out().join("interp");
    let rows = csv_rows(&dir.join("timeseries.csv"), by_aoi("pair", aoi))?;
    if rows.is_empty() {
        bail!("no interpolated extents for {aoi} (needs PRODUCT + MONIT01 that overlap)");
    }
    Ok(json!({"aoi": aoi, "timeseries": rows, "files": {"extents": dir.join("extents.gpkg")}}))
}

fn impact_all(m: &Models) -> Result<Value> {
    Ok(json!({"aois": csv_rows(&m.out().join("report/flood_summary.csv"), |_, _| true)?,
              "files": {"pdf": m.out().join("report/flood_summary.pdf")}}))
}

pub fn router(models: Models) -> Router {
    Router::new()
        .route("/models", get(|State(m): State<Models>| blocking(move || models_list(&m))))
        .route("/flood", post(|State(m): State<Models>, Json(r): Json<FloodReq>| blocking(move || flood(&m, r))))
        .route("/timeline", post(|State(m): State<Models>, Json(r): Json<TimelineReq>| blocking(move || timeline(&m, r))))
        .route("/impact", get(|State(m): State<Models>| blocking(move || impact_all(&m)))
            .post(|State(m): State<Models>, Json(r): Json<ImpactReq>| blocking(move || impact(&m, r))))
        .route("/roads/:aoi", get(|State(m): State<Models>, Path(a): Path<String>| blocking(move || roads(&m, &a))))
        .route("/interp/:aoi", get(|State(m): State<Models>, Path(a): Path<String>| blocking(move || interp(&m, &a))))
        .with_state(models)
}
