//! Flood-aware routing for "Help a friend": a road graph from the OSM extracts, each edge tagged
//! with the EFAS depth at its midpoint, and Dijkstra towards the person who asked for help.
//! Edges flooded in the chosen scenario are not removed but get a fixed extra cost per metre (the
//! same for every mode, so a slow walker is not punished more than a car for the same stretch):
//! routes avoid them and only use them when there is no other way in (reported as `flooded_m`).
//! Helpers are ranked by that cost; `eta_s` is the real travel time.
use crate::osm::{read_roads, Facility};
use crate::raster::read_window;
use crate::types::Scenario;
use anyhow::{bail, Context, Result};
use rstar::{primitives::GeomWithData, RTree};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{cmp::Reverse, collections::{BinaryHeap, HashMap}, path::Path};

/// Water deeper than this on a road counts as flooded (assumption: ~30 cm can float a car)
pub const FLOODED_M: f32 = 0.3;
/// Extra cost per metre of flooded road, s/m (assumption: worth a ~5 km car detour to avoid 100 m)
const FLOOD_PENALTY_S_PER_M: f64 = 5.0;
/// A point further than this from a usable road is refused
pub const MAX_SNAP_M: f64 = 500.0;
const WALK_MS: f64 = 5.0 / 3.6;
const M_PER_DEG: f64 = 111_320.0;

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Walk,
    Bike,
    Car,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Walk, Mode::Bike, Mode::Car];

    pub fn as_str(self) -> &'static str {
        match self { Mode::Walk => "walk", Mode::Bike => "bike", Mode::Car => "car" }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.as_str() == s)
    }

    /// km/h on this kind of road, None if not allowed. Car speeds are rough urban/rural averages.
    fn speed(self, highway: &str) -> Option<f64> {
        let motor = highway.starts_with("motorway") || highway.starts_with("trunk");
        let no_cars = matches!(highway, "path" | "footway" | "cycleway" | "pedestrian" | "steps" | "bridleway" | "track");
        match self {
            Mode::Walk => (!motor).then_some(5.0),
            Mode::Bike => (!motor && highway != "steps").then_some(15.0),
            Mode::Car if no_cars => None,
            Mode::Car => Some(match highway {
                "motorway" => 110.0,
                "trunk" => 90.0,
                "primary" => 70.0,
                "secondary" => 60.0,
                "tertiary" => 50.0,
                h if h.ends_with("_link") => 40.0,
                "unclassified" | "road" => 40.0,
                "residential" => 30.0,
                "service" => 15.0,
                _ => 10.0, // living_street
            }),
        }
    }
}

struct Edge {
    from: u32,
    to: u32,
    len_m: f32,
    road: u32,
    /// EFAS depth at the midpoint for rp10, rp100, rp500 (0 if dry)
    depth: [f32; 3],
    /// Against a oneway: only walking is allowed
    against_oneway: bool,
}

pub struct Graph {
    nodes: Vec<(f64, f64)>,
    edges: Vec<Edge>,
    /// Incoming / outgoing edges per node, for Dijkstra towards a target / from an origin
    incoming: Vec<Vec<u32>>,
    outgoing: Vec<Vec<u32>>,
    roads: Vec<(String, Option<String>)>,
    /// Node index by equirectangular metres
    index: RTree<GeomWithData<[f64; 2], u32>>,
    lat0: f64,
    efas: [crate::raster::Grid; 3],
    pub shelters: Vec<Facility>,
    /// [west, south, east, north] the graph was built for
    pub bbox: [f64; 4],
}

fn scenario_ix(s: Scenario) -> usize {
    match s { Scenario::Rp10 => 0, Scenario::Rp100 => 1, Scenario::Rp500 => 2 }
}

pub struct Route {
    /// Real travel time, s (snapping distances walked)
    pub eta_s: f64,
    /// eta_s + flood penalty: what helpers are ranked by
    pub cost_s: f64,
    pub distance_m: f64,
    pub flooded_m: f64,
    edges: Vec<u32>,
}

