# Preprocessing: `preprocess.py`

Run: `.venv/bin/python code/preprocess.py [--data working_data] [--out processed/ems_clean.gpkg]` (~6 s)

## What it does
1. Scans `ems_*` folders and parses the product names. Keeps the newest version per stage.
2. **Keeps only AOIs that have both PRODUCT and MONIT01.**
3. Reads 8 layer types: areaOfInterestA, observedEventA, builtUpA, builtUpP, transportationL, hydrographyA, hydrographyL, naturalLandUseA.
4. Cleans each layer:
   - repairs invalid geometries, drops empty geometries and exact duplicates
   - converts "Not Applicable" / "Unknown" to missing values
   - drops mudflow/landslide polygons (keeps riverine flood only)
5. Tags every row with `country, emsr, aoi, ptype, stage, version`. Both stages are stacked, so filter on `stage` if needed.
6. Adds derived layers: `aoi_boundary` and `flood_extent`, each the dissolved union of PRODUCT + MONIT01 per AOI. **Use `flood_extent` for labels.**

## Kept AOIs (8)

| AOI | Type | Country | AOI km² | Flooded km² (union) |
|---|---|---|---|---|
| EMSR517 AOI01 | DEL | DE | 588 | 5.2 |
| EMSR517 AOI03 | DEL | DE | 209 | 3.4 |
| EMSR517 AOI04 | DEL | DE | 988 | 16.2 |
| EMSR517 AOI05 | DEL | DE | 2,935 | 14.8 |
| EMSR517 AOI06 | DEL | DE | 1,267 | 14.5 |
| EMSR517 AOI15 | GRA | DE | 120 | 11.0 |
| EMSR518 AOI01 | DEL | BE | 1,597 | 14.0 |
| EMSR518 AOI02 | DEL | BE | 1,055 | 15.7 |

## Dropped (PRODUCT only)
- EMSR517 AOI07–11, 14, 16–18, 20 (all GRA)
- EMSR520 AOI01, which means **the Netherlands is gone entirely**

As a result, EMSR517 AOI15 is the only AOI left with damage grades.
Possible later change: an option to also keep PRODUCT-only AOIs.

## Outputs (`processed/`)
- `ems_clean.gpkg`: cleaned layers + `aoi_boundary` + `flood_extent`
- `ems_summary.csv`: AOI area and flooded area per AOI
- `ems_layer_counts.csv`: feature counts per layer and stage

## QGIS tips
- Filter a layer to one stage: `"stage" = 'MONIT01'`.
- Put the AOI boundaries at the bottom or give them no fill.
- Style `builtUpP` as *Categorized* on `damage_gra`.
