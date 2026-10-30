//! Data structures of the /region response, the npy handoff to the Python scorer and the
//! explained danger per cell. Mirrors code/grid_io.py (npy row layout), risk_model.py (FEATURES,
//! 0.5 threshold) and access_model.py (factor thresholds, overall level). Change those and this
//! file together.
use crate::features::{FEATURES, GRID_M};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, io::Write, path::PathBuf};

// ---------- Scenarios ----------

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
    /// Column in FEATURES holding this scenario's EFAS depth
    pub fn feature(self) -> usize {
        match self { Scenario::Rp10 => 5, Scenario::Rp100 => 6, Scenario::Rp500 => 7 }
    }
}

// ---------- Grid (Rust -> Python), grid_io.GRID_DTYPE ----------

/// numpy descr of one row: x, y (EPSG:3035 cell centre, f8) then FEATURES (f4), little-endian
pub fn npy_descr() -> String {
    let cols: Vec<String> = ["x", "y"].iter().map(|c| format!("('{c}', '<f8')"))
        .chain(FEATURES.iter().map(|f| format!("('{f}', '<f4')"))).collect();
    format!("[{}]", cols.join(", "))
}

/// Write cells as a .npy structured array (format 1.0): grid_io.load reads it without parsing
pub fn write_npy(path: &std::path::Path, xy: &[(f64, f64)], feats: &[[f32; 8]]) -> std::io::Result<()> {
    let mut head = format!("{{'descr': {}, 'fortran_order': False, 'shape': ({},), }}", npy_descr(), xy.len());
    // magic (6) + version (2) + length (2) + header incl. newline: total a multiple of 64
    head.push_str(&" ".repeat(63 - (10 + head.len()) % 64));
    head.push('\n');
    let mut w = std::io::BufWriter::new(std::fs::File::create(path)?);
    w.write_all(b"\x93NUMPY\x01\x00")?;
    w.write_all(&(head.len() as u16).to_le_bytes())?;
    w.write_all(head.as_bytes())?;
    for (&(x, y), f) in xy.iter().zip(feats) {
        w.write_all(&x.to_le_bytes())?;
        w.write_all(&y.to_le_bytes())?;
        for v in f {
            w.write_all(&v.to_le_bytes())?;
        }
    }
    w.flush()
}

/// Same threshold as risk_model.py / road_model.py
pub const HIGH_RISK: f32 = 0.5;

/// People per 100 m cell from GHS-POP people per km² (1 km source, so rough at 100 m)
pub fn people(pop_density: f32) -> f32 {
    pop_density * (GRID_M * GRID_M / 1e6) as f32
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
#[serde(rename_all = "snake_case")]
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
    HelpKm,
    People,
    SingleStoreyPct,
    Likelihood,
}

impl FactorKind {
    pub const ALL: [FactorKind; 8] = [FactorKind::FloodedPct, FactorKind::CutOffPct, FactorKind::BridgePct, FactorKind::Detour,
                                      FactorKind::HelpKm, FactorKind::People, FactorKind::SingleStoreyPct, FactorKind::Likelihood];

