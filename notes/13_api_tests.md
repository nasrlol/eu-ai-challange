# API test campaign (2026-10-29)

Goal: every feature in notes/08, 11, 12 and code/NOTES.md is reachable and correct through the Rust API (`flood_api`).
Coordinator: session hackathon-41 (only it edits this file). Test server: moving from laptop `localhost:8081` (out of RAM) to a valencia test slot via `code/testrun.sh` (never touch live :8080).
Rule: every heavy call goes through `flock /home/nasr/hackathon/flood_api/out/heavy.lock curl ...` (one at a time, all sessions).

| Area | Endpoints | Tester | Status |
|---|---|---|---|
| Build + /evacuation route + test server, Rust fixes | `POST /evacuation`, server on 8081 | hackathon-56 | 8081 up, /evacuation live; on call for Rust fixes |
| Python fixes, CLI vs API parity | `code/*.py` | cems-efas-da | CLI references in `code/outputs/eval/api_reference.json` (bbox 7.05,50.52,7.12,50.56; ints exact, floats ±0.5 %); on call for Python fixes |
| Region | `GET /health`, `POST /region` | hackathon-12 | done on 8081: 15 PASS, 4 FAIL (all B8/B9); too-large test on hold until B8 + valencia |
| Help a friend + shelters | `/help/*`, `/helpers*`, `/help-requests*` | hackathon-11 | done on 8081: ~40 PASS, 3 FAIL (B2, B5); persistence to test on valencia |
| Models I | `GET /models`, `POST /flood`, `POST /timeline`, `GET /impact` | hackathon-77 (was 22) | waiting for valencia test server |
| Resources: test server on valencia (`code/testrun.sh`), heavy-call lock | — | hackathon-22 | slot1 `srv` building on valencia (16:00, first build compiles deps) → `http://valencia.tail142ce3.ts.net:8081` |
| Models II + doc audit | `GET /roads/:aoi`, `GET /interp/:aoi`, `POST /evacuation`; notes ↔ API gaps | cems-efas-8c | roads/interp 60/70 (B1); /evacuation 25 runs: parity + plausibility PASS, E1–E5; congestion case + retests on valencia |
| Hosted | all, on valencia | hackathon-77 | ready (data + deps on valencia, runs tree ~15:45 without /evacuation); redeploy after local pass |

## Bugs / gaps
| # | Endpoint | Problem | Owner | Status |
|---|---|---|---|---|
| B1 | `/roads/:aoi`, `/interp/:aoi` | prefix match (`starts_with`): `EMSR517_AOI1`, `EMSR517`, `E` → 200 with mixed AOIs | hackathon-56 | fixed (8081), retest on valencia |
| G1 | all model routes | responses give absolute server paths; no route serves viewer.html / gpkg / tif / pdf → add `GET /files/...` | hackathon-56 | fixed (8081), retest on valencia |
| G2 | `/flood` | `evacuation_<flood>.csv` ranking not in `files` | hackathon-56 | fixed (8081), retest on valencia |
| B2 | `POST /help-requests`, `POST /helpers` | points outside zone accepted (request 200 km away gets offers); `snap()` has no max distance | hackathon-56 | queued |
| B3 | help routing | `eta_s` includes ×20 flood penalty → unrealistic times; nearby walker got no offer | hackathon-56 | queued |
| B4 | help ids/zone | unknown id → 422 not 404 (NotFound→404 added in models.rs); unknown zone leaks raw SQL error | hackathon-56 | queued |
| B5 | `POST /help-requests/:id/accept` | busy helper can accept a 2nd request; close frees helper while another request still assigned | hackathon-56 | queued |
| B6 | `GET /help-requests/:id/shelter` | defaults to rp100, not the request's scenario | hackathon-56 | queued |
| B7 | `POST /help/zones` (rebuild) | possible FK failure / helper loss on re-POST (untested) | hackathon-56 | to check |
| B8 | `POST /region` | validation after heavy build: size check late, no hull check, inverted bbox normalised, zero-area bbox → 36 s + empty result, outside-hull bbox accepted (200), lat/lon swap → misleading 'no rivers' after 36 s | hackathon-56 | fixed (8081), retest on valencia |
| B9 | `POST /region` | facility counts include 0.02° pad (triangle reports facilities outside it) | hackathon-56 | fixed (8081), retest on valencia |
| G3 | `POST /region` | output paths still FLOOD_OUT, not `/files/...` | hackathon-56 | fixed (8081), retest on valencia |
| E1 | `POST /evacuation` | `flood=observed` on bbox/polygon → silent 200 with 0 flooded | cems-efas-da | fixed (CLI exit 1 + message; access_model.flood_mask guard), retest on valencia |
| E2 | `/evacuation` (and /flood, /impact?) | unknown/suffix-less AOI → 422 with raw Python traceback | hackathon-56 | queued |
| E3 | `/evacuation` | `T=inf` accepted as RP500 | hackathon-56 | queued |
| E4 | `/evacuation` | duplicate hours/weather → colliding run names | hackathon-56 | queued |
| E5 | all JSON routes | serde rejections return plain text, not `{"error"}` | hackathon-56 | queued |

## Gaps from audit (cems-efas-8c), not built, waiting on user
- RegionResponse/caveats wiring, per-cell factors, ScenarioSummary/top_n, OOD check in /region, rename `expected_people_flooded_approx`, GDACS input (notes 09/10)
- AI layer (notes 09); € saved / lives saved (10 §2b); rescue allocation risk × travel time (06); live road-closure mode
- Minor/offline: per-AOI risk outputs, model v2, eval details, PDF regenerate, arbitrary-region road exposure, GHS-POP 2025