impl Graph {
    pub fn build(bbox: [f64; 4], data: &Path) -> Result<Graph> {
        let [w, s, e, n] = bbox;
        let (roads, shelters) = read_roads(&data.join("osm"), w, s, e, n)?.context("no OSM extract covers this zone")?;
        let efas: Vec<_> = Scenario::ALL.iter()
            .map(|rp| read_window(&data.join(format!("cems-efas/Europe_RP{}_filled_depth.tif", rp.years())), w, s, e, n))
            .collect::<Result<_>>()?;
        let efas: [_; 3] = efas.try_into().ok().unwrap();
        let lat0 = (s + n) / 2.0;
        let mut g = Graph { nodes: vec![], edges: vec![], incoming: vec![], outgoing: vec![], roads: vec![], index: RTree::new(),
                            lat0, efas, shelters, bbox };
        let mut ids: HashMap<i64, u32> = HashMap::new();
        for road in roads {
            let ri = g.roads.len() as u32;
            let ix: Vec<u32> = road.nodes.iter().map(|&(id, lon, lat)| *ids.entry(id).or_insert_with(|| {
                g.nodes.push((lon, lat));
                (g.nodes.len() - 1) as u32
            })).collect();
            for p in ix.windows(2) {
                let (a, b) = (g.nodes[p[0] as usize], g.nodes[p[1] as usize]);
                let len_m = g.dist(a, b) as f32;
                let (mlon, mlat) = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                let depth = g.efas.each_ref().map(|r| if r.contains(mlon, mlat) { r.at(mlon, mlat).max(0.0) } else { 0.0 });
                for (from, to, dir) in [(p[0], p[1], 1), (p[1], p[0], -1)] {
                    let against_oneway = road.oneway != 0 && road.oneway != dir;
                    g.edges.push(Edge { from, to, len_m, road: ri, depth, against_oneway });
                }
            }
            g.roads.push((road.highway, road.name));
        }
        if g.edges.is_empty() {
            bail!("no roads in this zone");
        }
        g.incoming = vec![vec![]; g.nodes.len()];
        g.outgoing = vec![vec![]; g.nodes.len()];
        for (i, e) in g.edges.iter().enumerate() {
            g.incoming[e.to as usize].push(i as u32);
            g.outgoing[e.from as usize].push(i as u32);
        }
        let pts = g.nodes.iter().enumerate().map(|(i, &p)| GeomWithData::new(g.xy(p), i as u32)).collect();
        g.index = RTree::bulk_load(pts);
        Ok(g)
    }

    fn xy(&self, (lon, lat): (f64, f64)) -> [f64; 2] {
        [lon * M_PER_DEG * self.lat0.to_radians().cos(), lat * M_PER_DEG]
    }

    fn dist(&self, a: (f64, f64), b: (f64, f64)) -> f64 {
        let (p, q) = (self.xy(a), self.xy(b));
        (p[0] - q[0]).hypot(p[1] - q[1])
    }

    pub fn summary(&self) -> Value {
        let km: f64 = self.edges.iter().map(|e| e.len_m as f64).sum::<f64>() / 2000.0;
        json!({"nodes": self.nodes.len(), "road_km": km.round(), "shelters": self.shelters.len()})
    }

    /// EFAS depth (m) at a point in the scenario, 0 if dry or outside the zone
    pub fn depth_at(&self, lon: f64, lat: f64, sc: Scenario) -> f32 {
        let r = &self.efas[scenario_ix(sc)];
        if r.contains(lon, lat) { r.at(lon, lat).max(0.0) } else { 0.0 }
    }

    fn allowed(&self, e: &Edge, mode: Mode) -> Option<f64> {
        if e.against_oneway && mode != Mode::Walk {
            return None;
        }
        mode.speed(&self.roads[e.road as usize].0)
    }

