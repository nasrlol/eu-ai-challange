# Project notes: July 2021 flood impact analysis

European AI Challenge 2026. Working log of everything done with Claude Code,
including the exact prompts used (for the AI sheet, Annex 7). Last updated 29 Oct 2026.

**Setup:** local environment (Gentoo Linux, aarch64), Python venv at `~/hackathon/.venv`,
data under `~/hackathon/working_data/`, code under `~/hackathon/code/`.

**Earlier work (before this log, not created here):** research notes in `~/hackathon/notes/`
(01 datasets, 02 EMS AOIs, 03 preprocessing, 04 modelling plan, 05 external data, 06 rescue APIs,
07 regional help sources), `code/preprocess.py`, and `~/hackathon/processed/` (`ems_clean.gpkg`,
`ems_summary.csv`, `ems_layer_counts.csv`).

**Installed during this log:** micromamba + QGIS 4.2.3 (separate env), and into the venv via `uv`:
`reportlab`, `scikit-learn`, `scipy`.

---

## 1. Understanding the CEMS data

> **Prompt:** "/home/nasr/hackathon/working_data/ems_be what are these vector folders? no need to open them indiviually they are large"

- `ems_*` folders are **Copernicus EMS Rapid Mapping** products: EMSR517 (Germany), EMSR518 (Belgium), EMSR520 (Netherlands).
- Folder name `EMSR518_AOI01_DEL_PRODUCT_r1_...`: activation, area of interest, map type (DEL = flood delineation, GRA = damage grading), PRODUCT = first map, MONIT01 = first monitoring update.
- Each layer comes in 5 formats (shp, json, kmz, sld, xml). Key layer: **`observedEventA`** = flood extent polygons. Others: AOI outline, image footprints, built-up, roads, rivers, land use, summary table.

## 2. Installing QGIS

> **Prompt:** "i heared that conda can install the qcis binary"
> **Prompt:** "install it using the fastest way possible"
> **Prompt:** *(pasted QGIS error: "Couldn't load SIP module ... No module named 'PyQt6'")*

- Installed standalone `micromamba` (`~/.local/bin`) and QGIS 4.2.3 from conda-forge into `~/micromamba/envs/qgis`.
- Error cause: the hackathon venv leaked into QGIS's embedded Python. Fixed with a launcher `~/.local/bin/qgis` that isolates it. Launch with `qgis`.
- PDAL `libhdf5_cpp` warnings at startup are harmless.

## 3. First analysis script (`main.py`)

> **Prompt:** *(pasted traceback: "DataSourceError: ems_de/...observedEventA_r1_v1.shp: No such file or directory")*
> **Prompt:** "make the script take file arguemnts and giev the building stuff if availabele or general features of the data."

- Bug: data paths were relative to `working_data/` but the script ran from `code/`. Fixed with a `DATA` path constant.
- `main.py <flood file or folder> [...]` now prints a general summary (CRS, features, area, bounds, attributes), then flooded buildings (GHS-OBAT, auto-detected per country), RP100 depth at those buildings, and population.
- Check: EMSR517 AOI01 gives 70 flooded buildings because the flood polygons there cover only 0.28 km², not because of a CRS problem.

## 4. Impact model chain (`pipeline.py`)

> **Prompt:** "can we make a model pipeline? one model simulates the buidlings for ewxample and then passes the results to another model and that simlutates further?"

`hazard → exposure → vulnerability → loss → people`. Each stage is a function that adds to a shared state:

| Stage | Method |
|---|---|
| hazard | CEMS observed flood extent |
| exposure | GHS-OBAT buildings inside the extent; depth sampled from EFAS RP100 map (cap 6 m) |
| vulnerability | JRC Europe depth-damage curves (Huizinga et al. 2017), residential / commercial |
| loss | damage fraction × floor area × €1,500/m² (placeholder) |
| people | occupants = residential floor area / 40 m²; displaced if depth ≥ 0.5 m |

**Assumptions to verify:** OBAT `use` codes (1 residential, 2 non-residential, 0 unknown), €/m² cost, 3 m storey, 40 m²/person.
**Weak link:** RP100 depth is a scenario, not the 2021 event depth (saturates at 6 m in some AOIs, zero in others).

## 5. PDF summary (side track, `report.py`)

> **Prompt:** "convert the preprocessed data to a pdf summary"

- `outputs/report/`: `flood_summary.pdf`, `flood_summary.csv`, `maps/`; pipeline run over all 19 PRODUCT AOIs.
- Totals: 96.1 km² flooded, 9,742 buildings, about €722 M direct damage, about 13,262 displaced (model estimates).
- Two German GRA areas (AOI15, AOI18) dominate. AOI08 shows €0 because the depth map has no data there.
- Regenerate with `python3 report.py [-o out.pdf] [--cost 1500] [--max-depth 6]` (a few minutes).

> **Prompt:** "okay wait i kind of forgot where we were and we've done. besides the pdf that was a side trrack"

*(Recap only.)*

## 6. Flood susceptibility ML model (`risk_model.py`)

> **Prompt:** "no firs step make a basic ml model that can predict the risk of the region based on the relevat geographical factors close to the flood areas"

- **Data:** 3,000 flooded + 3,000 dry random points per AOI (19 AOIs), labelled with the CEMS extent. Permanent water excluded.
- **Features:** elevation, slope, local relief (height above lowest point within ~1 km), height above nearest river (HAND-like), distance to river, EFAS RP10/RP100/RP500 depths.
- **External data:** Copernicus DEM GLO-30 (AWS open data), 9 one-degree tiles (N49–N51, E004–E007, 298 MB) downloaded automatically into `working_data/dem/`. This is **not challenge data; cite it.**
- **Model:** scikit-learn HistGradientBoostingClassifier. **Validation:** leave-one-area-out.
- **Results:** median held-out ROC AUC **0.926**, vs 0.820 for the EFAS RP100 map alone and 0.777 for height-above-river alone. Strong on river-valley floods (0.90–0.99), weak on urban GRA areas (0.63–0.72), fails on AOI20 (0.45, almost no flood).
- **Most important features:** local relief > elevation > distance to river.
- **Outputs (`code/outputs/risk/`):** `risk_<area>.tif` (100 m maps, each from a model that never saw that area), `regions.csv`, `model.joblib`, `cache/risk_features.pkl` (cache; `--rebuild` to recompute).
- **Most at-risk regions (mean score):** EMSR520 AOI01 (NL) 0.44, EMSR517 AOI07 0.43, AOI08 0.37.
- **Caveats:** scores are relative rankings, not calibrated probabilities (balanced sampling). Trained on one event. CEMS labels are incomplete (cloud, urban). The DEM is a surface model (includes buildings and trees).

## 7. Interpolating between PRODUCT and MONIT01 (`interp_model.py`)

> **Prompt:** "could you write a model now that predicts the intermediate data between the PRODUCT and MONIT? so we can make the simmulations in between the time"

**What the data allows:**
- MONIT01 is cumulative: it contains the PRODUCT polygons plus new polygons from a later image. Each polygon's image date comes from `dmg_src_id` → `source` table.
- Time gaps: DE DEL areas 15/07 05:50 → 16/07 05:41 (about 24 h); BE areas 15/07 → 17/07 17:30 (about 60 h); DE AOI15 has three dates (16, 18, 20/07).
- Each image covers a different footprint, so change can only be simulated where both footprints overlap.