    /// (label, medium, high) as in access_model.FACTORS
    pub fn spec(self) -> (&'static str, f32, f32) {
        match self {
            FactorKind::FloodedPct => ("Flooded buildings", 5.0, 25.0),
            FactorKind::CutOffPct => ("Cut off by road", 10.0, 50.0),
            FactorKind::BridgePct => ("Depends on bridges", 10.0, 50.0),
            FactorKind::Detour => ("Long detour", 1.3, 2.0),
            FactorKind::HelpKm => ("Far from help", 8.0, 15.0),
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
    pub label: &'static str,
    /// null when not available
    pub value: Option<f32>,
    pub level: Level,
    /// Plain-language reason, e.g. "relative terrain risk 0.62 (a ranking, not a probability)"
    pub reason: String,
}

impl Factor {
    pub fn new(kind: FactorKind, value: f32, reason: String) -> Self {
        Factor { kind, label: kind.spec().0, value: (!value.is_nan()).then_some(value), level: kind.level(value), reason }
    }
}

// ---------- Response ----------

#[derive(Serialize)]
pub struct RegionResponse {
    pub name: String,
    pub cells: usize,
    pub area_km2: f64,
    /// River vertices used for hand / dist_river; low counts mean OSM rivers are sparse
    pub river_points: usize,
    /// OSM facilities inside the region, by kind
    pub facilities: BTreeMap<String, usize>,
    /// In the 0.02° pad around it (still used for distances)
    pub facilities_nearby_outside: BTreeMap<String, usize>,
    pub risk: Option<RiskSummary>,
    /// How far the region is from the training data (code/score_region.py); null without scoring
    pub region_check: Option<Value>,
    pub files: Files,
    pub sources: &'static [Source],
    /// Limits to show next to any number
    pub caveats: Vec<&'static str>,
}

#[derive(Serialize)]
pub struct RiskSummary {
    pub mean: f32,
    pub high_risk_km2: f64,
    pub people_in_high_risk_approx: f32,
    /// Σ risk × people: for comparing regions only (risk is a ranking, not a probability)
    pub risk_weighted_people_index: f32,
    pub by_scenario: Vec<ScenarioSummary>,
    /// Ranked by risk × people
    pub priority_cells: Vec<PriorityCell>,
}

#[derive(Serialize)]
pub struct ScenarioSummary {
    pub scenario: Scenario,
    /// Cells with EFAS depth > 0
    pub flooded_km2: f64,
    /// Cells with EFAS depth >= 0.3 m (road-impassable threshold, access_model.py)
    pub deep_km2: f64,
    pub people_approx: f32,
    pub max_depth_m: f32,
}

#[derive(Serialize)]
pub struct PriorityCell {
    pub rank: usize,
    pub lon: f64,
    pub lat: f64,
    pub risk: f32,
    pub people: f32,
    /// Straight-line metres to the nearest OSM facility (not travel distance); null if none found
    pub dist_fire_station_m: Option<f32>,
    pub dist_hospital_m: Option<f32>,
    pub overall: Overall,
    pub factors: Vec<Factor>,
}

#[derive(Serialize, Default)]
pub struct Files {
    /// Every cell: x, y, lon, lat, FEATURES, pop_density, straight-line facility distances
    pub features: PathBuf,
    /// Same cells in grid_io npy layout (the scorer's input)
    pub grid: PathBuf,
    pub facilities: PathBuf,
    pub risk: Option<PathBuf>,
    pub risk_tif: Option<PathBuf>,
    /// Map layer: cells with risk >= 0.2 or wet in any EFAS scenario, 100 m squares in lon/lat
    pub cells: Option<PathBuf>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// In the challenge zip (EFAS, GHS-POP, OBAT, CEMS)
    Challenge,
    /// Fetched by us (Copernicus DEM, OSM); cite in the brief
    External,
}

#[derive(Serialize, Clone, Copy, Debug)]
pub struct Source {
    pub name: &'static str,
    pub used_for: &'static str,
    pub origin: Origin,
    pub licence: &'static str,
}

pub const SOURCES: &[Source] = &[
    Source { name: "EFAS flood hazard maps RP10/100/500", used_for: "rp10, rp100, rp500, by_scenario", origin: Origin::Challenge, licence: "Copernicus EMS" },
    Source { name: "GHS-POP 2020 (30″)", used_for: "people", origin: Origin::Challenge, licence: "CC BY 4.0" },
    Source { name: "CEMS Rapid Mapping EMSR517/518/520", used_for: "training labels of the risk model", origin: Origin::Challenge, licence: "Copernicus EMS" },
    Source { name: "Copernicus DEM GLO-30", used_for: "elev, slope, relief, hand", origin: Origin::External, licence: "Copernicus DEM licence (free use)" },
    Source { name: "OpenStreetMap (Geofabrik extracts)", used_for: "rivers, water, fire stations, hospitals", origin: Origin::External, licence: "ODbL" },
];

pub const CAVEATS: &[&str] = &[
    "risk is a relative ranking from terrain and EFAS hazard, not a calibrated probability",
    "people are approximate: GHS-POP is 1 km, spread evenly over 100 m cells",
    "facility distances are straight-line, not travel distance; real road distances are longer",
    "OSM is today's map, not July 2021; OSM rivers differ from the CEMS hydrography",
    "factors that need roads or buildings (cut off, bridges, detour, flooded buildings, single storey) are not computed here; use POST /flood",
    "the model learned from CEMS maps: a snapshot when the satellite passed, not the flood peak; outside the mapped AOIs \"not flooded\" means \"not mapped\"",
    "EFAS leaves out rivers with an upstream catchment under 500 km², so small streams and flash floods are missing",
    "the 1-in-500-year map is the ceiling of the scenarios; July 2021 went beyond it in places",
    "GHS-POP counts residents only (no commuters or visitors)",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npy_header_is_aligned() {
        let p = std::env::temp_dir().join("flood_api_types_test.npy");
        write_npy(&p, &[(1.0, 2.0), (3.0, 4.0)], &[[0.0; 8], [1.0; 8]]).unwrap();
        let b = std::fs::read(&p).unwrap();
        let head = u16::from_le_bytes([b[8], b[9]]) as usize;
        assert_eq!((10 + head) % 64, 0);
        assert_eq!(b.len(), 10 + head + 2 * (16 + 4 * FEATURES.len()));
        assert_eq!(npy_descr().matches("'<f4'").count(), FEATURES.len());
        std::fs::remove_file(p).ok();
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
    fn missing_value_is_not_available() {
        let f = Factor::new(FactorKind::CutOffPct, f32::NAN, "x".into());
        assert_eq!((f.level, f.value), (Level::NotAvailable, None));
        assert_eq!(FactorKind::HelpKm.level(16.0), Level::High);
    }
}
