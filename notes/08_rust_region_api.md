# Rust region API (in progress, 2026-10-29)

Goal: input = region with a flood alert (bbox / polygon, lon/lat) → fetch data → features that match our models.

Hosted copy: see [12_hosting.md](12_hosting.md) (valencia, systemd `flood-api`, update steps).

## Model contract (from `code/`)
| Model | Features | Grid |
|---|---|---|
| `risk_model.py` | `elev, slope, relief, hand, dist_river, rp10, rp100, rp500` | 100 m cell centres, EPSG:3035, permanent water excluded |
| `road_model.py` | above + `risk, road_code` | ~50 m road pieces |

Feature recipe (copy exactly):
- DEM: Copernicus GLO-30, merged at 1/3600°, bounds + 0.02° pad.
- slope: `np.gradient`, dy = 111320/3600, dx = dy·cos(mid-lat).
- relief: DEM − min filter, 33 px.
- hand: elev − river_z, where river_z = 5 px min filter sampled at river vertices (densified 50 m, snapped to 25 m).
- dist_river: metres to the nearest river vertex.
- EFAS: depth > 0, else 0. Sampling: floor of the pixel index.
- Model: `code/outputs/risk/model.joblib` (sklearn) → Rust computes features, Python scores.

## Environment
- cargo 1.98 installed, crates.io reachable. **No GDAL/PROJ**, so pure Rust: `tiff`, `proj4rs`, `geo`, `rstar`, `axum`, `reqwest`.
- No onnx / skl2onnx in `.venv`.

## Rasters
| File | Layout | Type / nodata |
|---|---|---|
| EFAS RP10/100/500 | 256 px tiles, DEFLATE, 4326, 3″ | float32, −9999 |
| GHS-POP 2020/2025 | 256 px tiles, LZW, 4326, 30″ | float64 |
| Copernicus DEM | 1024 px tiles, DEFLATE, no predictor | float32; 9 tiles cached in `working_data/dem/` |

## Rivers (training vs new regions)
- CEMS `hydrographyL`: 26,195 streams (BH141), 1,160 rivers (BH140).
- CEMS `hydrographyA`: lakes 6,829, land subject to inundation 683, rivers 463, reservoirs 239.
- New regions → OSM. The waterway filter **must include streams** (`river|stream|canal`) and `natural=water`, or `hand` / `dist_river` drift.
- Limit for the brief: OSM ≠ CEMS hydrography, so expect some shift from the training data.

## Open
- Alert input: GDACS adapter later; bbox/polygon first.
- Extras for allocation: population per cell, distance to fire station / hospital (OSM).
- Overpass is flaky (406 without User-Agent, 502). Retry mirrors and cache to disk.

## Handoff format: `code/grid_io.py` (no JSON)
- `.npy` structured array, one row per cell: `x, y` (EPSG:3035, f8) + `FEATURES` (f4), little-endian.
- Python: `grid_io.load(path)` / `from_bytes(body)` → DataFrame ready for `model.predict_proba(df[FEATURES])`.
- Rust: write the npy header (descr is in the module docstring) + raw rows. No crate needed.
- 1 M rows: save 0.03 s, load 0.02 s; JSON round trip 45 s.
- Changing `FEATURES` changes the layout, so update the Rust writer too.

## Data structures: `flood_api/src/types.rs` (first design; superseded by "/region v2" below)
| Stage | Types |
|---|---|
| Request | `RegionRequest { name, area, scenarios, options }`; `Area` = `bbox` / `polygon` / `aoi` (tagged by `"type"`); `Scenario` = rp10/rp100/rp500; `Options { score, facilities, top_n=20 }` |
| Rust → Python | `Cell` (`#[repr(C)]`, 48 B = one npy row in `grid_io` layout) + `NPY_DESCR`; `CellContext` (lon/lat, pop, facility distances) parallel to it; `Grid` holds both + `Facility` list |
| Python → Rust | `Scores { risk }` (same order as cells; ranking, not probability) |
| Explanation | `FactorKind` (7 access_model factors + thresholds), `Level`, `Factor { value, level, reason }`, `Overall::from_levels` (count of highs, no weights) |
| Response | `RegionResponse` → `RiskSummary` (+ `ScenarioSummary` per RP, `PriorityCell` with factors), `Files`, `Source { origin: challenge/external }`, `caveats` |