**Method:** no training data exists for flood dynamics with only 2 snapshots, so this is a physically motivated interpolation, not ML. Cells wet at both times stay wet. Receding cells dry from **highest above the river first**; newly flooded cells fill from **lowest first**. The water level moves linearly between the two images. Baseline for comparison: signed-distance shape morphing (ignores terrain).

**Results (9 simulated intervals, 8 areas):**

| Area | Interval | Flooded km² (start → end, overlap only) |
|---|---|---|
| BE EMSR518 AOI01 | 15/07 05:50 → 17/07 17:32 | 7.53 → 11.03 (rising) |
| BE EMSR518 AOI02 | 15/07 05:50 → 17/07 17:32 | 8.30 → 9.37 |
| DE EMSR517 AOI01 | 15/07 05:50 → 16/07 05:41 | 0.28 → 1.88 |
| DE EMSR517 AOI03 | 15/07 → 16/07 | 1.68 → 2.63 |
| DE EMSR517 AOI04 | 15/07 → 16/07 | 10.62 → 5.54 (receding) |
| DE EMSR517 AOI05 | 15/07 → 16/07 | 9.82 → 2.69 (receding) |
| DE EMSR517 AOI06 | 15/07 → 16/07 | 11.41 → 7.47 (receding) |
| DE EMSR517 AOI15 | 16/07 → 18/07 → 20/07 | 0.74 → 0.24, then 0.29 → 0.77 |

(DE AOI01 also has a RADARSAT image on 14/07 17:26 with no flood mapped, so that interval is empty.)

**Validation (only one usable case):** predicting AOI15 on 18/07 from 16/07 and 20/07 gives IoU **0.14** with the terrain model, vs 0.007 for shape morphing and 0.02 for the nearest snapshot. Terrain ordering clearly helps, but absolute accuracy is low: the observed extent goes down, then up again (0.74 → 0.24 → 0.77 km²), which no smooth interpolation can follow.

**Caveats:**
- Part of each "change" is a **sensor difference**, not water movement (e.g. BE 17/07 adds 3 m COSMO-SkyMed imagery on top of 10 m Sentinel-1; AOI15 mixes aerial photos and Pleiades).
- Linear water-level change is an assumption; real recession is faster at first.
- Validated on one case only. Present the in-between extents as **plausible scenarios, not observations**.

**Bug fixed:** writing the state raster failed (`-1` does not fit in `uint8`); now cast to float first.

**Outputs (`code/outputs/interp/`):** `extents.gpkg` (one layer per interval, polygon per 6 h step, `--step` to change), `timeseries.csv` (flooded km² per step), `<interval>.tif` (band 1: 0 dry / 1 wet throughout / 2 receding / 3 rising; band 2: hours after start when each cell changes).

## 8. Note

> **Prompt:** "put everything in this conversion and the prompts we used into a note aswell"

*(This file.)* Also saved as a standing instruction in Claude's memory: keep this file updated after every task, and read it before answering new requests.

## 9. Roads under future floods (`road_model.py`)

> **Prompt:** "there is road data in the data can we make models that checks for intersection of the future floods with thoase roads."
> **Prompt:** "keep making notes of things you do and when asked something else update yourself with the notes"

**Data:** CEMS `transportationL` (OSM-derived): highways, primary/secondary/local roads, cart tracks, railways, bridges, about 16,000 km over 18 AOIs (AOI18 has no roads). Cut into 50 m pieces. The damage grading (GRA) maps carry **`damage_gra`** per road (No visible damage / Possibly damaged / Damaged / Destroyed), used as labels.

**"Future floods" = forward-looking layers:** EFAS RP10/RP100/RP500 (road impassable if depth ≥ 0.3 m, an assumed car threshold), ML high risk (score ≥ 0.5 from `risk_model.py`), plus the 2021 observed extent as reference and the `interp_model.py` steps for a closure timeline.

**Results:**

| Road class | Total km | 2021 | RP10 | RP100 | RP500 | ML high risk |
|---|---|---|---|---|---|---|
| Highway | 2,345 | 14 | 203 | 303 | 361 | 276 |
| Primary road | 3,082 | 43 | 524 | 631 | 704 | 671 |
| Secondary road | 3,486 | 19 | 322 | 422 | 493 | 580 |
| Local road | 2,966 | 79 | 252 | 503 | 755 | 427 |
| Railway | 1,836 | 21 | 552 | 724 | 885 | 693 |
| Bridge | 7.6 | 0.8 | 6.4 | 7.1 | 7.1 | 1.5 |

- Each scenario catches only **52–59 %** of the road km that actually flooded in 2021 (RP10 52 %, RP100 57 %, RP500 59 %, ML 55 %). 2021 flooded places the hazard maps did not expect.
- **Road damage model** (gradient boosting on terrain + EFAS + ML risk + road class, pre-flood inputs only, leave-one-area-out over 8 GRA areas): median AUC **0.746**, **not better** than RP100 alone (0.769) or ML risk alone (0.753); fails on AOI07 (0.35). Conclusion: use the simpler RP100 / ML risk layers for roads; the extra model adds nothing yet.
- The observed 2021 extent "predicts" damage perfectly (AUC 1.0 in 5 areas), but that is **circular**: CEMS graded road damage mostly inside the mapped flood.
- **Closure timeline** (road km under water, start → end of interval): DE AOI04 19.7 → 4.3 km in 24 h (main roads 12.7 → 4.3); DE AOI06 1.4 → 0; BE AOI01 2.8 → 0.4 km over 60 h; DE AOI01 and BE AOI02 rise (1.3 → 3.2, 1.1 → 3.3 km).

**Bugs fixed during this step:** (1) `.area` on a GeoDataFrame returns geometric area, not the `area` column, so the table is converted to a plain DataFrame; (2) areas sharing the same date pair were summed together in the timeline (DE AOI01 showed 27 km instead of 1.3 km), now split per area; (3) scikit-learn could not encode the pandas-3 `road_class` category, so road class is passed as integer codes.

**Outputs (`code/outputs/roads/`):** `exposure_by_class.csv`, `exposure_by_area.csv`, `damage_validation.csv`, `closure_timeline.csv`, `roads_at_risk.gpkg` (pieces flagged by any scenario, with all columns; open in QGIS), `cache/road_pieces.pkl` (cache, `--rebuild` to recompute).

## Questions without code

> **Prompt:** "what can of data do these models take now?"

Answered with the input table per script: CEMS folders (layers found by name: `observedEventA`, `areaOfInterestA`, `hydrographyL/A`, `transportationL`, `imageFootprintA`, `source`), GHS-OBAT `.gpkg` auto-found as `obat_XX/` next to `ems_XX/`, EFAS and GHS-POP at fixed paths, DEM downloaded. Limits: the batch models scan all of `working_data/` (no single-file input yet); the saved risk model needs a scoring script for new regions; DEM tile naming only works north of the equator / east of Greenwich.

> **Prompt:** "i also want to predict if the flood has a big risc of overflooding and increasing in its dangers but that is based on that data we had earlier that contained information anboutn the soil and weather etc. can we train on that aswell?"

