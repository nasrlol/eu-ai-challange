//! "Help a friend": a person with a disability in a flood risk zone asks for help, the closest
//! volunteer helpers (by flood-aware travel time) get an offer, the first to accept gets a route.
//! Data minimisation: no names, only a broad need category; requests and helper positions are
//! deleted after RETENTION_H. If the person reports water already at their location, no
//! volunteer is sent: the request is escalated to 112.
use crate::roads::{Graph, Mode};
use crate::types::Scenario;
use anyhow::{bail, Context, Result};
use crate::blocking;
use axum::{extract::{Path, Query, State}, routing::{get, patch, post}, Json, Router};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::HashMap, path::PathBuf, sync::{Arc, Mutex, RwLock}, time::{SystemTime, UNIX_EPOCH}};

/// Helpers offered each request
const OFFERS: usize = 3;
const RETENTION_H: i64 = 72;
/// Same limit as /region
const MAX_ZONE_KM2: f64 = 3000.0;
const NEEDS: [&str; 4] = ["mobility", "visual", "hearing", "other"];

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS zones (
    name TEXT PRIMARY KEY, w REAL NOT NULL, s REAL NOT NULL, e REAL NOT NULL, n REAL NOT NULL);
CREATE TABLE IF NOT EXISTS helpers (
    id INTEGER PRIMARY KEY, zone TEXT NOT NULL REFERENCES zones(name),
    lon REAL NOT NULL, lat REAL NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('walk', 'bike', 'car')),
    available INTEGER NOT NULL DEFAULT 1, updated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS help_requests (
    id INTEGER PRIMARY KEY, zone TEXT NOT NULL REFERENCES zones(name),
    lon REAL NOT NULL, lat REAL NOT NULL,
    need TEXT NOT NULL CHECK (need IN ('mobility', 'visual', 'hearing', 'other')),
    urgent INTEGER NOT NULL DEFAULT 0,
    scenario TEXT NOT NULL, depth_m REAL NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('open', 'assigned', 'done', 'escalated', 'unmatched')),
    created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS assignments (
    request_id INTEGER NOT NULL REFERENCES help_requests(id) ON DELETE CASCADE,
    helper_id INTEGER NOT NULL REFERENCES helpers(id) ON DELETE CASCADE,
    rank INTEGER NOT NULL, eta_s REAL NOT NULL, distance_m REAL NOT NULL,
    offered_at INTEGER NOT NULL, accepted_at INTEGER, route TEXT,
    PRIMARY KEY (request_id, helper_id));
";

#[derive(Clone)]
pub struct Help {
    db: Arc<Mutex<Connection>>,
    graphs: Arc<RwLock<HashMap<String, Arc<Graph>>>>,
    data: PathBuf,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn purge(db: &Connection) -> Result<()> {
    let cutoff = now() - RETENTION_H * 3600;
    db.execute("DELETE FROM help_requests WHERE created_at < ?1", [cutoff])?;
    db.execute("DELETE FROM helpers WHERE updated_at < ?1", [cutoff])?;
    Ok(())
}

impl Help {
    pub fn open(db_path: &std::path::Path, data: PathBuf) -> Result<Help> {
        let db = Connection::open(db_path).with_context(|| format!("open {}", db_path.display()))?;
        db.execute_batch("PRAGMA foreign_keys = ON;")?;
        db.execute_batch(SCHEMA)?;
        purge(&db)?;
        Ok(Help { db: Arc::new(Mutex::new(db)), graphs: Default::default(), data })
    }

    /// Road graph of a zone, rebuilt from its stored bbox after a restart
    fn graph(&self, zone: &str) -> Result<Arc<Graph>> {
        if let Some(g) = self.graphs.read().unwrap().get(zone) {
            return Ok(g.clone());
        }
        let bbox: [f64; 4] = self.db.lock().unwrap()
            .query_row("SELECT w, s, e, n FROM zones WHERE name = ?1", [zone], |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?]))
            .optional()?.with_context(|| format!("unknown zone {zone}, create it with POST /help/zones"))?;
        let g = Arc::new(Graph::build(bbox, &self.data)?);
        self.graphs.write().unwrap().insert(zone.into(), g.clone());
        Ok(g)
    }
}

#[derive(Deserialize)]
struct ZoneReq {
    name: String,
    /// [west, south, east, north] in degrees
    bbox: [f64; 4],
}