- Tests: npy row size, overall-level rule, tagged request parsing (`cargo test types`).
- Next: switch `features.rs` to `Grid`/`Cell`, write `.npy` instead of CSV, use `FacilityKind` in `osm.rs`, return `RegionResponse`.
- Factors needing roads/OBAT (cut_off, bridge, detour, single_storey) are `NotAvailable` until the API computes them.

## Built so far (flood_api/)
- Run: `cd flood_api && cargo build --release && ./target/release/flood_api` → `http://127.0.0.1:8080` (`FLOOD_ADDR`, `FLOOD_DATA`, `FLOOD_OUT`, `FLOOD_PYTHON`, `FLOOD_SCORER` override defaults).
- `POST /region {"name", "bbox":[w,s,e,n] | "polygon":<GeoJSON>, "score":true}` (current CSV version; `types.rs` request not wired yet).
- Output `flood_api/out/<name>/`: `features.csv` (x, y, lon, lat, 8 FEATURES, pop_density, dist_fire_station_m, dist_hospital_m), `facilities.geojson`, `risk.csv` + `risk.tif` (via `code/score_region.py`).
- Response: cells, km², facility counts, mean risk, high-risk km², people in high-risk (approx), top 20 cells by risk × people.
- Modules: `raster.rs` (tiled GeoTIFF windows, min filter), `features.rs` (grid + features, LAEA test vs pyproj < 1 cm), `osm.rs` + `pbf.rs`.
- Limit 3000 km² per request.

## OSM source
- Overpass down all day (502/504/timeout on 3 mirrors) → **offline Geofabrik extracts** in `working_data/osm/`: rheinland-pfalz (274 MB), nordrhein-westfalen (916 MB), belgium (698 MB).
- `pbf.rs`: picks extracts by header bbox, 2 passes (nodes+relations, then ways); ways cut at bbox edge. Overpass only if no extract covers the region.
- Parsed OSM cached per bbox in `flood_api/out/cache/`.

## Validation on EMSR517 AOI15 (6,002 cells inside the PRODUCT AOI, 11 s per request)
| Feature | API vs Python | Why |
|---|---|---|
| elev, slope, relief | median diff 0, corr ≥ 0.96 | same recipe |
| rp10/100/500 | API = raw EFAS exactly; Python differs on 4.7 % of cells | **bug in risk_model.py**: fractional `from_bounds` window → data read rounded, transform not → EFAS shifted ≤ 1 px (~90 m). **Fixed** (whole-pixel window, retrained; code/NOTES.md after step 13) |
| hand, dist_river | corr 0.84 / 0.07; dist median 242 m (OSM) vs 768 m (CEMS) | OSM has denser streams than CEMS hydrography  **Closed**: model retrained on OSM rivers (`RIVERS = "osm"`), same source as the API; held-out AUC 0.923 vs 0.922 |
- AUC vs observed flood: API features 0.962, Python (CEMS rivers) 0.973 → OSM rivers cost ~0.01 AUC. (Model trained incl. AOI15, so both optimistic.)
- Test request: boundary from `processed/` (PRODUCT ∪ MONIT01) is larger than the PRODUCT AOI used by risk_model; compare only inside the PRODUCT AOI.