Checked: there is **no** soil/weather data in the challenge data (also stated in `~/hackathon/notes/05_external_data.md`, which only lists external sources). Proposed ERA5-Land via the Open-Meteo archive (no key) + HYSOGs250m soil groups, and warned that "escalation" can only be learned from 8 PRODUCT→MONIT01 changes (a demo, not a forecaster). The user dropped the idea in the next prompt.

> **Prompt:** "can we think about getting infromation about damaged stuff. recent damaged stuff. becausd the original data set showed this but if try to make a guess on the current situtatuion it might be hard"

Discussion only. The CEMS damage grades are a July 2021 snapshot; repairs since then are unknown, so the current state cannot be guessed from the challenge data. Two meanings, two kinds of data (all external, none verified yet):
- **Recovery since 2021:** OSM history via the ohsome API (compare 2021 `damage_gra` roads with later OSM edits, giving a recovery timeline); official reconstruction reports; satellite change detection (too heavy for 2 days).
- **Live situation in a next flood:** new CEMS activations (same format, so the scripts work unchanged), national road-closure feeds (DE/BE/NL), alerts (GDACS, Meteoalarm, national warning apps).
- **Proposed method:** use our models as the prior and update with the first live reports (e.g. one closed bridge, then recompute who is cut off): "where to look first".
- Options offered: (A) OSM recovery check for AOI15; (B) "live update" mode in `access_model.py` (mark roads closed, recompute cut-off areas, no external data). Recommended B. Waiting for the user's choice.

> **Prompt:** "is the api capable of providing a complete prediction of the flood visually in time and space?"

Answer: **no**. Checked `flood_api` (outputs `risk.tif`, `features.csv`, `facilities.geojson` per bbox ≤ 3,000 km²; roads/buildings/factors still on `notes/10_todo.md`). Space: yes as a relative-risk ranking; extent/depth only as fixed EFAS scenarios (no depth raster written yet). Time: nothing is time-dependent; `interp_model.py` only fills gaps between past images (IoU 0.14). Visual: files for QGIS, no viewer. Proposed: (1) write extent/depth rasters per scenario from `flood_tool.py`; (2) "staged scenario timeline" driven by a river-flow forecast (e.g. Open-Meteo Flood API / GloFAS, unverified, external) mapping forecast return period → EFAS stage per hour; (3) hydrodynamic model as future work; (4) web map with time slider for the demo. Waiting for the user's choice.

> **Prompt:** "give me a list of the data sources we have"

Answered from the Data sources section plus a check of `working_data/` (sizes: EMS 948 MB, EFAS 902 MB, OBAT 11.5 GB, GHS-POP 984 MB, DEM 298 MB, OSM 2.0 GB). Noted: GHS-POP 2025 is present but unused; Open-Meteo discharge was never retrieved (daily limit); placeholder assumptions (cost, storey, m²/person, 0.3 m, evacuation thresholds) have no source.

## 10. Access danger: can help reach people? (`access_model.py`)

> **Prompt:** "okay no ignore that soil stuff but we need to start calculating in danger factors like roads. roadds cause for help to be blocked and cant arrive. take as much parmaetsd we have avilable and guesws the risk based on t hose. but dont put it in a single random number it should be clear because of what"

**Assumption:** help comes from **outside each mapped area along the CEMS road network** (no rescue-station data; the Overpass query in `~/hackathon/notes/06_rescue_apis.md` failed). Roads only, no railways or airfields. DEL areas only contain main roads, so access there means "via the main road network".

**Method:**
- Road lines become a graph of ≤50 m stretches. Nodes within 300 m of the AOI edge are where help enters.
- Per scenario, flooded stretches (depth ≥ 0.3 m, or inside the observed extent) are closed. Shortest paths (Dijkstra) are computed twice: flooded bridges kept open, and flooded bridges lost. Bridges = tagged bridges or road stretches crossing CEMS rivers/water.
- Every OBAT building is snapped to the nearest road node (≤2 km, otherwise not assessed). Occupants = GHS-POP 2020 pixel population split evenly over the buildings in that pixel.
- Results are aggregated to **1 km cells**. Each factor is reported separately, with value, level and reason:

| Factor | Medium ≥ | High ≥ |
|---|---|---|
| Flooded buildings (% under ≥0.3 m) | 5 % | 25 % |
| Cut off by road (% unreachable from outside) | 10 % | 50 % |
| Depends on bridges (% reachable only if flooded bridges hold) | 10 % | 50 % |
| Long detour (median route length vs normal) | 1.3× | 2.0× |
| People affected (in flooded or cut-off buildings) | 100 | 500 |
| No upper floor (% of flooded buildings single-storey, ≥5 flooded) | 20 % | 50 % |
| Flood-prone terrain (mean `risk_model.py` score) | 0.3 | 0.5 |

**Overall level = count of HIGH factors**, no weights: ≥3 Critical, 2 High, 1 HIGH or ≥2 medium Elevated, else Low. Each cell has a `reasons` text, e.g. "HIGH: Cut off by road, 96% of buildings unreachable from outside; HIGH: People affected, ~524 people …".

**Results (all 18 areas with roads, ~5.76 M people in the mapped areas):**

| Scenario | Critical | High | Elevated | People flooded or cut off |
|---|---|---|---|---|
| Observed 2021 | 5 | 50 | 950 | ~14,100 |
| RP10 | 636 | 504 | 1,776 | ~694,000 |
| RP100 | 840 | 626 | 1,732 | ~1,049,000 |
| RP500 | 972 | 675 | 1,668 | ~1,336,000 |

- 2021 top cell: EMSR517 AOI03 (50.51 N, 6.99 E), 96 % of 250 buildings cut off, ~524 people.
- **Contradiction to report:** RP10 cuts off far more than the real (much rarer) 2021 flood. Two reasons: (1) EFAS is 90 m, so riverside roads share a pixel with the river channel and get flagged; (2) the radar-based 2021 extent misses water in towns and under trees, and ignores debris and damaged roads. Treat **observed = lower bound, RP = upper bound**. Within one scenario the *ranking* of cells is more trustworthy than the absolute numbers.
- "Depends on bridges" rarely triggers: where a bridge floods, its approaches usually flood too.

**Bugs fixed:** (1) population was first taken for the whole 1 km cell even when only one building of it lay inside the AOI (6,600 people for 1 building); now per-building occupants; (2) the summary table lost the area names (same `.area` issue as step 9).

**Outputs (`code/outputs/access/`):** `access_danger.gpkg` (layer per scenario: 1 km cells with every factor, level, `overall`, `reasons`, `lon`/`lat`; open in QGIS), `danger_<scenario>.csv` (cells above Low, ranked), `run_log.txt`.
Usage: `python3 access_model.py [folders…] --scenario observed rp10 rp100 rp500`.

## 11. Notes check

> **Prompt:** "make sure evyerthing is noted and updated"

Reviewed this file against the code and outputs. Added: earlier work and installed packages, missing bug fixes (steps 7, 9, 10), DEM tile details, risk ranking, a separate "Questions without code" section, run order, open issues, and complete file and data tables.

## 12. Restructure + final analysis