fn create_zone(h: &Help, req: ZoneReq) -> Result<Value> {
    let [w, s, e, n] = req.bbox;
    let km2 = (e - w) * 111.32 * ((s + n) / 2.0).to_radians().cos() * (n - s) * 111.32;
    if !(w < e && s < n) || km2 > MAX_ZONE_KM2 {
        bail!("bbox must be [w, s, e, n] and at most {MAX_ZONE_KM2} km² (got {km2:.0})");
    }
    let g = Arc::new(Graph::build(req.bbox, &h.data)?);
    h.db.lock().unwrap().execute("INSERT OR REPLACE INTO zones VALUES (?1, ?2, ?3, ?4, ?5)", params![req.name, w, s, e, n])?;
    let summary = g.summary();
    h.graphs.write().unwrap().insert(req.name.clone(), g);
    Ok(json!({"zone": req.name, "area_km2": km2.round(), "roads": summary}))
}

#[derive(Deserialize)]
struct HelperReq {
    zone: String,
    lon: f64,
    lat: f64,
    mode: Mode,
}

fn create_helper(h: &Help, req: HelperReq) -> Result<Value> {
    let db = h.db.lock().unwrap();
    db.execute("INSERT INTO helpers (zone, lon, lat, mode, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
               params![req.zone, req.lon, req.lat, req.mode.as_str(), now()]).context("unknown zone?")?;
    Ok(json!({"id": db.last_insert_rowid()}))
}

#[derive(Deserialize)]
struct HelperUpdate {
    lon: Option<f64>,
    lat: Option<f64>,
    available: Option<bool>,
}