## Model endpoints (`src/models.rs`, added 2026-10-29)
Region = `{"type":"bbox","bbox":[w,s,e,n]}` | `{"type":"aoi","aoi":"EMSR517_AOI15_GRA"}` | `{"type":"polygon","geometry":<GeoJSON>}`.
| Endpoint | Model | How |
|---|---|---|
| `GET /models` | all | list + `outputs/eval/summary.csv` |
| `POST /flood {name, region, floods:[rp10/rp100/rp500/observed/risk>=t], allow_outside}` | risk + access/evacuation | runs `flood_tool.py` → `summary.json` |
| `POST /timeline {name, region, driver}` | timeline | `driver.type` = `whatif {peak_rp, rise_h, fall_h, step_h}` / `open_meteo {point?, start, end}` / `series {steps:[{time, return_period}]}` → `steps.csv` as JSON |
| `POST /impact {aoi, cost_eur_m2, max_depth_m}` · `GET /impact` | pipeline (damage €, displaced) | runs `pipeline.py --json` (new flag) · `report/flood_summary.csv` |
| `GET /roads/{aoi}` · `GET /interp/{aoi}` | road_model, interp_model | stored outputs, filtered by AOI prefix |
| `POST /evacuation {name, region, flood, roads_flood, hours, weekend, weather}` | evac_sim (human evacuation flow) | `python3 code/evac_sim.py --region <w,s,e,n or AOI> --flood <rp100/T=500/…> [--roads-flood none\|T=10\|…] [--hours 3 8 17] [--weekend] [--weather dry rain heavy_rain fog snow] --name <name>` → `code/outputs/evacuations/<name>/summary.json` (+ `roads_<HHh_weather>.gpkg`, `cells_<…>.gpkg`, `viewer.html`). Exit code ≠ 0 + message on bad region/weather. ~5 s small bbox, ~1 min dense 450 km². API validates flood (`rp*`/`T=`/`observed`), roads_flood (+`none`), hours 0–23, weather ∈ dry/rain/heavy_rain/fog/snow; returns summary.json; listed in `/models`. |
- Python results land in `code/outputs/` (not `FLOOD_OUT`). Flood specs are whitelisted (no file paths).
- Tested: small bbox in AOI15 /flood + /timeline ~2 s; `/impact` EMSR518_AOI02 = report CSV (38 bldgs, €3.25 M). Big regions take minutes (synchronous).
- AOI paths (`/roads/{aoi}`, `/interp/{aoi}`): must match `EMSR\d{3}_AOI\d{2}(_DEL|_GRA)?` → else 422; valid but no results → 404. Match = exact or `aoi_…` (no loose prefixes).
- `GET /files/{path}`: read-only, serves files under `code/outputs/` only (canonicalised; `..`/symlinks outside → 404); folder → JSON listing. Content-type by extension (html, json, csv, tif, gpkg, pdf, png).
- All model responses (`/models`, `/flood`, `/timeline`, `/impact`, `/evacuation`, `/roads`, `/interp`) rewrite absolute paths under `code/outputs/` to `/files/…` URLs. `/region` (writes to `FLOOD_OUT`) not yet.
- `/flood` files: + `danger.{flood}` and `evacuation.{flood}` CSV URLs, `summary`.
- `NotFound` error type (main.rs) → 404; other errors stay 422.
- `/region` validation (`src/region_check.rs`) runs BEFORE OSM/raster work, 422 in < 5 ms: inverted / zero-area bbox, < 0.01 km², > 3000 km², > 1 % outside the study hull (same `study_region.gpkg` + rule as flood_tool). Polygon may be a geometry or a Feature; other types → 422.
- `/region` facility counts: `facilities` = inside the region only; `facilities_nearby_outside` = in the 0.02° pad (still used for distances). geojson features carry `inside`.
- `/region` outputs moved from `FLOOD_OUT/<name>` to `code/outputs/api_region/<name>`, returned as `/files/…` URLs (FLOOD_OUT keeps cache + help.sqlite).
- Errors (all JSON `{"error"}`): 400 unparseable JSON · 404 unknown AOI / zone / helper / request · 409 state conflict (helper busy, request already assigned/done) · 422 everything else. axum rejections (body, query, path) wrapped by `extract::{JsonBody, QueryArgs, PathArg}` in main.rs.
- Python failures → 422 with only the last stderr line (no traceback).
- AOI regions (`/flood`, `/timeline`, `/evacuation`) and `/impact`: AOI checked in Rust first, needs `_DEL`/`_GRA` (else 422), unknown → 404.
- `/evacuation`: `T=` must be finite; duplicate hours / weathers dropped; `observed` (flood or roads_flood) needs an AOI region; `/flood` same guard.
- CORS (`tower-http` 0.5 `CorsLayer`, main.rs `cors()`): all routes incl. `/help/*`, `/files`; methods GET/POST/PATCH, header content-type. Origins from env `FLOOD_CORS_ORIGINS` (comma list, default `*`); bad origin → server refuses to start. Tested: OPTIONS preflight → `access-control-allow-*`; with a list, unlisted origins get no allow-origin header. Tighten before any public deploy.
- `/flood` `files.danger` / `files.evacuation` list only this request's floods (label = `flood_label`: `risk>=0.5` → `risk_ge_0.5`); older CSVs in a reused folder are left on disk but not listed. `/timeline`, `/evacuation` return fixed file names (no globbing).
- `/impact`: `cost_eur_m2`, `max_depth_m` must be finite and > 0 → else 422 (−5 gave a negative € loss).

