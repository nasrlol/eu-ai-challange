# TODO (2026-10-29)

## 1. API fixes (before any AI layer)
- [ ] Drop or rename `expected_people_flooded_approx` (risk is a ranking, not a probability) → "risk-weighted population index"
- [ ] Wire `types.rs` `RegionResponse` into `main.rs` (caveats + sources in output)
- [ ] Compute per-cell `factors` + `overall`: flooded_pct, people, likelihood; others `NotAvailable`
- [ ] Out-of-distribution check: % cells outside training 1–99 pct (`code/risk/features.pkl`), river_points, nearest training AOI
- [ ] Fill `ScenarioSummary` (rp10/100/500); use `options.top_n` instead of hard-coded 20
- [ ] Label facility distances as straight-line (travel time via OSRM later)
- [ ] Write `.npy` via `grid_io` layout instead of `features.csv`
- [ ] Alert input: GDACS adapter (alert → polygon → `/region`)
- [ ] Add OBAT buildings + roads (map OSM `highway` → CEMS `road_code` for road_model)

## 1b. Model / data match (see [08_rust_region_api.md](08_rust_region_api.md))
- [ ] **Decide**: fix EFAS ≤1 px shift in `risk_model.py` (`from_bounds(...).round_offsets().round_lengths()`), then `--rebuild` + retrain; reported AUCs will change
- [ ] Test EMSR517 AOI05/06 and EMSR518 AOI01/02 (NRW + Belgium OSM extracts downloaded 2026-10-29)
- [ ] Validate API vs Python on all 8 kept AOIs (feature diffs + AUC), not just EMSR517 AOI15
- [ ] Close river drift (dist_river 242 m OSM vs 768 m CEMS): try OSM filters (e.g. drop small `stream`s), keep the one closest to CEMS

## 2. AI layer (design: [09_ai_layer.md](09_ai_layer.md))
- [ ] `RegionResponse` → facts list `{id, text, value, unit, source, origin}`
- [ ] Rule engine + closed action catalogue → candidate recs `{id, action, trigger, evidence}`
- [ ] One Claude call (facts only, key from env `ANTHROPIC_API_KEY`) → JSON statements `fact|inference|recommendation|unknown` with evidence ids
- [ ] Validator: numbers ∈ cited facts, rec ids ∈ candidates, evidence ids exist, ban "probability"/"certain"/"will"; fail → template + `ai_rejected`
- [ ] Pass model limits as facts (weak held-out AUC in EMSR517 AOI20/09/07, OSM vs CEMS rivers, GHS-POP 1 km, OSM = today)
- [ ] Log prompt/output/validation to `out/<name>/ai_log.json`

## 2b. Evacuation × money (see code/NOTES.md §14, `pipeline.py`)
- [ ] Add € damage per 1 km cell to `evacuation_<flood>.csv` (JRC curves × €1,500/m² placeholder); evacuation doesn't reduce building damage
- [ ] Optional scenario "€ saved by early warning" (contents/cars moved): needs a sourced reduction %, declare as external + assumption
- [ ] No lives-saved model exists; would need an external depth→fatality rate + evacuation success rate

## 3. Checks
- [ ] Annex 3/4: external data allowed (Copernicus DEM, OSM, OSRM)? Label as external in the brief
- [ ] Annex 3: AI-generated text in the tool allowed? how to declare
- [ ] AI sheet (Annex 7): describe the layer + validator
- [ ] Brief: output = draft for a named actor (e.g. crisis centre), no automatic dispatch
