//! Data structures of the region API: what comes in, what goes to the Python scorer, what goes out.
//! Mirrors code/grid_io.py (npy row layout), risk_model.py (FEATURES, 0.5 threshold) and
//! access_model.py (factor thresholds, overall level). Change those and this file together.
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------- Request ----------

#[derive(Deserialize)]
pub struct RegionRequest {
    /// Output folder name, e.g. an alert id: letters, digits, _ or -
    pub name: String,
    pub area: Area,
    /// EFAS return periods to report; empty = all three
    #[serde(default)]
    pub scenarios: Vec<Scenario>,
    #[serde(default)]
    pub options: Options,
}

/// Region in lon/lat (EPSG:4326)
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Area {
    /// [west, south, east, north] in degrees
    Bbox { bbox: [f64; 4] },
    /// GeoJSON Polygon geometry; outer ring only
    Polygon { geometry: Value },
    /// One of our training AOIs, e.g. "EMSR517_AOI15" (boundary from processed/ems_clean.gpkg)
    Aoi { code: String },
}

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Scenario {
    Rp10,
    Rp100,
    Rp500,
}

impl Scenario {
    pub const ALL: [Scenario; 3] = [Scenario::Rp10, Scenario::Rp100, Scenario::Rp500];
    pub fn years(self) -> u32 {
        match self { Scenario::Rp10 => 10, Scenario::Rp100 => 100, Scenario::Rp500 => 500 }
    }
}

#[derive(Deserialize)]
#[serde(default)]
pub struct Options {
    /// Run the sklearn model (code/risk/model.joblib) on the grid
    pub score: bool,
    /// Fetch OSM emergency facilities and compute distances
    pub facilities: bool,
    /// Number of priority cells returned
    pub top_n: usize,
}

impl Default for Options {
    fn default() -> Self { Options { score: true, facilities: true, top_n: 20 } }
}

// ---------- Grid (Rust -> Python) ----------

pub const GRID_M: f64 = 100.0;
pub const N_FEATURES: usize = 8;
pub const FEATURES: [&str; N_FEATURES] = ["elev", "slope", "relief", "hand", "dist_river", "rp10", "rp100", "rp500"];
/// numpy descr for grid_io.GRID_DTYPE; must match `Cell` field for field
pub const NPY_DESCR: &str = "[('x', '<f8'), ('y', '<f8'), ('elev', '<f4'), ('slope', '<f4'), ('relief', '<f4'), \
('hand', '<f4'), ('dist_river', '<f4'), ('rp10', '<f4'), ('rp100', '<f4'), ('rp500', '<f4')]";

/// One 100 m cell as the model sees it: exactly one npy row (48 bytes, little-endian, no padding).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Cell {
    /// Cell centre, EPSG:3035 metres
    pub x: f64,
    pub y: f64,
    /// In FEATURES order; EFAS depths are 0 where dry
    pub features: [f32; N_FEATURES],
}

/// Per-cell data the model does not use, kept parallel to `Grid::cells` (same index).
#[derive(Clone, Copy, Debug, Serialize)]
pub struct CellContext {
    pub lon: f64,
    pub lat: f64,
    /// GHS-POP 2020, people per km² (1 km source, so rough at 100 m)
    pub pop_density: f32,
    /// Straight-line metres to nearest OSM facility; NaN if none found
    pub dist_fire_station_m: f32,
    pub dist_hospital_m: f32,
}

impl CellContext {
    pub fn people(&self) -> f32 { self.pop_density * (GRID_M * GRID_M / 1e6) as f32 }
}

pub struct Grid {
    pub cells: Vec<Cell>,
    pub context: Vec<CellContext>,
    pub facilities: Vec<Facility>,
    /// River vertices used for hand / dist_river; low counts mean OSM rivers are sparse
    pub n_river_pts: usize,
}

impl Grid {
    pub fn area_km2(&self) -> f64 { self.cells.len() as f64 * GRID_M * GRID_M / 1e6 }
}

// ---------- Facilities (OSM) ----------

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[serde(rename_all = "snake_case")]
pub enum FacilityKind {
    FireStation,
    Hospital,
    Police,
    AmbulanceStation,
}

#[derive(Serialize, Clone, Debug)]
pub struct Facility {
    pub kind: FacilityKind,
    pub name: Option<String>,
    pub lon: f64,
    pub lat: f64,
}

// ---------- Scores (Python -> Rust) ----------

/// Same threshold as risk_model.py / road_model.py
pub const HIGH_RISK: f32 = 0.5;

/// Model output, same order as `Grid::cells`. Relative ranking, not a calibrated probability.
pub struct Scores {
    pub risk: Vec<f32>,
}

// ---------- Explained danger (access_model.py logic) ----------

#[derive(Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    NotAvailable,
    Low,
    Medium,
    High,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Overall {
    Low,
    Elevated,
    High,
    Critical,
}