> **Prompt:** "do i final analysis on what we have restruture the folders etc. what can be improvede to be able to do a guess on an arbtirrary flood within the given region they gave us NL BE DE"

**Found while doing this:** the team has a parallel workstream (another session): `flood_api/` (Rust region API: any bbox/polygon → same 8 risk features from DEM + EFAS + **OSM** rivers → scored with our model via `code/score_region.py`), `code/grid_io.py` (npy handoff format), and design notes `~/hackathon/notes/08_rust_region_api.md` and `09_ai_layer.md`. It was running (port 8080) and downloading OSM extracts into `working_data/osm/` during this step, so those were not touched.

**Restructure (code only; data, `notes/`, `processed/`, `flood_api/` untouched):**
```
code/
  paths.py              all folders in one place (DATA, OUTPUTS, CACHE)
  *.py                  scripts (names unchanged)
  NOTES.md              this log
  outputs/  report/ risk/ interp/ roads/ access/   results
  cache/    risk_features.pkl, road_pieces.pkl     recomputable tables
```
- Every script now takes its paths from `paths.py`. `score_region.py` and the Rust API follow automatically (they import `risk_model.OUT`). Paths in `notes/08` and `notes/09` updated; comments in `flood_api/src/*.rs` still say `code/risk/model.joblib` (comments only, left alone).
- `preprocess.py` defaults pointed to `code/working_data` (broken after an earlier move), now `paths.DATA` / `~/hackathon/processed`.

**Bug fixed (found by the API work, `notes/08`):** `risk_model.py` read EFAS with a fractional window, so the data and its transform were off by up to 1 pixel (~90 m) on ~4.7 % of cells. Now whole-pixel windows. Retrained: `risk_model.py --rebuild`, then `road_model.py --rebuild` and `access_model.py` rerun on the fixed features. Effect is small; the conclusions of steps 6, 9 and 10 still hold. **Current values (replace the older ones in steps 6/9/10):**
- Risk model median held-out AUC **0.924** (was 0.926); RP100 alone 0.823 (was 0.820); feature ranking unchanged (relief > elevation > distance to river).
- Road damage model median AUC 0.729 vs RP100 0.747 and ML risk 0.752: still no better than the simple layers. Scenarios catch 51–58 % of 2021 flooded road km (ML high risk 56 %).
- Access: observed 5 Critical / 50 High cells, ~14,100 people; RP100 844 / 623, ~1.05 M (unchanged in substance).
- Full log: `outputs/rebuild_log.txt`.

### What we have, and how much to trust it

| Component | What it delivers | Trust |
|---|---|---|
| Data understanding, `main.py` | Clean summaries per AOI | High |
| `pipeline.py` / PDF | € damage, displaced people | Low: depth is a scenario map, costs are placeholders |
| `risk_model.py` | Flood-prone terrain ranking, 100 m maps | **Best part**: tested on unseen areas, beats EFAS alone. Ranking, not probability |
| `interp_model.py` | Extents between satellite passes | Low: plausible scenarios, IoU 0.14 on the one test |
| `road_model.py` | Road km at risk per scenario, closure timeline | Medium for exposure; the damage classifier adds nothing over RP100 |
| `access_model.py` | Who is cut off, explained factor by factor | Medium: transparent, but bounded between "observed" (too low) and EFAS (too high) |
| `flood_api` (team) | Features + risk for any bbox | Promising; river features differ from training (below) |

### What blocks a guess for an arbitrary flood in NL / BE / DE (in priority order)

1. **Training and serving use different rivers.** The model learned `hand` / `dist_river` from CEMS hydrography, but the API computes them from OSM (dist_river correlation 0.07, median 242 m vs 768 m, `notes/08`). *Fix:* retrain `risk_model.py` with OSM rivers from the same extracts and filter as the API, so both sides compute features identically. Needs OSM for every training AOI: RLP and Belgium present, NRW downloading, **Netherlands extract missing** (EMSR520).
2. **Roads and rivers only exist inside the CEMS AOIs.** `road_model.py` and `access_model.py` read CEMS `transportationL` / `hydrography`. *Fix:* switch both to OSM (same extracts), which also gives real fire stations and hospitals, so "help comes from the AOI edge" can become "route from the nearest station". Region-wide coverage needs the Germany and Netherlands extracts (large).
3. **The scripts need a CEMS folder as input.** For a new flood there is no observed extent. *Fix:* one input layer, `--region <bbox|polygon>` plus `--flood <polygon file | rp10/rp100/rp500 | risk≥t>`, decoupled from CEMS folders. A new CEMS activation stays one possible flood input (same format, works as is).
4. **EFAS depth is coarse (90 m) and a scenario.** It is the cause of "RP10 cuts off more than 2021". *Fix:* a DEM check per road and building: flooded only if its height above the river is below the local flood water level (EFAS depth + ground level at the nearest flooded EFAS cell).
5. **Everything was learned from one event, mostly in hilly valleys** (Eifel/Ardennes) plus one Dutch area. Dutch polders, dike-protected land and the North German lowlands are outside what the model has seen: `hand` means little behind a dike. Check whether the EFAS maps include flood defences before using them in NL. *Fix:* an out-of-distribution flag per region (planned in `notes/09`), more training events (other CEMS activations in the three countries, same format), and leave-one-event-out validation.
6. **Scores are rankings, not probabilities** (balanced training sample). Keep the wording "relative risk", or calibrate on the natural flood share before showing a probability.