    /// Nearest node that `mode` can use within MAX_SNAP_M, and how far away it is
    fn snap(&self, lon: f64, lat: f64, mode: Mode) -> Option<(u32, f64)> {
        let p = self.xy((lon, lat));
        self.index.nearest_neighbor_iter_with_distance_2(&p).take(500)
            .take_while(|(_, d2)| *d2 <= MAX_SNAP_M * MAX_SNAP_M)
            .find(|(n, _)| self.incoming[n.data as usize].iter().any(|&e| self.allowed(&self.edges[e as usize], mode).is_some()))
            .map(|(n, d2)| (n.data, d2.sqrt()))
    }

    /// Is there a road `mode` can use within MAX_SNAP_M?
    pub fn near_road(&self, lon: f64, lat: f64, mode: Mode) -> bool {
        self.snap(lon, lat, mode).is_some()
    }

    pub fn contains(&self, lon: f64, lat: f64) -> bool {
        let [w, s, e, n] = self.bbox;
        lon >= w && lon <= e && lat >= s && lat <= n
    }

    /// Dijkstra from `root`. Backwards (towards = true): travel time from every node to root, and
    /// `via[n]` = edge to take from n. Forwards: time from root to every node, `via[n]` = edge into n.
    fn search(&self, root: u32, mode: Mode, sc: Scenario, towards: bool) -> (Vec<f64>, Vec<u32>) {
        let si = scenario_ix(sc);
        let mut time = vec![f64::INFINITY; self.nodes.len()];
        let mut next = vec![u32::MAX; self.nodes.len()];
        let mut heap = BinaryHeap::new();
        time[root as usize] = 0.0;
        heap.push(Reverse((0u64, root)));
        while let Some(Reverse((t_ms, v))) = heap.pop() {
            let t = t_ms as f64 / 1000.0;
            if t > time[v as usize] {
                continue;
            }
            let adj = if towards { &self.incoming } else { &self.outgoing };
            for &ei in &adj[v as usize] {
                let e = &self.edges[ei as usize];
                let Some(kmh) = self.allowed(e, mode) else { continue };
                let mut dt = e.len_m as f64 / (kmh / 3.6);
                if e.depth[si] > FLOODED_M {
                    dt += e.len_m as f64 * FLOOD_PENALTY_S_PER_M;
                }
                let u = if towards { e.from } else { e.to };
                if t + dt < time[u as usize] {
                    time[u as usize] = t + dt;
                    next[u as usize] = ei;
                    heap.push(Reverse((((t + dt) * 1000.0) as u64, u)));
                }
            }
        }
        (time, next)
    }

    /// Routes from each origin to the target for one mode (None = unreachable). Snapping distances
    /// are walked at 5 km/h and added to the time.
    pub fn routes(&self, origins: &[(f64, f64)], target: (f64, f64), mode: Mode, sc: Scenario) -> Vec<Option<Route>> {
        let Some((t, t_off)) = self.snap(target.0, target.1, mode) else { return origins.iter().map(|_| None).collect() };
        let (time, next) = self.search(t, mode, sc, true);
        origins.iter().map(|&(lon, lat)| {
            let (start, o_off) = self.snap(lon, lat, mode)?;
            if !time[start as usize].is_finite() {
                return None;
            }
            let (mut n, mut edges) = (start, vec![]);
            while n != t {
                let ei = next[n as usize];
                edges.push(ei);
                n = self.edges[ei as usize].to;
            }
            Some(self.route(edges, time[start as usize], o_off + t_off, mode, sc))
        }).collect()
    }

    /// `cost_s` = search cost along the edges (incl. flood penalty)
    fn route(&self, edges: Vec<u32>, cost_s: f64, off_m: f64, mode: Mode, sc: Scenario) -> Route {
        let si = scenario_ix(sc);
        let es = edges.iter().map(|&i| &self.edges[i as usize]);
        let distance_m = es.clone().map(|e| e.len_m as f64).sum::<f64>() + off_m;
        let flooded_m = es.clone().filter(|e| e.depth[si] > FLOODED_M).fold(0.0, |a, e| a + e.len_m as f64); // sum() gives -0.0
        let time_s = es.fold(0.0, |a, e| a + e.len_m as f64 / (self.allowed(e, mode).unwrap_or(5.0) / 3.6));
        Route { eta_s: time_s + off_m / WALK_MS, cost_s: cost_s + off_m / WALK_MS, distance_m, flooded_m, edges }
    }

