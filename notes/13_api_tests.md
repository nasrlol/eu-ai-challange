# API test campaign (2026-10-29), CLOSED 16:30

**Result:** all routes tested on valencia test build (commit c7742c0). 23 bugs/gaps found, 22 fixed + verified, 1 known limit (B11). Next: live :8080 redeploy (hackathon-77) — waiting for the user's explicit OK — then frontend wiring (hackathon-e9, notes/15).

Goal: every feature in notes/08, 11, 12 and code/NOTES.md is reachable and correct through the Rust API (`flood_api`).
Coordinator: session hackathon-41 owns the tables above "## Open decisions (user)
- Hosting: tailnet-only vs public; public needs auth (help routes hold location/need data), restricted `FLOOD_CORS_ORIGINS`, HTTPS. Frontend (hackathon-e9) will require the API.
- Public repo: e6902b5 already pushed notes/12 + old 13_test_resources (valencia hostname/IP); notes/12/13/14 held back.
- Build unbuilt features from the gap audit? (fuller /region RegionResponse, AI layer, € saved, rescue allocation)

## Coverage check (16:22)
- Every bug ID B1–B11, G1–G6, E1–E6 appears in this file; fixes documented in notes/08 (Rust API), 11 (help), code/NOTES.md (Python, steps 17–18).
- Sessions: results sections for cems-efas-da, hackathon-12, -11, cems-efas-8c, -77; hackathon-56 work in notes/08 + 11; hackathon-22 in notes/14; hackathon-e9 in notes/15; hosting in notes/12.

## Results"; each tester only appends to its own `### <session>` section under "## Results" (small Edits, no rewrites). Test server: `http://valencia.tail142ce3.ts.net:8081` (testrun slot `srv`, restarts via hackathon-22; never touch live :8080). Laptop 8081 retired (out of RAM).
Rule: every heavy call goes through `flock flood_api/out/heavy.lock curl ...` (from the repo root) (one at a time, all sessions).

| Area | Endpoints | Tester | Status |
|---|---|---|---|
| Build + /evacuation route + test server, Rust fixes | `POST /evacuation`, server on 8081 | hackathon-56 | all Rust bugs fixed locally; laptop 8081 stopped; on call |
| Python fixes, CLI vs API parity | `code/*.py` | cems-efas-da | CLI references in `code/outputs/eval/api_reference.json` (bbox 7.05,50.52,7.12,50.56; ints exact, floats ±0.5 %); on call for Python fixes |
| Region | `GET /health`, `POST /region` | hackathon-12 | **done**: all /region FAILs pass on valencia (see Results) |
| Help a friend + shelters | `/help/*`, `/helpers*`, `/help-requests*` | hackathon-11 | **done**: B2–B7, B10 verified; persistence across restart PASS |
| Models I | `GET /models`, `POST /flood`, `POST /timeline`, `GET /impact` | hackathon-77 (was 22) | **done**: 72/74 PASS (/models, /flood all options + G4, /timeline peak = /flood, /impact, 14 error cases); found E6, B11 |
| Resources: test server on valencia (`code/testrun.sh`), heavy-call lock | — | hackathon-22 | valencia test server up (restart 16:07: all Rust fixes B1–B9, G1–G3, E1–E5) |
| Models II + doc audit | `GET /roads/:aoi`, `GET /interp/:aoi`, `POST /evacuation`; notes ↔ API gaps | cems-efas-8c | **done**: B1/G1/G2/E1–E5 verified, parity + congestion (EMSR518 AOI01, max v/c 2.59) PASS; found G6 |
| Frontend integration (after campaign) | `AI-Challenge/` (separate repo, gitignored here) | hackathon-e9 | waiting for campaign close |
| Hosted | all, on valencia | hackathon-77 | ready (data + deps on valencia, runs tree ~15:45 without /evacuation); redeploy after local pass |