## /region v2 (cems-efas-72, 2026-10-29 evening)
- Request (old shape still works): `{name, bbox | polygon | region:{type:bbox|polygon}, score=true, top_n=20 (≤500), scenarios=[rp10,rp100,rp500]}`. `region.type=aoi` → 422 (use /flood etc.).
- Response = `types::RegionResponse`: name, cells, area_km2, river_points, facilities, facilities_nearby_outside, `risk`, `region_check`, `files`, `sources`, `caveats`.
- `risk`: mean, high_risk_km2, people_in_high_risk_approx, **`risk_weighted_people_index`** (was `expected_people_flooded_approx`), `by_scenario` [{scenario, flooded_km2 (>0 m), deep_km2 (≥0.3 m), people_approx, max_depth_m}], `priority_cells` [{rank, lon, lat, risk, people, dist_fire_station_m, dist_hospital_m (straight-line, null if none), overall, factors}].
- Factors = all 8 of access_model.FACTORS (+ `help_km` added to types): likelihood, people, help_km (straight-line) computed; the 5 needing roads/buildings `not_available` with value null → use /flood.
- `region_check` (from `score_region.py` → `risk_check.json`): % cells outside training 1–99 pct per feature + any, nearest training AOI, km, its held-out AUC. > 10 % outside → extra caveat. Ranges cached in `outputs/risk/feature_ranges.json` (rebuilt when model.joblib is newer).
- Handoff: Rust writes `grid.npy` (grid_io layout, `types::write_npy`); scorer reads npy (csv still accepted), writes `risk.csv` = x, y, risk only. `features.csv` kept as download.
- Async (all POST routes, `src/jobs.rs`): `?async=1` → 202 `{job, status_url}`; `GET /jobs/{id}` → 202 running / route's own status + body. In memory, 6 h, lost on restart.
- 2026-10-30: `/region` also writes `cells.geojson` (`files.cells`): 100 m squares in lon/lat for cells with risk ≥ 0.2 or EFAS rp500 > 0; props `r`, `d10`, `d100`, `d500` (2 decimals). Study bbox 25k cells → 654 KB.
- 2026-10-30: `GET /point?lon&lat` (< 0.1 s, no OSM/model): raw EFAS depth per RP at the pixel + nearest pixel ≥ 0.3 m within ~300 m. Level per RP: ≥ 1 m / ≥ 0.3 m here → `danger`; water ≥ 0.3 m within 100 m or < 0.3 m here → `at_risk`; else `safe`; overall = worst. Same study-region check as /region (~600 m box). Note: 1 km cell centres from /flood are often dry pixels; EFAS includes river channels (12 m "depth").

## OSM store (2026-10-30, `src/osm_store.rs`)
- Why: every new bbox re-read the extracts (2 GB, 2 passes): `/region` 2–3 min, `/help/zones` ~3.5 min; the per-bbox JSON cache only helped for the identical bbox.
- Build once: `flood_api build-osm-store` → `<FLOOD_OUT>/osm_store` (or `FLOOD_OSM_STORE`). Study hull bbox + 0.15°; 4 parallel passes per extract (relations → needed node ids → node coords → ways), keeps rivers, water, water-relation outers, facilities, shelters, routable roads with full node lists. 0.1° tiles (bincode), way in every tile it touches; atomic swap of the folder.
- valencia: 146 s, 3.42 M ways, 4,455 facility points, 691 tiles, 802 MB.
- Query: tiles of the bbox (parallel load, in-memory LRU ≤ 1 GB), dedupe ways by id, same bbox cut as `pbf::read` / `read_roads`. Store hit → no per-bbox cache written.
- Fallback: store missing / stale (`manifest.json` extract names + size + mtime differ) / bbox outside → extracts as before. Startup prints which.
- Parity `flood_api check-osm-store w s e n` (store vs extracts):
  | bbox | store | extracts | result |
  |---|---|---|---|
  | 7.07,50.49,7.17,50.57 (/region test + pad) | 0.02 s | 274 s | all counts identical |
  | 5.6,50.55,5.96,50.64 (default help zone) | 0.12 s | 433 s | all counts identical |
  | 6.0,50.55,6.3,50.75 (DE/BE border) | 0.10 s | 486 s | roads, nodes, facilities, shelters identical; extracts keep water ways twice where extracts overlap (raw 38,937 vs 34,534 river pts); unique rivers identical (2,038) |