fn update_helper(h: &Help, id: i64, req: HelperUpdate) -> Result<Value> {
    let n = h.db.lock().unwrap().execute(
        "UPDATE helpers SET lon = COALESCE(?2, lon), lat = COALESCE(?3, lat), available = COALESCE(?4, available),
         updated_at = ?5 WHERE id = ?1", params![id, req.lon, req.lat, req.available, now()])?;
    if n == 0 {
        bail!("no helper {id}");
    }
    Ok(json!({"id": id}))
}

#[derive(Deserialize)]
struct HelpReq {
    zone: String,
    lon: f64,
    lat: f64,
    need: String,
    #[serde(default)]
    urgent: bool,
    /// Flood scenario used for the risk level and to keep routes off flooded roads
    #[serde(default = "rp100")]
    scenario: Scenario,
    /// Water already at the person's location: escalate to 112, no volunteer
    #[serde(default)]
    water_now: bool,
}
fn rp100() -> Scenario { Scenario::Rp100 }

fn sc_str(sc: Scenario) -> String {
    serde_json::to_value(sc).unwrap().as_str().unwrap().into()
}

fn create_request(h: &Help, req: HelpReq) -> Result<Value> {
    if !NEEDS.contains(&req.need.as_str()) {
        bail!("need must be one of {NEEDS:?}");
    }
    let g = h.graph(&req.zone)?;
    let depth = g.depth_at(req.lon, req.lat, req.scenario);
    let risk = json!({"scenario": req.scenario, "efas_depth_m": depth, "in_risk_zone": depth > 0.0});
    let status = if req.water_now { "escalated" } else { "open" };
    let id = {
        let db = h.db.lock().unwrap();
        purge(&db)?;
        db.execute("INSERT INTO help_requests (zone, lon, lat, need, urgent, scenario, depth_m, status, created_at)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                   params![req.zone, req.lon, req.lat, req.need, req.urgent, sc_str(req.scenario), depth, status, now()])?;
        db.last_insert_rowid()
    };
    if req.water_now {
        return Ok(json!({"id": id, "status": status, "risk": risk,
            "message": "Water is already at your location: call 112 now. No volunteer is sent into flood water."}));
    }

    // Available helpers in the zone, routed per travel mode
    let helpers: Vec<(i64, f64, f64, String)> = {
        let db = h.db.lock().unwrap();
        let mut st = db.prepare("SELECT id, lon, lat, mode FROM helpers WHERE zone = ?1 AND available = 1")?;
        let rows = st.query_map([&req.zone], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut found = vec![];
    for mode in Mode::ALL {
        let hs: Vec<_> = helpers.iter().filter(|x| x.3 == mode.as_str()).collect();
        let origins: Vec<_> = hs.iter().map(|x| (x.1, x.2)).collect();
        for (x, r) in hs.iter().zip(g.routes(&origins, (req.lon, req.lat), mode, req.scenario)) {
            if let Some(r) = r {
                found.push((x.0, mode, r));
            }
        }
    }
    found.sort_by(|a, b| a.2.eta_s.total_cmp(&b.2.eta_s));
    found.truncate(OFFERS);

    let db = h.db.lock().unwrap();
    for (rank, (hid, _, r)) in found.iter().enumerate() {
        db.execute("INSERT INTO assignments (request_id, helper_id, rank, eta_s, distance_m, offered_at)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![id, hid, rank as i64 + 1, r.eta_s, r.distance_m, now()])?;
    }
    let status = if found.is_empty() { "unmatched" } else { "open" };
    db.execute("UPDATE help_requests SET status = ?2 WHERE id = ?1", params![id, status])?;
    let offers: Vec<Value> = found.iter().map(|(hid, mode, r)| json!({"helper_id": hid, "mode": mode,
        "eta_s": r.eta_s.round(), "distance_m": r.distance_m.round(), "flooded_m": r.flooded_m.round()})).collect();
    let mut out = json!({"id": id, "status": status, "risk": risk, "offers": offers});
    if found.is_empty() {
        out["message"] = json!("No helper can reach you. Call 112.");
    }
    Ok(out)
}

#[derive(Deserialize)]
struct AcceptReq {
    helper_id: i64,
}

fn accept(h: &Help, id: i64, req: AcceptReq) -> Result<Value> {
    let (zone, lon, lat, scenario, status): (String, f64, f64, String, String) = h.db.lock().unwrap()
        .query_row("SELECT zone, lon, lat, scenario, status FROM help_requests WHERE id = ?1", [id],
                   |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .optional()?.with_context(|| format!("no request {id}"))?;
    if status != "open" {
        bail!("request {id} is {status}");
    }
    let (hlon, hlat, mode): (f64, f64, String) = h.db.lock().unwrap()
        .query_row("SELECT h.lon, h.lat, h.mode FROM assignments a JOIN helpers h ON h.id = a.helper_id
                    WHERE a.request_id = ?1 AND a.helper_id = ?2", [id, req.helper_id],
                   |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?.with_context(|| format!("helper {} was not offered request {id}", req.helper_id))?;
    let sc: Scenario = serde_json::from_value(json!(scenario))?;
    let g = h.graph(&zone)?;
    let r = g.routes(&[(hlon, hlat)], (lon, lat), Mode::parse(&mode).unwrap(), sc).pop().flatten()
        .context("route no longer found")?;
    let route = g.describe(&r, sc, "Arrive at the person who asked for help");

    let mut db = h.db.lock().unwrap();
    let tx = db.transaction()?;
    // the status check guards against two helpers accepting at the same time
    if tx.execute("UPDATE help_requests SET status = 'assigned' WHERE id = ?1 AND status = 'open'", [id])? == 0 {
        bail!("request {id} was just taken by another helper");
    }
    tx.execute("UPDATE assignments SET accepted_at = ?3, route = ?4, eta_s = ?5 WHERE request_id = ?1 AND helper_id = ?2",
               params![id, req.helper_id, now(), route.to_string(), r.eta_s])?;
    tx.execute("UPDATE helpers SET available = 0 WHERE id = ?1", [req.helper_id])?;
    tx.commit()?;
    Ok(json!({"id": id, "status": "assigned", "helper_id": req.helper_id, "route": route}))
}

fn get_request(h: &Help, id: i64) -> Result<Value> {
    let db = h.db.lock().unwrap();
    let mut out = db.query_row("SELECT status, need, urgent, scenario, depth_m, created_at FROM help_requests WHERE id = ?1", [id],
        |r| Ok(json!({"id": id, "status": r.get::<_, String>(0)?, "need": r.get::<_, String>(1)?, "urgent": r.get::<_, bool>(2)?,
                      "risk": {"scenario": r.get::<_, String>(3)?, "efas_depth_m": r.get::<_, f64>(4)?}, "created_at": r.get::<_, i64>(5)?})))
        .optional()?.with_context(|| format!("no request {id}"))?;
    let mut st = db.prepare("SELECT helper_id, rank, eta_s, distance_m, accepted_at, route FROM assignments WHERE request_id = ?1 ORDER BY rank")?;
    let offers: Vec<Value> = st.query_map([id], |r| {
        let route: Option<String> = r.get(5)?;
        Ok(json!({"helper_id": r.get::<_, i64>(0)?, "rank": r.get::<_, i64>(1)?, "eta_s": r.get::<_, f64>(2)?.round(),
                  "distance_m": r.get::<_, f64>(3)?.round(), "accepted_at": r.get::<_, Option<i64>>(4)?,
                  "route": route.and_then(|s| serde_json::from_str::<Value>(&s).ok())}))
    })?.collect::<rusqlite::Result<_>>()?;
    out["offers"] = json!(offers);
    Ok(out)
}

#[derive(Deserialize)]
struct ShelterQuery {
    #[serde(default = "rp100")]
    scenario: Scenario,
    /// Travel mode to the shelter (default: the accepted helper's, else walking)
    mode: Option<Mode>,
    /// Also consider schools and sports halls (default true)
    candidates: Option<bool>,
}

fn shelter_json(g: &Graph, i: usize, sc: Scenario) -> Value {
    let f = &g.shelters[i];
    let depth = g.depth_at(f.lon, f.lat, sc);
    json!({"kind": f.kind, "name": f.name, "lon": f.lon, "lat": f.lat, "efas_depth_m": depth, "dry": depth == 0.0})
}

fn zone_shelters(h: &Help, zone: &str, q: ShelterQuery) -> Result<Value> {
    let g = h.graph(zone)?;
    let list: Vec<Value> = (0..g.shelters.len()).map(|i| shelter_json(&g, i, q.scenario)).collect();
    let dry = list.iter().filter(|s| s["dry"] == true).count();
    Ok(json!({"zone": zone, "scenario": q.scenario, "shelters": list.len(), "dry": dry, "list": list,
              "caveat": "From OpenStreetMap; candidate_shelter = schools and sports halls, not confirmed as designated shelters; capacity unknown."}))
}

/// Nearest shelter that stays dry in the scenario, by flood-aware route from the person
fn nearest_shelter(h: &Help, id: i64, q: ShelterQuery) -> Result<Value> {
    let (zone, lon, lat, helper_mode): (String, f64, f64, Option<String>) = h.db.lock().unwrap()
        .query_row("SELECT r.zone, r.lon, r.lat, (SELECT h.mode FROM assignments a JOIN helpers h ON h.id = a.helper_id
                    WHERE a.request_id = r.id AND a.accepted_at IS NOT NULL) FROM help_requests r WHERE r.id = ?1", [id],
                   |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .optional()?.with_context(|| format!("no request {id}"))?;
    let mode = q.mode.or(helper_mode.as_deref().and_then(Mode::parse)).unwrap_or(Mode::Walk);
    let g = h.graph(&zone)?;
    let ok: Vec<usize> = (0..g.shelters.len())
        .filter(|&i| q.candidates.unwrap_or(true) || g.shelters[i].kind == "shelter")
        .filter(|&i| g.depth_at(g.shelters[i].lon, g.shelters[i].lat, q.scenario) == 0.0)
        .collect();
    let pts: Vec<_> = ok.iter().map(|&i| (g.shelters[i].lon, g.shelters[i].lat)).collect();
    let (k, r) = g.nearest((lon, lat), &pts, mode, q.scenario).context("no dry shelter reachable; call 112")?;
    Ok(json!({"id": id, "mode": mode, "shelter": shelter_json(&g, ok[k], q.scenario),
              "route": g.describe(&r, q.scenario, "Arrive at the shelter")}))
}

fn close(h: &Help, id: i64) -> Result<Value> {
    let mut db = h.db.lock().unwrap();
    let tx = db.transaction()?;
    if tx.execute("UPDATE help_requests SET status = 'done' WHERE id = ?1 AND status IN ('open', 'assigned')", [id])? == 0 {
        bail!("no open or assigned request {id}");
    }
    tx.execute("UPDATE helpers SET available = 1, updated_at = ?2
                WHERE id IN (SELECT helper_id FROM assignments WHERE request_id = ?1 AND accepted_at IS NOT NULL)", params![id, now()])?;
    tx.commit()?;
    Ok(json!({"id": id, "status": "done"}))
}

pub fn router(help: Help) -> Router {
    Router::new()
        .route("/help/zones", post(|State(h): State<Help>, Json(r): Json<ZoneReq>| blocking(move || create_zone(&h, r))))
        .route("/helpers", post(|State(h): State<Help>, Json(r): Json<HelperReq>| blocking(move || create_helper(&h, r))))
        .route("/helpers/:id", patch(|State(h): State<Help>, Path(id): Path<i64>, Json(r): Json<HelperUpdate>|
            blocking(move || update_helper(&h, id, r))))
        .route("/help-requests", post(|State(h): State<Help>, Json(r): Json<HelpReq>| blocking(move || create_request(&h, r))))
        .route("/help-requests/:id", get(|State(h): State<Help>, Path(id): Path<i64>| blocking(move || get_request(&h, id))))
        .route("/help-requests/:id/accept", post(|State(h): State<Help>, Path(id): Path<i64>, Json(r): Json<AcceptReq>|
            blocking(move || accept(&h, id, r))))
        .route("/help/zones/:zone/shelters", get(|State(h): State<Help>, Path(z): Path<String>, Query(q): Query<ShelterQuery>|
            blocking(move || zone_shelters(&h, &z, q))))
        .route("/help-requests/:id/shelter", get(|State(h): State<Help>, Path(id): Path<i64>, Query(q): Query<ShelterQuery>|
            blocking(move || nearest_shelter(&h, id, q))))
        .route("/help-requests/:id/close", post(|State(h): State<Help>, Path(id): Path<i64>| blocking(move || close(&h, id))))
        .with_state(help)
}