## Bugs / gaps
| # | Endpoint | Problem | Owner | Status |
|---|---|---|---|---|
| B1 | `/roads/:aoi`, `/interp/:aoi` | prefix match (`starts_with`): `EMSR517_AOI1`, `EMSR517`, `E` → 200 with mixed AOIs | hackathon-56 | **verified on valencia** |
| G1 | all model routes | responses give absolute server paths; no route serves viewer.html / gpkg / tif / pdf → add `GET /files/...` | hackathon-56 | **verified on valencia** |
| G2 | `/flood` | `evacuation_<flood>.csv` ranking not in `files` | hackathon-56 | **verified on valencia** |
| B2 | `POST /help-requests`, `POST /helpers` | points outside zone accepted (request 200 km away gets offers); `snap()` has no max distance | hackathon-56 | **verified on valencia** |
| B3 | help routing | `eta_s` includes ×20 flood penalty → unrealistic times; nearby walker got no offer | hackathon-56 | **verified on valencia** |
| B4 | help ids/zone | unknown id → 422 not 404 (NotFound→404 added in models.rs); unknown zone leaks raw SQL error | hackathon-56 | **verified on valencia** |
| B5 | `POST /help-requests/:id/accept` | busy helper can accept a 2nd request; close frees helper while another request still assigned | hackathon-56 | **verified on valencia** |
| B6 | `GET /help-requests/:id/shelter` | defaults to rp100, not the request's scenario | hackathon-56 | **verified on valencia** |
| B7 | `POST /help/zones` (rebuild) | possible FK failure / helper loss on re-POST (untested) | hackathon-56 | **verified on valencia** |
| B8 | `POST /region` | validation after heavy build: size check late, no hull check, inverted bbox normalised, zero-area bbox → 36 s + empty result, outside-hull bbox accepted (200), lat/lon swap → misleading 'no rivers' after 36 s | hackathon-56 | **verified on valencia** |
| B9 | `POST /region` | facility counts include 0.02° pad (triangle reports facilities outside it) | hackathon-56 | **verified on valencia** |
| G3 | `POST /region` | output paths still FLOOD_OUT, not `/files/...` | hackathon-56 | **verified on valencia** |
| E1 | `POST /evacuation` | `flood=observed` on bbox/polygon → silent 200 with 0 flooded | cems-efas-da | **verified on valencia** |
| E2 | `/evacuation` (and /flood, /impact?) | unknown/suffix-less AOI → 422 with raw Python traceback | hackathon-56 | **verified on valencia** |
| E3 | `/evacuation` | `T=inf` accepted as RP500 | hackathon-56 | **verified on valencia** |
| E4 | `/evacuation` | duplicate hours/weather → colliding run names | hackathon-56 | **verified on valencia** |
| E5 | all JSON routes | serde rejections return plain text, not `{"error"}` | hackathon-56 | **verified on valencia** |
| G4 | `POST /flood` | per-flood summary lacks buildings_flooded / buildings_cut_off (only in timeline steps.csv) | cems-efas-da | **verified on valencia** |
| G5 | all | no CORS → browser frontend (AI-Challenge/, hackathon-e9) blocked; add CorsLayer, `FLOOD_CORS_ORIGINS` | hackathon-56 | **verified on valencia** |
| B10 | `POST /help/zones` | empty zone name accepted; zone bbox not run through region_check (bad zone fails only after ~2 min build) | hackathon-56 | **verified on valencia** |
| G6 | `POST /flood` | reusing a name with other floods lists stale danger_/evacuation_ CSVs (glob, not req.floods) | hackathon-56 | **verified on valencia** |
| E6 | `POST /impact` | no validation of cost_eur_m2 / max_depth (−5 → negative €) | hackathon-56 | **verified on valencia** |
| B11 | `POST /flood` risk>=0.5 | buildings_flooded 1476 on valencia vs ref 1475 (laptop); threshold/platform float? | cems-efas-da | **known limit**: deterministic platform effect (pixel-edge floor after pyproj, x86 vs ARM); tolerance ±1 on risk>=t counts |
## Gaps from audit (cems-efas-8c), not built, waiting on user
- RegionResponse/caveats wiring, per-cell factors, ScenarioSummary/top_n, OOD check in /region, rename `expected_people_flooded_approx`, GDACS input (notes 09/10)
- AI layer (notes 09); € saved / lives saved (10 §2b); rescue allocation risk × travel time (06); live road-closure mode
- Minor/offline: per-AOI risk outputs, model v2, eval details, PDF regenerate, arbitrary-region road exposure, GHS-POP 2025

## Results