**Do now (hackathon):** 1 + 3 (makes the API's risk trustworthy for any region), then 2 for access. **Brief / future work:** 4, 5, 6.

### Scope clarified: the study region only, not whole countries

> **Prompt:** "in the same region as in the data set. not the entire countries"

- **Study region** = convex hull around the 19 PRODUCT AOIs: **28,400 km²**, lon 4.97–7.73, lat 49.54–51.48 (Wallonia/Meuse, Dutch Limburg, Eifel/Ahr/Erft/Rur, Moselle, Rhine near Cologne). The AOIs cover **26.5 %** of it (7,540 km²); the other 73.5 % has no CEMS labels, roads or rivers. Saved by the new `region.py` → `outputs/region/study_region.gpkg` (layers `hull`, `aois`).
- **What this makes easier:**
  - Our leave-one-area-out test (median AUC 0.92) *is* this use case: a flood in a part of the region the model has not seen.
  - The "outside training conditions" concern (old point 5) mostly disappears: same terrain types, Dutch Limburg is in training.
  - Data coverage: EFAS, GHS-OBAT and GHS-POP cover the whole hull; DEM tiles are downloaded automatically (9 present, 1–2 edge tiles may still be fetched).
  - OSM only for this region: Rheinland-Pfalz ✓, Nordrhein-Westfalen downloading (`.part`), **Belgium not present** (despite `notes/08`), **Dutch Limburg missing**, Luxembourg / Saarland only if EMSR517 AOI01's southern edge needs them (`notes/07`).
- **Revised priorities:**
  1. **Same rivers in training and use:** retrain `risk_model.py` with OSM rivers (needs the extracts above), so the API's features match.
  2. **Region input + domain check:** `--region <bbox|polygon>` and `--flood <polygon | rp10/rp100/rp500 | risk≥t>`; reject or flag requests outside `study_region.gpkg` (replaces the out-of-distribution flag).
  3. **OSM roads + fire stations/hospitals** for `access_model.py` in the 73.5 % between AOIs.
  4. **DEM depth check** (fixes "RP10 cuts off more than 2021").
  5. **"Relative risk" wording** or calibration.
- Dropped for this scope: whole-country OSM extracts and Dutch polder/dike handling. More flood events in the same region would still be the best extra validation.

## 13. Finishing the roadmap (items 1–5)

> **Prompt:** "okay finish what needs to be done"

- **OSM data (`osm_data.py`):** downloaded the missing Geofabrik extracts (Dutch Limburg, Luxembourg, Saarland; Belgium and NRW were finished by the API workstream). One pass per extract builds `cache/osm_study_region.gpkg` for the study region + 0.05°: `waterways` (river/stream/canal, same filter as `flood_api`), `water` (natural=water), `roads` (drivable highways → Highway / Primary / Secondary / Local road, `bridge` tag) and `facilities` (fire stations, hospitals).
- **Item 1, same rivers:** `risk_model.Area` now takes rivers from OSM by default (`--rivers cems` reproduces the old variant, scores only, writes nothing). The model file records `rivers="osm"`. Cache: `cache/risk_features_osm.pkl` (old: `risk_features_cems.pkl`). **Result:** OSM rivers match CEMS rivers in quality (median held-out AUC 0.923 vs 0.922; average precision 0.241 vs 0.255) and are *better* on the urban GRA areas (AUC 0.835 vs 0.744; OSM has denser streams). So the model and `flood_api` now use the same river source with no loss. OSM layers built: 170,226 waterways, 74,357 water bodies, 1,395,596 road segments, 3,469 fire stations/hospitals.
- **Test on an unseen region** (`flood_tool.py --region 6.0,50.3,6.3,50.45`, 356 km² on the BE/DE border, 0 % covered by AOIs): works end to end. RP100: 210 of 25,629 buildings flooded, 406 cut off, 9 help bases, 4 cells "Evacuate now" (~547 people). `risk>=0.5`: 2,859 flooded, 3,507 cut off (the risk threshold flags far more than EFAS RP100). Output in `outputs/regions/between_aois_test/`.
- **Item 2, any region (`flood_tool.py`):** `--region w,s,e,n | polygon file | AOI name`, `--flood rp10|rp100|rp500|risk>=t|polygon file|observed`. Refuses regions more than 1 % outside the study region (`--allow-outside` clips) and regions over 3,000 km². Writes `outputs/regions/<name>/`: `risk.tif`, `access_danger.gpkg`, `danger_<flood>.csv`, `evacuation_<flood>.csv`, `summary.json` (with sources and caveats). `Area` can now be built from any polygon (`Area(region=..., name=...)`).
- **Item 3, OSM roads and help bases:** `access_model.py` rewritten on OSM roads (bridges = OSM tag or crossing water). Help starts from OSM fire stations and hospitals (snapped ≤ 500 m) plus every road entering the region. New factor **Far from help** (road km from the nearest fire station/hospital; medium ≥ 8 km, high ≥ 15 km, assumed). Buildings from every OBAT country file that overlaps the region.
- **Item 4, DEM depth check:** `Area.depth(rp, xy)`: water surface = lowest ground within ~75 m + EFAS depth; ground at the point = lowest DEM within ~45 m (drops roofs and trees); higher points are dry. Tested by `eval_depth_check.py` (`outputs/eval/depth_check.csv`) on all road pieces against the 2021 extent:

| Flood | Check | Flagged km | Precision | Recall | F1 |
|---|---|---|---|---|---|
| RP10 | raw | 2,219 | 0.054 | 0.507 | 0.098 |
| RP10 | DEM | 1,634 | 0.061 | 0.423 | 0.107 |
| RP100 | raw | 3,062 | 0.043 | 0.558 | 0.080 |
| RP100 | DEM | 2,356 | 0.048 | 0.478 | 0.087 |
| RP500 | raw | 3,728 | 0.037 | 0.577 | 0.069 |
| RP500 | DEM | 2,902 | 0.041 | 0.504 | 0.076 |

  The check flags about 25 % fewer roads, raises precision about 12 % and costs about 15 % recall; F1 improves slightly in every scenario. It is a **modest** improvement, not a fix: EFAS still flags ~20× more road than was seen flooded in 2021 (partly because the 2021 extent misses water in towns). Now the default (`--no-dem-check` for raw).
- **Item 5, wording:** the terrain factor now reads "relative terrain risk … (a ranking, not a probability)"; caveats in every `summary.json`.

## 14. Evacuation: who has to leave, in what order

> **Prompt:** "when a flood happens locations with a higher density of people would neeed to evacuate. based on population data predict which regions need to evacuate together with all of the other models and parameterswe hae untill now"

Built into `access_model.py` (so also in `flood_tool.py`); combines flood depth (EFAS + DEM check), buildings (storeys), population (GHS-POP per building), road network (cut-off, bridges, detours), help bases (OSM) and terrain risk (`risk_model.py`). Fixed rules per building, no weighted score:

| Category | Rule | Why |
|---|---|---|
| Evacuate now | flooded and (depth ≥ 1.0 m, or single-storey) | no safe place left inside |
| Evacuate early | flooded (≥ 0.3 m) or cut off by road | cannot be reached or supplied later |
| Prepare | terrain risk ≥ 0.5 and (bridge-dependent, detour ≥ 2×, or ≥ 15 km from help) | may become cut off |
| By boat/air | evacuate now/early **and** cut off | no road out |

Per 1 km cell: people per category; the cell gets the highest category with ≥ 25 people (assumed). **Priority** = fixed order: level, then people "now", then "early", then "prepare". Each cell has `evac_reasons` in words and its population (density per km²). Depth is only known for EFAS floods, so for observed/polygon/risk floods "now" relies on single-storey only.
Outputs: `evacuation_<flood>.csv` (ranked list) and the evacuation columns in `access_danger.gpkg`.

**Results (all 18 AOIs, OSM roads, DEM-checked EFAS):**

| Flood | Cells: now / early / prepare | People: now | early | by boat/air (of now+early) | prepare |
|---|---|---|---|---|---|
| Observed 2021 | 51 / 81 / 16 | 6,340 | 20,380 | 24,058 | 2,756 |
| RP10 | | 156,725 | 207,891 | 323,328 | 20,810 |
| RP100 | 1,233 / 395 / 25 | 304,723 | 350,280 | 569,959 | 20,851 |
| RP500 | | 432,201 | 442,214 | 757,174 | 20,584 |

- 2021 replay, top of the list: EMSR517 AOI18 (50.6 N, 6.5 E, ~442 people "now", ~434 only reachable by boat/air) and AOI15 (50.5 N, 7.0 E, ~425 "now"), among the worst-hit areas of the 2021 event, which is a plausibility check, not a validation.
- Most affected people are **cut off** rather than flooded (boat/air ≈ 90 % of now+early), because a building counts as cut off when its road is closed.
- For observed / polygon / risk floods the depth is unknown, so "now" relies on the single-storey rule only (OBAT height < 4.5 m), which probably overstates it.
- EFAS scenarios remain upper bounds (see step 10 and the DEM check).
- People in flooded or cut-off buildings: observed 26,719; RP10 364,616; RP100 655,003; RP500 874,415 (RP100 was 1.05 M before OSM roads + DEM check).

## 15. Model precision and how to improve it

> **Prompt:** "okay now we need to check the models precision. we have built and performance how can we improve those"
> **Prompt:** "update the notes when you are done"

`evaluate.py` scores everything on data the model did not see → `outputs/eval/` (`summary.csv`, `risk_grid.csv`, `building_damage.csv`, `feature_experiment.csv`).

**Risk model on the full grid of each held-out AOI** (real flood share ≈ 1 %, not the balanced training sample), medians over 18 AOIs, CEMS-river features:

| Model | ROC AUC | Avg precision | Top 10 % flagged: precision / recall |
|---|---|---|---|
| Risk model | 0.922 | 0.255 | 0.081 / 0.723 |
| EFAS RP100 depth alone | 0.828 | 0.123 | 0.072 / 0.669 |
| (random) | 0.5 | 0.010 | 0.010 / 0.10 |

- **Ranking is good, precision is low**: flagging the top 10 % of an unseen area catches ~72 % of what flooded, but only ~8 % of flagged cells actually flooded (8× better than random, because only ~1 % floods). That is normal for rare events. Present outputs as "where to look first", never "will flood".
- **Weak spot:** urban damage-grading AOIs (AOI07–11: AUC 0.60–0.74). River-valley DEL areas: 0.91–0.99.

**Experiments (cached features, same held-out test):**

| Variant | AUC DEL / GRA | Avg precision DEL / GRA |
|---|---|---|
| Baseline | 0.931 / 0.744 | 0.255 / 0.184 |
| More regularised (min leaf 200, depth 4) | 0.934 / 0.740 | 0.253 / 0.189 |
| Train on DEL areas only | 0.922 / 0.784 | 0.214 / 0.207 |
| Without EFAS features | 0.923 / 0.741 | 0.119 / 0.127 |
| Terrain only (elev, relief, hand, dist) | 0.919 / 0.728 | 0.109 / 0.123 |

→ Tuning does not help (capacity is not the limit). EFAS features double precision. GRA labels are noisy for training (DEL-only helps GRA but hurts DEL): a trade-off, not a win. **Gains must come from new information**, so candidate features were added to `risk_model.Area` (`EXTRA`: relief at 300 m and 3 km, height above / distance to *major* rivers from OSM) and tested; the API contract stays the 8 `FEATURES` unless the gain is clear.

**Results after the OSM retrain (`chain_log.txt`, `outputs/eval/`):**

Feature experiment (held-out full grids, medians over AOIs), saved as `outputs/eval/feature_experiment.csv`:

| Features | AUC | Avg precision | Top 10 %: precision / recall | GRA AUC / AP |
|---|---|---|---|---|
| v1 (8, current API contract) | 0.923 | 0.241 | 0.080 / 0.717 | 0.835 / 0.236 |
| + major rivers | 0.922 | 0.253 | 0.082 / 0.762 | 0.847 / 0.239 |
| + multi-scale relief | 0.923 | 0.245 | 0.083 / 0.744 | 0.840 / 0.250 |
| **v2 (all 12)** | **0.926** | **0.256** | **0.084 / 0.767** | **0.851 / 0.272** |

→ v2 is a consistent but **modest** gain (+6 % avg precision, +5 points recall at the same flagged area, best on urban areas). Saved as `outputs/risk/model_v2.joblib`; `model.joblib` stays v1 so the running `flood_api` is not broken. Switching needs the API to compute the 4 extra features (relief at 11 and 99 px; height above / distance to OSM `waterway=river` + water, same recipe as `hand`/`dist_river`).

Building damage (CEMS `builtUpP`, 10 GRA areas; it lists only damaged buildings, so negatives = OBAT buildings 20 m–1 km from a graded one), median ROC AUC:

| Score | Hit (graded vs not) | Severity (damaged/destroyed vs possibly) |
|---|---|---|
| Terrain risk (held-out map) | **0.71** | 0.57 |
| EFAS RP100 depth, raw | 0.67 | 0.55 |
| EFAS RP100 depth, DEM-checked | 0.64 | 0.55 |
| Inside observed 2021 extent | 0.89 (circular) | 0.70 |

→ Terrain risk finds damaged buildings best. The DEM check slightly **hurts** for buildings (the surface model includes roofs; it helps for roads, see step 13). **Nothing predicts damage severity**, so the depth → damage-fraction → € stage of `pipeline.py` / the PDF has no support in this data: present € figures as illustrative only.

Other models (unchanged): road damage classifier median AUC 0.706 vs RP100 0.747 and terrain risk 0.747 (drop it, use the simple layers); in-between model IoU 0.14 on its single test.

**How to improve, in order of expected gain:**
1. **Better labels, not bigger models.** More flood events in the same region (other CEMS activations), and treating GRA "flood trace" vs "flooded area" separately. Tuning gave nothing.
2. **Adopt v2 features** in both Python and `flood_api` (+6 % precision; cheap to compute).
3. **Flow accumulation / upstream catchment area** from the DEM (needs a flow-routing step, not installed): the most informative terrain variable still missing, especially for flash floods.
4. **Depth:** keep the DEM check for roads, drop it for buildings (or use a bare-earth DEM instead of the surface model).
5. **Report precision honestly:** ~8 % precision at 72–77 % recall when flagging the top 10 % of area. Use "where to look first" language; calibrate if a probability is ever needed.
6. **Evacuation thresholds** (1.0 m, single-storey, 25 people, 15 km) are assumptions; there is no ground truth for evacuation, so show them and let users change them.

---

## 16. Flood in time and space: timeline + viewer (`timeline.py`)

> **Prompt:** "implement it we really need that"

**What it does:** for any region in the study area, a time series of flood return periods drives, step by step: water depth map → roads closed → buildings flooded / cut off → evacuation levels, shown in an HTML map with a time slider.

**Drivers (time input):**
- `--peak-rp 500 --rise-h 24 --fall-h 72 --step-h 6`: **what-if hydrograph** (log T rises linearly from 2 years to the peak, then falls). Works offline; the demo default.
- `--series file.csv`: `time, return_period`, or `time, discharge` + `--q10 --q100` (Gumbel through both). For a real gauge or official forecast.
- `--open-meteo [lon,lat] --start --end`: GloFAS discharge via Open-Meteo, thresholds from 1984–2020 annual maxima (Gumbel, method of moments), cached in `cache/discharge/`. **Not tested:** the API refused today ("Daily API request limit exceeded"), so do not rely on it for the demo.

**Space:** depth for any T = EFAS RP10/100/500 depths (DEM-checked) interpolated on log T (`risk_model.blend_T`); below RP10 the depth shrinks to 0 at T = 2 (assumption: rivers stay in their banks up to a 2-year flow); above RP500 it is held at RP500. Depth grid 0.0005° (~35 × 55 m). New flood spec `T=<return period>` in `access_model.py` (also usable in `flood_tool.py`); steps with the same T (2 significant digits) share one analysis.

**Outputs (`outputs/timelines/<name>/`):** `steps.csv` (per step: T, flooded km², roads closed km, buildings flooded / cut off, people now / early / boat-air / prepare), `depth_<step>.tif`, `cells.gpkg` (evacuation cells per step), **`viewer.html`** (self-contained: Leaflet from cdnjs + OSM tiles, depth overlay, evacuation cells with reasons in popups, fire stations/hospitals, return-period chart, KPIs, "evacuate first" list, play button).

**Runs:**
- `ahr_whatif` (6.9–7.15 E, 50.45–50.58 N, peak RP500 at H+24, recedes by H+96): H+6 (T≈8) 4.5 km² flooded, 47 km roads closed, ~2,571 people "evacuate now"; peak (T≈500) 5.3 km², 61 km, ~3,612 now / ~4,267 early. Most of the extent is already reached by RP10: in a narrow valley the walls limit spreading, so higher return periods mainly add **depth**.
- `series_test` (BE/DE border region, CSV discharge 40 → 420 → 90 m³/s, Q10 = 120, Q100 = 300): T 3.9 → 480 → 6.9; 1.8 → 2.1 km² flooded.
- Checks: viewer JavaScript parses (esprima); peak-step depth + cells rendered and inspected (water follows the Ahr valley, cells along it).

**Limits (also shown in the viewer):** this is a **staged scenario**, not hydraulics. Water does not travel downstream over time: the whole region switches stage together, from one river point. No rainfall-runoff, no flash-flood timing, no debris or bridge failure. Treat time as "when each stage is reached, if the river follows this hydrograph". A hydrodynamic model (LISFLOOD-FP) is future work.

**For `flood_api`:** it can call `timeline.py` the same way it calls `score_region.py` (subprocess, then serve `viewer.html` / `steps.csv`).

## 17. Human evacuation flow ("the human flood", `evac_sim.py`)

> **Prompt:** "make a model and an enpdoint that based on the ppopilation density it predicts how they will travel and what roads they will use and what roads they can use. so we have the wate flood but also the human evacuation flod. simulate that. based on times of day. weather. etc etc."

**Model (per region, flood, hour, weekday/weekend, weather):**
1. Evacuees per building = occupants (GHS-POP) × share at home at that hour × compliance by category (now 0.9, early 0.6, prepare 0.15 "shadow" evacuation).
2. Cars = evacuees × 0.85 car share / 2.5 persons per car; the rest need organised transport.
3. Destination = nearest safe road node by travel time: dry, ≥ 500 m from any road flooded in the target flood, or a dry road leaving the region.
4. Routes = fastest paths on the roads usable *while people leave* (`--roads-flood`). All cars are loaded onto a shortest-path tree, giving per road: cars, demand per hour over the departure window (2 h day, 3 h night), capacity left after background traffic (5 % night, 50 % weekday rush, 30 % day) and weather, load (v/c), level (free / busy / congested / overloaded), BPR delay, time to clear the queue.
5. Time to safety per building = departure window + travel time + worst bottleneck; 95th percentile per run.
- Speeds/capacities by class: Highway 100 km/h / 3,600 veh/h, Primary 70 / 1,500, Secondary 60 / 1,100, Local 40 / 600. Weather (capacity, speed): rain 0.92 / 0.9, heavy rain 0.8 / 0.8, fog 0.85 / 0.75, snow 0.7 / 0.6; night speed × 0.9. **All assumed** (no evacuation data to fit them); listed in every `summary.json`.
- **Boat/air** = evacuees with no usable road route to any safe node. First version used "cut off from help" (wrong question: a cut-off valley house can still reach a dry hillside road); fixed.

**Key finding: timing beats everything.** EMSR517 Ahr box (6.9–7.15 E, 50.45–50.58 N), flood T=500, 03:00:

| Roads closed while leaving | Evacuees | Cars | Boat/air only | 95 % safe after |
|---|---|---|---|---|
| none (order before water arrives) | 5,560 | 1,886 | 14 | 3.8 h |
| as at RP10 (order once valley roads flood) | 5,560 | 525 | 4,017 | 3.2 h |
| as at peak (T=500) | 5,560 | 214 | 4,930 | 3.1 h |

**Time of day and weather matter in dense areas.** Vesdre valley (BE, 5.55–5.95 E, 50.55–50.66 N), RP100, all roads open:

| Run | Evacuees | Cars | Congested + overloaded km | 95 % safe after |
|---|---|---|---|---|
| 03:00 dry | 64,430 | 21,877 | 0.5 | 6.0 h |
| 08:00 dry (rush hour) | 47,475 | 16,120 | 3.4 (worst stretch v/c 2.1) | 6.2 h |
| 08:00 heavy rain | 47,475 | 16,120 | 5.6 (v/c 2.6) | 7.4 h |
| 17:00 heavy rain | 44,084 | 14,968 | 5.4 | 7.0 h |

Night: more people at home and slower warning, but empty roads. Rush hour: fewer evacuees but roads already half full. Heavy rain adds ~1 h. Congestion concentrates at the dense western end (preview checked); the rural Ahr box never congests.

**Endpoint:** the other session (hackathon-41) asked not to add a second Python server: the Rust `flood_api` (`src/models.rs`, port 8080) already runs our scripts as CLIs. So `evac_sim.py` got a stable CLI (documented in `notes/08` "Model endpoints" as proposed `POST /evacuation`), and the Rust route is pending the user's/team's decision. `evac_sim.run(...)` is also importable.

**Outputs (`outputs/evacuations/<name>/`):** `summary.json` (per run + assumptions + caveats), `roads_<HHh_weather>.gpkg` (every used stretch: cars, v/c, level, clear time), `cells_<…>.gpkg` (evacuees, cars, transport needs, boat/air, time to safety per 1 km cell), `viewer.html` (run selector; roads coloured by congestion and sized by cars; flooded roads in blue; cells shaded by evacuees; JS parse-checked; 3.4 MB after merging pieces, was 35 MB). Runs kept: `ahr_evac` (peak), `ahr_evac_none`, `ahr_evac_T10`, `vesdre_evac`.

**Limits:** static assignment (fastest route chosen before congestion; no rerouting, contraflow or phased evacuation); people not at home (work/school) are not modelled; walking routes (footpaths, tracks) are not in the network, so boat/air is an upper bound; EFAS scenario floods remain upper bounds.

## 18. API test campaign: Python owner

> **Prompt (from coordinator session hackathon-41, on the user's behalf):** full API test of every feature across 8 sessions (`notes/13_api_tests.md`). This session = **Python owner**: only session editing `code/*.py` during the campaign; fixes Python bugs forwarded from API testers, keeps CLIs stable, logs fixes here, does not edit `flood_api/src`.

- **Hosting** (asked by the user, from `notes/12_hosting.md`): `http://valencia.tail142ce3.ts.net:8080`, tailnet only, systemd `flood-api`; code there is an rsync copy (last sync ~15:45, before `/evacuation` and this afternoon's model changes). Local test server for the campaign: `localhost:8081`.
- **Reference numbers** for CLI vs API parity: `outputs/eval/api_reference.json` (sent to hackathon-41). Test bbox `7.05,50.52,7.12,50.56` (EMSR517 AOI15, 22.1 km², 5,023 buildings); runs `ref_flood`, `ref_timeline`, `ref_evac` (do not overwrite), plus `/impact` EMSR518_AOI02 and stored roads/interp for AOI15.
- Built-in consistency check passed: timeline peak (T=100) = flood_tool rp100 = evac_sim building counts (449 flooded, 522 cut off).
- **Fix E1** (from /evacuation tests): `evac_sim.run` with `flood="observed"` (or `roads_flood="observed"`) on a bbox/polygon region silently returned 0 flooded / 0 evacuees. Now raises `ValueError` → CLI exit 1 + "'observed' needs a CEMS AOI name as region…", same as `flood_tool.py`. Safety net: `access_model.flood_mask` raises if the area has no observed flood. `timeline.py` only builds `T=` floods (not affected). Verified: bbox+observed → exit 1 (flood and roads_flood); `flood_tool` guard unchanged; AOI name + observed still works (EMSR517_AOI15_GRA 03h: 6,363 evacuees); reference `ref_evac` 03h_dry unchanged (1,263.49 / 427.36 / 3.25 h). CLI unchanged.
- **Fix G4** (from /flood parity test): `flood_tool` `summary.json` flood blocks lacked building counts. Each block now also has `buildings`, `buildings_flooded`, `buildings_cut_off`, `roads_closed_km`, `road_km` (from `access_model.STATS`); existing keys unchanged. Verified `ref_flood` rp100: 5,023 / 449 / 522 / 22.94 km closed of 292.26 km (matches timeline peak); risk_ge_0.5: 1,475 / 2,682 / 59.92 km. `outputs/eval/api_reference.json` updated.

## Run order

Later scripts reuse earlier outputs, so after a change rerun in this order:

0. `region.py`, then `osm_data.py` (once; builds `outputs/region/` and `cache/osm_study_region.gpkg`)
1. `risk_model.py --rebuild`: downloads DEM tiles, writes `outputs/risk/` (needed by 3–5)
2. `interp_model.py`: writes `outputs/interp/` (needed by 4)
3. `road_model.py --rebuild`: writes `outputs/roads/`
4. `access_model.py --flood observed rp10 rp100 rp500`: writes `outputs/access/` (danger + evacuation)
5. `flood_tool.py --region … --flood …`: any region, writes `outputs/regions/<name>/`
5b. `timeline.py --region … --peak-rp 500` (or `--series` / `--open-meteo`): flood in time + viewer, writes `outputs/timelines/<name>/`
5c. `evac_sim.py --region … --flood … --roads-flood none --hours 3 8 17 --weather dry heavy_rain`: evacuation traffic + viewer, writes `outputs/evacuations/<name>/`
6. `main.py`, `pipeline.py`, `report.py`, `eval_depth_check.py`: independent

## Open issues / ideas not done yet

- **Depth is the weakest input everywhere** (EFAS 90 m scenario, not the 2021 event). A DEM-based depth estimate (flood edge water level minus ground) would improve `pipeline.py`, `road_model.py` and `access_model.py`.
- **Access model:** now uses OSM fire stations and hospitals, but routes by road distance, not travel time.
- **Unverified assumptions:** OBAT `use` codes; €1,500/m² cost; 3 m storey; 40 m²/person; 0.3 m impassable depth; factor thresholds in `access_model.py`.
- **External data use** (DEM, damage curves) must be checked against Annex 3 and declared in the brief and the AI sheet.
- Any region can now be run with `flood_tool.py`; the river mismatch with `flood_api` is fixed (OSM in both).
- Open: v2 features in `flood_api`; flow accumulation; more flood events for training; evacuation thresholds are assumptions (see step 15).

## Files

| File | Purpose |
|---|---|
| `main.py` | Summary of a flood extent + buildings/depth/population |
| `pipeline.py` | Impact model chain |
| `report.py` → `outputs/report/` | PDF summary |
| `risk_model.py` → `outputs/risk/` | Flood susceptibility ML model |
| `interp_model.py` → `outputs/interp/` | Flood extents between PRODUCT and MONIT01 |
| `road_model.py` → `outputs/roads/` | Roads under flood scenarios, road damage model, closure timeline |
| `access_model.py` → `outputs/access/` | Access danger per 1 km cell: cut-off, detours, bridges, people, explained factor by factor |
| `paths.py` | All folders in one place |
| `region.py` → `outputs/region/` | Study region polygon (hull around the AOIs) + AOI outlines |
| `osm_data.py` → `cache/osm_study_region.gpkg` | OSM rivers, water, roads (+bridges), fire stations/hospitals for the study region |
| `flood_tool.py` → `outputs/regions/<name>/` | Any region + any flood: risk map, danger and evacuation tables, summary.json |
| `evaluate.py`, `eval_depth_check.py` → `outputs/eval/` | Held-out precision checks for all models; DEM depth check |
| `outputs/risk/model_v2.joblib` | 12-feature risk model (not yet used by the API) |
| `timeline.py` → `outputs/timelines/<name>/` | Flood timeline: depth per step, roads, evacuation, `viewer.html` with time slider |
| `evac_sim.py` → `outputs/evacuations/<name>/` | Human evacuation flow: who leaves, routes, congestion, time to safety, by hour/weather; `viewer.html` |
| `outputs/`, `cache/` | Results; recomputable tables |
| `grid_io.py`, `score_region.py` | Team API workstream: npy handoff format, scoring a region's features (`notes/08`) |
| `~/hackathon/flood_api/` | Team Rust region API (any bbox → features → risk) |
| `~/hackathon/working_data/osm/` | Geofabrik OSM extracts (API workstream) |
| `preprocess.py` | Earlier work (not from this log) |
| `NOTES.md` | This log |
| `~/hackathon/notes/` | Earlier research notes (datasets, plans, external data, rescue APIs) |
| `~/hackathon/working_data/dem/` | Downloaded Copernicus DEM tiles |
| `~/.local/bin/qgis`, `~/.local/bin/micromamba` | QGIS launcher, conda client |

## Data sources

- **Challenge data:** Copernicus EMS Rapid Mapping (EMSR517 DE, EMSR518 BE, EMSR520 NL; incl. roads, rivers, image dates, road damage grades), EFAS flood hazard (RP10/100/500), GHS-OBAT buildings, GHS-POP 2020 population (GHS-POP 2025 present, unused).
- **External (declare in brief and AI sheet):** Copernicus DEM GLO-30 (elevation, via AWS open data); OpenStreetMap via Geofabrik extracts (rivers, water, roads, bridges, fire stations, hospitals; ODbL, current state, not 2021); JRC global depth-damage curves (Huizinga et al. 2017); optional GloFAS river discharge via Open-Meteo (`timeline.py --open-meteo`); viewer basemap tiles from OpenStreetMap.

## AI use (for Annex 7)

All code in `code/` (except `preprocess.py`) was written by Claude Code (Opus) from the prompts above. Checks done on the results: running every script, comparing outputs with the source data (CRS, flooded area, observed road km), testing models on areas they never saw (leave-one-area-out), comparing with simple baselines, and rendering the PDF to check it visually. Each of these checks caught real bugs, listed per step. Numbers are model estimates; assumptions and limits are listed per step and under Open issues.
