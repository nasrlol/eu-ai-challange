# Frontend handoff: new API features (2026-10-30)

For the AI-Challenge frontend. Base `https://dev.nsrddyn.com/api`. Everything is **additive**: no route or field was renamed or removed; old requests give the same answers. Answers page with examples: https://claude.ai/artifact/XAyaJhGy4513b1Y4kuAafA

## What to build in the app (by priority)

| # | Feature | API | Frontend work |
|---|---|---|---|
| 1 | **Water per 100 m cell over time** | `POST /timeline` → `files.cells_geojson` | Load once per run. Each feature has `id` + `d` (depth m per step, same order as `steps`). Colour by `d[step]` on the slider. The file's top-level `steps` lists `{step, time, T}`. |
| 2 | Join with /region cells | `id` = `"<x>_<y>"` (EPSG:3035 m) in both `/region` `cells.geojson` and `/timeline` `cells.geojson` | For the same bbox, the ids match: use `/region` for risk (`r`), `pop`, `elev`, and `/timeline` for depth in time. |
| 3 | **Real July 2021 / GloFAS** | `driver: {"type":"open_meteo","point":[lon,lat],"start":"2021-07-10","end":"2021-07-20"}` | Add a driver choice "July 2021 (GloFAS)" next to what-if. Steps are daily; `discharge` is m³/s. Show "T above 500 = held at the 1-in-500 map". |
| 4 | Own gauge series | `driver: {"type":"series","q10":120,"q100":300,"steps":[{"time":"…","discharge":40}, …]}` | Optional. Old form `{"time","return_period"}` still works. Every step needs the same kind of value. |
| 5 | **Demo: preload + reuse** | add `"reuse": true` to any POST of `/region`, `/flood`, `/timeline`, `/evacuation`, `/layers` | Same `name` + same body → stored answer at once, with `"reused": true` and header `x-reused: 1`, no model run, no wait. Different body → runs again. Preload every demo area before the demo with `?async=1`, then send the same requests with `"reuse": true`. |
| 6 | **Buildings, towns, shelters** | `POST /layers {"name","region"}` → `files.buildings`, `files.places`, `files.shelters` (GeoJSON points) | Draw buildings (props below), town labels from `places` (sorted by population), shelter pins sized by `cap`. Max 300,000 buildings per request (Vesdre study box ≈ 173k: OK). |
| 7 | AOI by code in /region | `"region": {"type":"aoi","aoi":"EMSR517_AOI06_DEL"}` | Use the AOI outline instead of a drawn box (never "outside the study region"). Suffix may be left out when unique. |
| 8 | Study region outline | `GET /files/region/study_region.geojson` | Draw where the models work; features `kind` = `study_region` / `aoi` (`name` = AOI code). Block or warn on boxes outside it. |
| 9 | Car routes | `POST /evacuation` → `files.roads_geojson[<HHh_weather>]`, `files.cells_geojson[<HHh_weather>]` | Lines with `cars`, `vc` (load), `level` (free/busy/congested/overloaded), `clear_h`; only stretches with ≥ a few cars. |
| 10 | 2021 roads + in-between extents | `GET /roads/{aoi}` → `files.roads_geojson{<AOI>: url}`; `GET /interp/{aoi}` → `files.extents_geojson{<pair>: url}` | Optional "2021 replay" layer: roads with `observed/rp10/rp100/rp500` flags; extents = one polygon per 6 h step (`time`, `hours`). |

Status: tested on the valencia test build (:8091, branch `frontend-asks`), all checks pass except **`/layers`, not tested yet** (its first call builds the OSM places/shelters cache, a few minutes). **Not live yet**: dev.nsrddyn.com gets it after the PR is merged and deployed.

Fixed on the server (no frontend work): `/region` "corrupt deflate stream" for boxes touching empty EFAS tiles (e.g. [5.31, 50.13, 5.58, 50.40]) now works.

## Field reference
- `/timeline` `cells.geojson` feature: `{"id":"3981650_3071250","d":[0,0.12,0.48,…]}`; only cells that get wet in some step; DEM-checked depth (as `flooded_km2`), so it can differ a little from `/region` `d10/d100/d500` (raw EFAS).
- `/layers` `buildings`: `h` height m, `fl` floors, `use` (GHS-OBAT: 1 residential, 2 non-residential, see the OBAT PDF), `a` footprint m², `pop` people, `d10/d100/d500` depth m at the building.
- `/layers` `places`: `name`, `place` (city/town/village/suburb/hamlet), `population` (null when not in OSM).
- `/layers` `shelters`: `name`, `kind` (`shelter` / `candidate_shelter`), `tag` (school, sports_hall, …), `floor_m2`, `cap` (estimated places = floor area × 0.5 / 3.5 m² per person; null when no GHS-OBAT building matches). Label `cap` as "estimated".
- `/layers` summary: `buildings`, `people`, `buildings_wet{rp10,rp100,rp500}` (≥ 0.3 m), `places`, `shelters`, `shelter_places_est`, `assumptions`, `caveats`.

## Demo limits (unchanged)
- nginx: 10 model runs/min per IP on `/region`, `/flood`, `/evacuation`, `/timeline`, `/help/zones` (429 beyond; `/layers` still only under the general 20 req/s limit until nginx is updated); 300 s per synchronous request (504) → use `?async=1` + `GET /jobs/{id}` for anything big.
- Reused answers and `GET /files/…` are instant. Async job results are lost on an API restart (each deploy restarts it); reuse files are not.

## Depth between the EFAS maps (for your own maths)
T ≤ 2 → 0; 2–10: `d10·w`, w = ln(T/2)/ln 5; 10–100: `d10 + (d100−d10)·w`, w = ln(T/10)/ln 10; 100–500: `d100 + (d500−d100)·w`, w = ln(T/100)/ln 5; T > 500 → `d500`.