### cems-efas-da (Python owner)
- CLI references for parity: `code/outputs/eval/api_reference.json`; bbox 7.05,50.52,7.12,50.56 (EMSR517 AOI15, 22.1 km², 5,023 buildings); runs `ref_flood`, `ref_timeline`, `ref_evac` (do not overwrite).
- Key reference values: /flood rp100 449 flooded / 522 cut off, evac now 710 / early 1,102; /timeline whatif peak 100 (H+12) 0.91 km², 22.94 km roads closed; /impact EMSR518 AOI02 38 buildings, €3.25 M; /evacuation 03h dry 1,263 evacuees / 427 cars / p95 3.25 h.
- Internal consistency PASS: timeline peak T=100 = /flood rp100 = /evacuation building counts (449 / 522).
- E1 fixed in `evac_sim.py` + `access_model.flood_mask` guard; CLI check PASS (bbox + observed → exit 1; AOI + observed works; reference unchanged).
- G4 fixed in `flood_tool.py` (new keys buildings, buildings_flooded, buildings_cut_off, roads_closed_km, road_km); CLI check PASS (ref_flood rp100 5,023 / 449 / 522 / 22.94 km).
- B11 diagnosed: local reruns deterministic (1,475 twice, identical scores); no building within 1e-6 of 0.5 (nearest 0.500034 / 0.499847), so it is a platform effect (pixel-edge floor after pyproj, x86_64 vs aarch64), not nondeterminism. Accepted by hackathon-41 as known limit: tolerance ±1 (±0.1 %) on `risk>=t` building counts; no code change.

### hackathon-12 (`/health`, `/region`)
- Laptop 8081 (old build): 15 PASS, 4 FAIL (inverted bbox 200, zero-area bbox 200 after 36 s, no hull check, facility counts incl. 0.02° pad) → B8/B9/G3.
- Valencia 8081 (B8/B9/G3 build), retests all PASS, rejected in ~0.06 s, before any build:
  | Input | Got |
  |---|---|
  | inverted / zero-area bbox | 422 "bbox must be [west, south, east, north]…" |
  | outside hull [4.30,51.20,4.36,51.24] / lat-lon swapped | 422 "100% of the region lies outside the study region" |
  | too large [6.0,50.3,7.2,50.9] | 422 "region is 5670 km²; split…" |
  | polygon as Feature | 200 (was 422) |
  | triangle in EMSR517_AOI15 | 948 cells; `facilities` 6 inside + `facilities_nearby_outside` 58; per-feature `inside` flag checked by geometry |
  | `/files/api_region/test12_poly_tri/*` | 200 geojson/csv (948 rows); `..` traversal and missing file → 404 |
- Valencia vs laptop, bbox [7.09,50.51,7.15,50.55]: same 1,895 cells and 64 facilities (now split 49 inside / 15 outside). OSM cache: cold call 77–111 s on valencia, cached 0.4 s (laptop 36.5 s → 0.05 s).
- `dry` flag: N/A for /region (only in help shelters).
- Open: lat/lon swap reported as "outside study region", not "coordinates out of range" (acceptable).

### hackathon-11 (Help a friend + shelters)
- Laptop 8081 (old build), zone `test11_EMSR517_AOI15` (806 km², 273,007 nodes, 7,810 km roads, 164 shelters, build ~30 s): ~40 PASS, 3 FAIL → B2, B5; observations → B3, B4, B6.
- Valencia 8081 (fix build), B2–B7 all PASS:
  | Bug | Check | Got |
  |---|---|---|
  | B2 | helper (20, 10) / request (4.35, 50.85) | 422 "… outside the zone [bbox]" |
  | B3 | request at 0.49 m RP100 cell | walker 259 m: eta 187 s, cost_s 1,120 → rank 1; dry point eta == cost |
  | B4 | unknown request (get/accept/close/shelter), helper, zone | 404 ×6 |
  | B5 | busy helper accepts 2nd request | 409 "helper 2 is busy …" |
  | B6 | shelter for rp10 request | scenario rp10 (Ahrtalschule, 1,785 m walk, 0 m flooded); `?scenario=` overrides |
  | B7 | zone re-POST (2 min 10 s on valencia) | R1–R4 identical, helper 2 still 409 |