    /// Fastest of the `targets` to reach from `origin`, with its index
    pub fn nearest(&self, origin: (f64, f64), targets: &[(f64, f64)], mode: Mode, sc: Scenario) -> Option<(usize, Route)> {
        let (o, o_off) = self.snap(origin.0, origin.1, mode)?;
        let (time, prev) = self.search(o, mode, sc, false);
        let (i, t, t_off) = targets.iter().enumerate()
            .filter_map(|(i, &(lon, lat))| { let (t, off) = self.snap(lon, lat, mode)?; Some((i, t, off)) })
            .filter(|&(_, t, _)| time[t as usize].is_finite())
            .min_by(|a, b| (time[a.1 as usize] + a.2 / WALK_MS).total_cmp(&(time[b.1 as usize] + b.2 / WALK_MS)))?;
        let (mut n, mut edges) = (t, vec![]);
        while n != o {
            let ei = prev[n as usize];
            edges.push(ei);
            n = self.edges[ei as usize].from;
        }
        edges.reverse();
        Some((i, self.route(edges, time[t as usize], o_off + t_off, mode, sc)))
    }

    fn bearing(&self, e: &Edge) -> f64 {
        let (a, b) = (self.xy(self.nodes[e.from as usize]), self.xy(self.nodes[e.to as usize]));
        (b[0] - a[0]).atan2(b[1] - a[1]).to_degrees()
    }

    /// GeoJSON line plus turn-by-turn steps, one per street
    pub fn describe(&self, r: &Route, sc: Scenario, arrive: &str) -> Value {
        let si = scenario_ix(sc);
        let mut coords = vec![];
        let mut steps: Vec<Value> = vec![];
        let (mut street, mut len, mut wet, mut action, mut last) = (None::<String>, 0.0, 0.0, "Start".to_string(), None::<&Edge>);
        let flush = |street: &Option<String>, len: f64, wet: f64, action: &str, steps: &mut Vec<Value>| {
            let name = street.clone().unwrap_or_else(|| "unnamed road".into());
            steps.push(json!({"instruction": format!("{action} on {name}"), "street": street, "distance_m": len.round(),
                              "flooded_m": wet.round()}));
        };
        for (k, &ei) in r.edges.iter().enumerate() {
            let e = &self.edges[ei as usize];
            if k == 0 {
                coords.push(self.nodes[e.from as usize]);
            }
            coords.push(self.nodes[e.to as usize]);
            let name = self.roads[e.road as usize].1.clone();
            if let Some(prev) = last {
                if name != street {
                    flush(&street, len, wet, &action, &mut steps);
                    let turn = (self.bearing(e) - self.bearing(prev) + 540.0) % 360.0 - 180.0;
                    action = match turn {
                        t if t.abs() < 30.0 => "Continue",
                        t if t.abs() > 150.0 => "Turn around",
                        t if t < 0.0 => "Turn left",
                        _ => "Turn right",
                    }.into();
                    (len, wet) = (0.0, 0.0);
                }
            }
            street = name;
            len += e.len_m as f64;
            if e.depth[si] > FLOODED_M {
                wet += e.len_m as f64;
            }
            last = Some(e);
        }
        if last.is_some() {
            flush(&street, len, wet, &action, &mut steps);
        }
        steps.push(json!({"instruction": arrive, "street": null, "distance_m": 0, "flooded_m": 0}));
        json!({
            "eta_s": r.eta_s.round(), "cost_s": r.cost_s.round(), "distance_m": r.distance_m.round(), "flooded_m": r.flooded_m.round(),
            "geometry": {"type": "LineString", "coordinates": coords.iter().map(|&(x, y)| [x, y]).collect::<Vec<_>>()},
            "steps": steps,
        })
    }
}
