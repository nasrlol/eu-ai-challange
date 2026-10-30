# Frontend ↔ API integration (hackathon-e9, 2026-10-29)

Frontend: `AI-Challenge/`, a separate git repo (origin sistahoppet/AI-Challenge).
- **Never commit there.** It is in this repo's `.gitignore`.
- It is a static MapLibre app. Its JS modules are classic scripts, loaded in the order set in `index.html`.
- Today it reads `data/*.geojson` (made by `scripts/export_*.py`). `build.py` turns it into one offline HTML file in `dist/`.

## Status
- **Waiting:** implementation starts once the test campaign in notes/13 is closed. hackathon-41 will signal it.
- 2026-10-29: test campaign closed. Building against the test server `:8081`; live `:8080` is not redeployed until the user OKs it.
- **Built:** a "Live model" tab (`data-t="api"`) and an `API ONLINE/OFFLINE` header badge.
  - New file `js/api.js`. The API address comes from `?api=` or the tab field (localStorage `fse-api`); default is the `:8081` test server.
  - Hooks in other files, one line each: `panels.js` setTab (`'api'` + `apiShow()`), `evac.js` onMapClick (`apiPick(p)` first), `main.js` start-up (`apiInit()`), `index.html` (badge, tab, pane, script after `response.js`), `css/style.css` (`.apiT`, `.apiSteps`, `.err`, `h3` at the end).
  - Tab sections:
    1. Area: study bbox / map view / box drawn with 2 clicks.
    2. `/flood`.
    3. `/evacuation`.
    4. `/timeline` what-if.
    5. `/region`: top 20 priority cells drawn as purple dots.
    6. Help a friend: zone, shelters (dry/flooded dots), helper/person placed on the map, offers → accept → route line + turn-by-turn, nearest dry shelter, close.
    7. `/impact` table.
    8. `/models` + evaluation.
  - Run names are derived from the bbox (`app_<bbox×1000>`, `zone_<…>`), so re-running the same area reuses the same server folder.
- **Tested on `:8081` with curl, for the study bbox 5.6,50.55,5.96,50.64 (255 km²):**

  | Call | Time | Result |
  |---|---|---|
  | `/flood` rp10/100/500 | 10 s | rp100: 20,458 people flooded or cut off, 9,041 leave now, 5,150 buildings flooded |
  | `/evacuation` | 16 s | 03h dry: 14,463 evacuees, p95 3.1 h |
  | `/timeline` whatif | 13 s | – |
  | `/region` | ~2 min | 25,395 cells, 12.3 km² high risk, 6 hospitals |
  | `/help/zones` | 3 min 34 s | 112k nodes, 223 shelters |
  | helper → request → accept → shelter → close | instant | all OK |

  - Pre-built under the app's run names (`zone_5600_50550_5960_50640`, `app_…`), so the study area opens instantly. Test leftovers `e9_vesdre*` and helper #8 are purged after 72 h.
  - Real shapes fixed in the UI:
    - `/region` takes a top-level `bbox`, not `area`. `priority_cells` = lon/lat/risk/people/distances only.
    - `/flood` `top_cells` lat/lon is 0.1° (coarse), but `evacuation.first_cells` is precise → those are what the map draws.
- The browser view is not checked yet: the laptop preview server on :5500 was killed (out of RAM).
- Map base data (depth grid, buildings) still comes from `data/`. The API has no GeoJSON depth-grid endpoint, so the API only drives the Live model tab.
- **Preview** (2026-10-29): `http://squirtle.tail142ce3.ts.net:5500` (laptop, tailnet only). Stopped: killed by the low-memory reaper; restart only when the user asks.
  - Static server: `python3 -m http.server 5500 --bind 100.118.183.51`, run from `AI-Challenge/`.
  - Serves the working copy, so a browser reload shows edits.

## Decisions (user)
| Topic | Choice |
|---|---|
| CORS | `CorsLayer` in `flood_api` (permissive, env `FLOOD_CORS_ORIGINS`). Done by hackathon-56; e9 does not edit flood_api |
| Offline fallback | none: the frontend **requires** the API |
| Scope | `/flood`, `/evacuation`, `/timeline` → Simulate/Evacuation tabs; `/help/*` + shelters → new panel; `/region` from a bbox drawn on the map; `/impact` + `/models` → Policy tab |
| Code | new `js/api.js` (client + base URL setting) and UI changes in the existing modules |

## Open issues / limits
- The API is tailnet-only with no auth, so the jury can't reach it. Public exposure and auth are the user's call.
- Model outputs are mostly gpkg/tif. The browser can only use JSON, CSV and GeoJSON from `/files/...`.
- ~~`evacuation_*.csv` lat/lon rounded to 0.1°~~ fixed 2026-10-29 evening: 4 decimals (~10 m) in danger/evacuation CSVs and `/flood` top_cells.
- Testing rules:
  - Test server is `:8081`. Never call live `:8080`.
  - Heavy POSTs go through `flock flood_api/out/heavy.lock`. Run names are `e9_*`.
  - Browser tests stay on light or cached calls until the campaign ends.