- Shelters AOI15: dry 142 / 134 / 129 of 164 at rp10 / rp100 / rp500.
- Persistence: not yet tested (unannounced restart killed the first zone build, nothing written). Seed waiting for the announced restart: R1 assigned (helper 2), R2 open rp10, R3 assigned (helper 7), R4 escalated.
- Open: cost_s 1,120 for 187 m flooded of 259 m walk looks below ×20 (expected ~2,700 s); stray zone `""` on valencia from a quoting error in my wrapper (no delete endpoint).
- Update: cost_s is as intended (B3 = +5 s per flooded m: 187 + 187×5 ≈ 1,120 s, notes/11). Stray `""` zone → B10, row deleted at the announced restart; after it: check persistence of R1–R4 and retest B10 (empty name → 422).

### cems-efas-8c (Models II: `/roads`, `/interp`, `/evacuation`, `/files`; gap audit)
- Gap audit (notes + code/NOTES.md vs routes) → G1, G2 and "Gaps from audit" above.
- Laptop 8081: `/roads` + `/interp`, 19 AOIs (notes/02 + 03) + `_GRA`/`_DEL` names: row counts = `code/outputs/roads|interp` CSVs; 10 FAIL → B1 (prefix match). `/evacuation`: 25 valid runs + 23 bad inputs → E1–E5.
- Valencia 8081 (fix builds): B1, E1–E5, G1, G2 PASS.
  | Check | Got |
  |---|---|
  | B1 partial codes (EMSR517_AOI1, EMSR517, E …) | 422; EMSR517 AOI18 / unknown → 404 |
  | E1–E5 | 422 observed on bbox · 404/422 clean AOI errors · 422 T=inf · dup hours → 1 run · serde errors as `{"error"}` (400 for broken JSON) |
  | G1 `/files` | viewer.html, gpkg, pdf, json 200 with correct content types; 11 traversal variants → 404; `GET /files/` root → 404 (subfolders list) |
  | Parity vs `api_reference.json` (bbox 7.05–7.12 E, 50.52–50.56 N, rp100, roads none, 3/17 h × dry/heavy_rain) | identical: 449 flooded / 522 cut off; 03h_dry 1,263.49 evacuees, p95 3.25 h; 17h_dry 864.49 / 2.31 h; heavy rain 3.31 / 2.39 h |
  | Plausibility (5 weathers × 4 hours, weekend, roads_flood none→rp500) | p95 03h: dry 3.25 < rain 3.27 < fog 3.30 < heavy_rain 3.31 < snow 3.37; weekend 08h 1,197 evacuees vs weekday 931; boat/air 6.5 → 693 → 972 → 984 |
  | Congestion `test8c_cong_EMSR518_AOI01` (5.55–5.95 E, 50.55–50.66 N, 97.5 % in EMSR518 AOI01; rp100, roads none) | 25 s; 173,031 bldgs; 03h dry 64,430 evacuees, 0.49 km congested, p95 6.0 h; 08h dry 3.41 km / 6.22 h; 08h heavy_rain 5.61 km / 7.36 h; max v/c 2.59; = code/NOTES.md §17 |
- New, reported to hackathon-41: `/flood` same name twice (rp100, then rp10) lists the stale `danger_/evacuation_rp100.csv` too (handler globs the folder).
- Not covered: weekend × congestion; `/flood` and `/timeline` belong to Models I.
- Final build: G6 PASS (`/flood` test8c_g6 rp100 then rp10 → 2nd lists only rp10 in danger/evacuation/floods). CORS (G5) PASS: OPTIONS preflight on `/help-requests` POST, `/helpers/1` PATCH, `/files/…` GET, `/evacuation` POST → 200, `access-control-allow-origin: *`, methods GET,POST,PATCH, header content-type; plain GET with Origin also gets allow-origin; DELETE not in allowed methods.

### hackathon-77 (Models I: `/models`, `/flood`, `/timeline`, `/impact`; valencia live :8080)
- Script `test77.py` (scratchpad), 74 checks, run at ~16:40 against :8081 (G4 hot-synced, before the B10/CORS restart). Result: **72/74 PASS** (4 `/files` rows the script flagged were a script bug; all returned 200).