- Check peak RSS 0.5–1.0 GB (extract side). Test server uses the live store via symlink `srv_out/osm_store`.
- End-to-end (:8081, then live 02:22): new /help/zones 157 km² **0.34 s** (was ~3.5 min); new /region 160 km² 12.7 s (was 2–3 min); repeat 4.0–4.6 s, of which features 0.23 s, rest = Python scorer (start + imports + region_check). t72 bbox unchanged (1,895 cells, 49/15, 1.89 km², mean 0.13319). Next if needed: persistent Python worker.
- 2026-10-30 (for hackathon-bc): `cells.geojson` + `pop` (people per cell) + `elev` (DEM m); types::CAVEATS + flood_tool.CAVEATS + Annex 2 limits (EMS snapshot, outside AOI = not mapped, EFAS < 500 km², 500-yr ceiling, GHS-POP residents; OBAT Belgium in flood_tool only).

## Frontend asks (2026-10-30, branch `frontend-asks`; frontend view in [17_frontend_handoff.md](17_frontend_handoff.md))
- `raster.rs`: 0-byte (sparse) tiles stay NaN instead of failing ("corrupt deflate stream"; EFAS tile #41946 near 5.3 E 50.1 N). EFAS NaN → depth 0, GHS-POP NaN → 0.
- `/region`: `region.type = "aoi"` → outline of the AOI's largest part from `study_region.gpkg` layer `aois` (`region_check::aoi_ring`, WKB Polygon/MultiPolygon parser `outer_rings` + unit test); suffix optional when unique. Cells get `id` = `"<x>_<y>"` (EPSG:3035 centre).
- `/timeline`: `files.cells_geojson` (timeline.py `region_cells`: same 100 m grid as features.rs `build`, origin = bounds of the region / AOI largest part, OSM water excluded, depth per step from the DEM-checked depth grid, wet cells only, top-level `steps`). `series` driver: steps with `discharge` + `q10`/`q100` → Gumbel T.
- `src/reuse.rs` (layer inside jobs): POST /region /flood /timeline /evacuation /layers stored on 200 under `FLOOD_OUT/reuse/<route>/<name>.json` with the request (minus `reuse`); `"reuse": true` + identical body → stored answer + `"reused": true`, header `x-reused: 1`.
- `POST /layers {name, region}` → `code/area_layers.py` → `outputs/layers/<name>/{buildings,places,shelters}.geojson` + summary. Places/shelters = new layers in `cache/osm_study_region.gpkg` (`osm_data.build_extra`, one pass over the extracts, built on first use or `python osm_data.py --extra`). Shelter `cap` = GHS-OBAT floor area (area × floors) inside the OSM outline (point: building within 25 m) × 0.5 / 3.5 m² (Sphere): assumption. Max 300k buildings.
- GeoJSON copies: `evac_sim.py` writes `roads_/cells_<HHh_weather>.geojson` (+ `files.roads_geojson/cells_geojson`); `code/export_geojson.py` writes `outputs/roads/geojson/<AOI>.geojson`, `outputs/interp/geojson/<pair>.geojson` (listed in `/roads`, `/interp` `files.*_geojson`); `region.py` writes `outputs/region/study_region.geojson`.
- Tests (2026-10-30, test build :8091, `code/testrun.sh fa`): cargo test 5/5; Q8 bbox 200 (7 s, 57,749 cells); /region aoi EMSR517_AOI18_GRA 26.7 km² 2,666 cells, suffix optional, unknown → 422; /timeline whatif ref bbox: cells_geojson 96 cells, 94 ids in /region cells (rest dry, risk < 0.2); series discharge 40/300/420 → T 3.9/100/480; reuse 4.9 s → 0.04 s, changed body reruns, works under ?async=1, errors not stored; /evacuation roads_geojson 57 lines; /roads /interp geojson, study_region.geojson OK. Not tested (user: skip): /layers, old-vs-new parity.
- `code/testrun.sh`: also excludes AI-Challenge/, policy_api/, docs/ (were uploaded to every test copy: 245 MB policy_api/target, private docs).