- API changes 2026-10-29 evening (cems-efas-72), all backward compatible for api.js: `/region` adds `top_n`, `by_scenario`, per-cell `factors`/`overall`/`rank`, `region_check`, `sources`, `caveats`; `expected_people_flooded_approx` renamed `risk_weighted_people_index` (api.js does not read it). Slow calls can use `?async=1` and poll `GET /jobs/{id}` (202 running / final body). New DELETE /helpers/{id}, /help-requests/{id}. Live :8080 now has the same build as :8081.

## 2026-10-30 (cems-efas-72): pulled + integrated + hosted
- Pull: origin/main still ec1c687 (no new upstream commits); e9's uncommitted Live-model work kept. Nothing committed or pushed in AI-Challenge/.
- `js/api.js` changes: `apiJob()` (POST `?async=1`, poll `/jobs/{id}` every 3 s, shows elapsed) for /flood, /evacuation, /timeline, /region, /help/zones · /region shows by_scenario table, per-cell overall + high/medium factor reasons, model-fit line (region_check), API caveats · Help: "Delete this helper's data" / "Delete request data" (DELETE routes) · default API = `:8080` when served from valencia.
- Checked: all js parse (deno); api.js call code run in deno against live with the hosted Origin: async /region cold 131 s OK, /flood 449, bad bbox 422, CORS `*` + DELETE. **Not checked in a real browser** (no browser on laptop/valencia).
- Hosted: see notes/12 "Frontend".
- 2026-10-30: Live tab: "Am I in danger?" (click → `/point`; adds model risk of the 100 m cell if an area check is loaded; risk ≥ 0.5 lifts safe → at risk) · Area check draws `cells.geojson` as `api-flood` fill layer, "Show on map" = model risk / 1-in-10/100/500 water / nothing, legend. Redeployed live :8080 + hosted :5500; live layers rebuilt for study bbox + EMSR517 AOI15 ref bbox. Not checked in a real browser.

## Performance (2026-10-30, branch `frontend-perf` in AI-Challenge, commit 017e2f8, pushed)
- Data files fetched in parallel, `cache:'no-cache'` instead of `no-store`; team data loads first, demo only as fallback (no double `computeZones`/`buildWater`)
- `paintDynamic` ~10×/s, stats (`exposure`, classes, `cutKm`) ~5×/s during playback; the last call always lands
- Water canvas `animate:false`: ripples only while playing; paused = still water, map stops redrawing every frame
- Not done: compact grid export (gzip + computeZones done, see below)
- 2026-10-30 later (commit 1a5322c): "Animate this area" in Live model > Area check (/region cells → Simulate; uses `pop`/`elev` when the API sends them, asked cems-efas-72); "Back to the study area"; skewed API cells get their own grid slot; `computeZones` 424 → 135 ms (same output, checked by hash); demo loads first again (grey map was the map waiting for 21 MB); `scripts/serve.py` gzip server (grid 5.4 MB → 324 KB, ETag 304). Not live yet: service still runs `http.server` (systemd change needs the user)

## Forecast tab (2026-10-30, commit 07b149e, `js/forecast.js`)
- GloFAS test, Vesdre at Pepinster (cell 5.825, 50.575): Gumbel 1984–2020 Q10/100/500 = 55/81/100 m³/s; GloFAS 14 Jul 2021 = 153 m³/s (beyond 1-in-500). This is the reanalysis, not the forecast issued at the time
- Model: ridge on log flow; features rain d0/d-1/d-2, 3- and 7-day sums, √R3, antecedent precipitation index (k 0.9, from day −3), R3×API, −ET0 30 d, season; rain/wetness/dryness weights ≥ 0 (otherwise R3×API is negative and wetter ground lowers the flood)
- Python check (trained ≤2014, tested 2015–21): NSE log 0.72; 2021 peak 171 m³/s, 15 Jul (one day late); weeks over 1-in-10: 1/2 caught, 2 false alarms. Gradient boosting fails (2021 → 30 m³/s: trees don't extrapolate)
- Browser: trains per river point (Vesdre gauge in the study area, else the area centre + 4 rain points at ±0.08°), 2 Open-Meteo calls, ~9 s, cached in localStorage `fse-fc1:<lon>,<lat>`. Held-out NSE 2019–25: 0.71. Thresholds include 2021 → 74/121/153
- Sandbox: 3 rain days + 30-day rain + month → 60-day synthetic series → peak flow → T → people/buildings (EFAS blended on log T, = `blend_T`); map uses `planFromRp` (same rule as Alert)
- Now: forecast API past_days 60 + 16 days → model; GloFAS median forecast alongside
- Limits: GloFAS is the target (a model, not gauges); EFAS-type rivers only; 1 point per area; blend gives shallow water over the whole RP10 extent for T 2–10 (people count jumps)

## 2026-10-30: policy advice (commit db7a80c on `frontend-perf`, pushed, live on :5500)
- Live model tab, section 9: the user picks Claude (key field, sessionStorage, or localStorage if "remember"), local model, or rules → `js/policy.js` → policy API :8082 (`?policy=` override).
- `api.js` keeps the last result of each run in `A.res` (`keepRes`), plus a `policyInit()` hook in `apiInit`. Tested in deno with a stub DOM; **not checked in a real browser**.