| Check | Result |
|---|---|
| `/flood` ref bbox rp100: buildings 5,023 / flooded 449 / cut off 522 / roads 22.94 of 292.26 km, cells C/H/E/L 2/2/6/14, people 1,812, evacuation now/early/air/prepare 710/1,102/1,222/199 | exact |
| `/flood` risk>=0.5: cells, people 8,643, cut off 2,682, roads 59.92 km | exact |
| `/flood` risk>=0.5 buildings_flooded | **FAIL 1,476 vs 1,475** (off by one; threshold edge or ref from another run) |
| `/flood` rp10+rp500+risk>=0.8, observed on EMSR518 AOI02 DEL, allow_outside false→4xx / true→200 | PASS; people rp10 ≤ rp100 ≤ rp500 |
| `/timeline` whatif 100/12/24/6 | 7 steps, T 2/14/100/38/14/5.3/2, peak H+12 = `/flood` rp100 (449/522), people exact |
| `/timeline` series driver (2/100/10) | step t1 = `/flood` rp100 |
| `/impact` EMSR518 AOI02 DEL defaults | 38 flooded, €3.255 M, 91.04 displaced, all 8 fields exact; GET row matches |
| `/impact` cost 3000 / max_depth 1 | loss ×2, displaced unchanged / loss lower |
| `/impact` cost_eur_m2 -5 | **FAIL: 200, loss -0.011 M €** (no validation of cost/max_depth) |
| 14 error cases (bad flood, risk>=1.5, observed on bbox, `../x` name, unknown AOI, >3,000 km², bad bbox, peak_rp 1, step_h 0, bad date, bogus driver, impact unknown/`../../etc`) | all 4xx + JSON error |
| `/files` links from `/flood` + traversal (`../..`, `%2e%2e`, missing file) | 200 / 404 |
- Retest on final :8081 build (after B10/CORS): E6 `/impact` cost -5 and max_depth 0 → 422 "must be positive"; cost 2000 → €4.34 M (= 3.255 × 2000/1500), displaced 91.04 unchanged. `/files` folder + summary.json / risk.tif / access_danger.gpkg → 200 with correct content types. risk>=0.5 off-by-one = B11 (±1 accepted). **hackathon-77: all PASS.**
- After the 16:20 announced restart, all PASS:
  | Check | Got |
  |---|---|
  | Persistence R1–R4 | status, offers and accepted_at identical before/after |
  | Busy helpers | helper 2 → 409 (0 s, before graph build); helper 7 excluded from new R5 offers |
  | Lazy graph rebuild | first graph call 128 s (under flock), then 0 s |
  | B10 zone name `""`, `bad name!`, `test11_../x` | 422 "name must be letters, digits, _ or -" in 0.07 s |
  | Inverted / 5,670 km² / outside-hull bbox | 422 in 0.07 s |
  | Stray `""` zone | gone (404) |

### cems-efas-72 (follow-up after close, 2026-10-29 evening)
- New build on :8081 (21:46, 21:57) then live :8080: /region v2 (notes/08), `?async=1` + `/jobs`, DELETE helpers/requests, CSV lon/lat 4 decimals.
  | Check | Got |
  |---|---|
  | /region async bbox [7.09,50.51,7.15,50.55] | 202 → 200 in 10 s; 1,895 cells, facilities 49 inside / 15 outside (= hackathon-12) |
  | npy vs CSV handoff | features max diff 6e-5 (f32 vs decimal), risk identical (0.0), 189 high-risk cells both |
  | region_check | 5.6 % cells outside training range (hand 4.1 %), nearest AOI EMSR517_AOI03_DEL (inside), AUC 0.964 |
  | /flood ref bbox rp100 | 5,023 / 449 / 522 / 22.94 km = reference; evacuation CSV lon/lat 4 decimals |
  | DELETE helper with accepted request | request back to `open` (`requests_reopened` 1); DELETE request → 404 after; unknown ids 404 |
  | top_n 900, region aoi, inverted bbox | 422 |
- **B12 (found + fixed)**: `/flood`, `/timeline`, `/evacuation` accepted an inverted bbox (Python reorders it silently, 200 + full run). Now 422 in Rust (`region_arg`), verified on :8081 and :8080. Leftover `x` run folders removed (live + laptop).
- Known limit: SQLite reuses a deleted helper's id (9 reissued) → stale clients could address a new record; needs AUTOINCREMENT migration.
