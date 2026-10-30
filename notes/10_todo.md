# TODO (2026-10-29)

## 1. API fixes (before any AI layer)
- [x] Drop or rename `expected_people_flooded_approx` (risk is a ranking, not a probability) → "risk-weighted population index"
- [x] Wire `types.rs` `RegionResponse` into `main.rs` (caveats + sources in output)
- [x] Compute per-cell `factors` + `overall`: flooded_pct, people, likelihood; others `NotAvailable`
- [x] Out-of-distribution check: % cells outside training 1–99 pct (`code/risk/features.pkl`), river_points, nearest training AOI
- [x] Fill `ScenarioSummary` (rp10/100/500); use `options.top_n` instead of hard-coded 20
- [x] Label facility distances as straight-line (travel time via OSRM later)
- [x] Write `.npy` via `grid_io` layout instead of `features.csv`
- [x] (forecast tab, notes/15) weather → flood size model in the frontend · [ ] Alert input: GDACS adapter (alert → polygon → `/region`)
- [ ] Add OBAT buildings + roads (map OSM `highway` → CEMS `road_code` for road_model)

## 1b. Model / data match (see [08_rust_region_api.md](08_rust_region_api.md))
- [x] Fix EFAS ≤1 px shift in `risk_model.py` (`from_bounds(...).round_offsets().round_lengths()`), then `--rebuild` + retrain; reported AUCs will change
- [ ] Test EMSR517 AOI05/06 and EMSR518 AOI01/02 (NRW + Belgium OSM extracts downloaded 2026-10-29)
- [ ] Validate API vs Python on all 8 kept AOIs (feature diffs + AUC), not just EMSR517 AOI15
- [x] (retrained on OSM rivers, code/NOTES.md) Close river drift (dist_river 242 m OSM vs 768 m CEMS): try OSM filters (e.g. drop small `stream`s), keep the one closest to CEMS

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
- Annex check 2026-10-30: external data (OSM, Eurostat…) explicitly allowed (Annex 2) → resolved; Annex 3 doesn't forbid AI text in the tool, only keys/personal data/whole datasets → resolved
- [x] (done by cems-efas-72 2026-10-30, /region + /flood) Add missing Annex 2 caveats to `types::CAVEATS` / `flood_tool.CAVEATS`: EMS = snapshot not peak, outside AOI = not mapped, EFAS skips rivers < 500 km² + 500-yr is the ceiling (2021 exceeded it), OBAT undercounts Belgium, GHS-POP = residents only
- [ ] Write the policy brief (Annex 6 template, ≥1–2 figures, references with URL+date) and the AI sheet (Annex 7: team number only, ≤5 AI errors, 3 prompts, checks, limits)
- [ ] Submit by 13:30 Day 2 to JRC-SI@ec.europa.eu + info@fari.brussels; upload outputs to `gs://outputs_challenge/team-XX/`
- [x] (done by cems-efas-72) Remove `/home/nasr` paths from code/NOTES.md + notes/13 before quoting prompts (no personal data)

## 4. Help a friend (notes/11)
- [x] Restart the 8080 server to load the new endpoints (now hosted on valencia, see notes/12) (help + models)
- [ ] Brief/AI sheet: name the operator (e.g. municipality / Red Cross), GDPR Art. 9, the assumed thresholds
- [ ] Optional: map view of route.geojson for the policy tool

## 5. Done 2026-10-29 evening (cems-efas-72, details notes/08 "/region v2")
- [x] `?async=1` + `GET /jobs/{id}` for slow POSTs · [x] DELETE helpers / help-requests · [x] lon/lat 4 decimals in danger/evacuation CSVs + /flood top_cells · [x] bbox order check on /flood, /timeline, /evacuation (B12)
- Still open: GDACS input (alerts are live, our data is July 2021 → low value), OBAT/roads in /region (covered by /flood), API vs Python on the other AOIs, GeoJSON depth-grid endpoint, AI layer (user: skip for now), auth/public (user: leave tailnet-only), SQLite reuses deleted helper/request ids (needs AUTOINCREMENT migration).

## 6. Challenge-brief coverage gaps (2026-10-30, coverage table vs the policy brief list)
Quick wins (~1–2 h each):
- [ ] Decision log: who / level (commune, region, federal, EU) / when / model numbers at that moment, export → accountability + multi-level coordination
- [ ] Needs ↔ supplies matching: extend Help a friend with resources (boats, pumps, shelter beds, generators) matched by distance + road access
- [ ] Alert export as CAP (Common Alerting Protocol) per wave / language, ready for SMS, cell broadcast, social media
- [ ] Resource inventory per area (boats, pumps, crews) feeding the emergency-teams dispatch
- [ ] Roadmap per scenario (Plan A/B/C each with its own actions + activation thresholds), agreed thresholds instead of our assumptions
- [ ] Critical infrastructure: add dams, data centres, telecoms, irrigation from OSM; dependencies (site X needs power from Y)
- [ ] Multi-level roles: add federal / interstate / global next to municipality, region, civil protection, EU
Map / frontend (offered, not built):
- [x] "Animate this area" (frontend, 1a5322c): /region cells → Simulate water animation; `pop`/`elev` now in cells.geojson (cems-efas-72); still no OBAT buildings
- [ ] Quick option: 3D extrusion of the Live-tab flood layer
- [ ] Road-access map per 1 km cell (road out / bridges / detour / cut off) from `/flood`; "road out?" in "Am I in danger?"; optional per-building view
- [ ] Evacuation routes on the Live map (`evac_sim` → `roads_<run>.geojson`, coloured by cars / congestion); route from the clicked place to safe ground
- [ ] Rail context: OSM lines + stations, nearest dry station per cell (context only, not an escape route)
New projects (cover in the brief as next steps unless time allows):
- [ ] Other hazards: fire, heat, storms (only floods modelled; weather only drives floods + evacuation speed)
- [ ] Alternative supply chains for power, water, internet
- [ ] Fake-news detection + counter-messaging (sanctions = policy, not software)
- [ ] Permanent crisis team in government (governance proposal for the brief)
- [ ] AI layer (notes/09), skipped for now by the user