impl Overall {
    /// No weights: >=3 high Critical, 2 High, 1 high or >=2 medium Elevated
    pub fn from_levels(levels: &[Level]) -> Self {
        let n = |l| levels.iter().filter(|&&x| x == l).count();
        match (n(Level::High), n(Level::Medium)) {
            (h, _) if h >= 3 => Overall::Critical,
            (2, _) => Overall::High,
            (1, _) | (_, 2..) => Overall::Elevated,
            _ => Overall::Low,
        }
    }
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum FactorKind {
    FloodedPct,
    CutOffPct,
    BridgePct,
    Detour,
    People,
    SingleStoreyPct,
    Likelihood,
}

impl FactorKind {
    /// (label, medium, high) as in access_model.FACTORS
    pub fn spec(self) -> (&'static str, f32, f32) {
        match self {
            FactorKind::FloodedPct => ("Flooded buildings", 5.0, 25.0),
            FactorKind::CutOffPct => ("Cut off by road", 10.0, 50.0),
            FactorKind::BridgePct => ("Depends on bridges", 10.0, 50.0),
            FactorKind::Detour => ("Long detour", 1.3, 2.0),
            FactorKind::People => ("People affected", 100.0, 500.0),
            FactorKind::SingleStoreyPct => ("No upper floor", 20.0, 50.0),
            FactorKind::Likelihood => ("Flood-prone terrain", 0.3, 0.5),
        }
    }

    pub fn level(self, v: f32) -> Level {
        let (_, med, high) = self.spec();
        if v.is_nan() { Level::NotAvailable } else if v >= high { Level::High } else if v >= med { Level::Medium } else { Level::Low }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct Factor {
    pub kind: FactorKind,
    pub value: f32,
    pub level: Level,
    /// Plain-language reason, e.g. "96% of buildings unreachable from outside"
    pub reason: String,
}

// ---------- Response ----------

#[derive(Serialize)]
pub struct RegionResponse {
    pub name: String,
    pub cells: usize,
    pub area_km2: f64,
    pub river_points: usize,
    pub facilities: std::collections::BTreeMap<FacilityKind, usize>,
    pub risk: Option<RiskSummary>,
    pub files: Files,
    pub sources: Vec<Source>,
    /// Limits to show next to any number, e.g. "OSM rivers differ from CEMS training hydrography"
    pub caveats: Vec<&'static str>,
}

#[derive(Serialize)]
pub struct RiskSummary {
    pub mean: f32,
    pub high_risk_km2: f64,
    pub people_in_high_risk_approx: f32,
    pub expected_people_flooded_approx: f32,
    pub by_scenario: Vec<ScenarioSummary>,
    /// Ranked by risk × people
    pub priority_cells: Vec<PriorityCell>,
}

#[derive(Serialize)]
pub struct ScenarioSummary {
    pub scenario: Scenario,
    /// Cells with EFAS depth >= 0.3 m (road-impassable threshold)
    pub flooded_km2: f64,
    pub people_approx: f32,
}

#[derive(Serialize)]
pub struct PriorityCell {
    pub rank: usize,
    #[serde(flatten)]
    pub context: CellContext,
    pub risk: f32,
    pub people: f32,
    pub overall: Overall,
    pub factors: Vec<Factor>,
}

#[derive(Serialize, Default)]
pub struct Files {
    /// npy grid in grid_io layout
    pub grid: Option<std::path::PathBuf>,
    pub risk_tif: Option<std::path::PathBuf>,
    pub facilities_geojson: Option<std::path::PathBuf>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// In the challenge zip (EFAS, GHS-POP, OBAT, CEMS)
    Challenge,
    /// Fetched by us (Copernicus DEM, OSM); cite in the brief
    External,
}

#[derive(Serialize, Clone, Debug)]
pub struct Source {
    pub name: &'static str,
    pub origin: Origin,
    pub licence: &'static str,
}

#[derive(Serialize)]
pub struct ApiError {
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_matches_npy_row() {
        assert_eq!(std::mem::size_of::<Cell>(), 16 + 4 * N_FEATURES);
        assert_eq!(NPY_DESCR.matches("'<f4'").count(), N_FEATURES);
    }

    #[test]
    fn overall_counts_highs() {
        use Level::*;
        assert_eq!(Overall::from_levels(&[High, High, High, Low]), Overall::Critical);
        assert_eq!(Overall::from_levels(&[High, High, Low]), Overall::High);
        assert_eq!(Overall::from_levels(&[Medium, Medium, Low]), Overall::Elevated);
        assert_eq!(Overall::from_levels(&[Medium, NotAvailable]), Overall::Low);
    }

    #[test]
    fn area_is_tagged() {
        let a: RegionRequest = serde_json::from_str(r#"{"name":"t","area":{"type":"bbox","bbox":[6.9,50.4,7.2,50.6]}}"#).unwrap();
        assert!(matches!(a.area, Area::Bbox { .. }));
        assert!(a.options.score && a.scenarios.is_empty());
    }
}
