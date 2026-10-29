# Datasets

All data lives in `working_data/` (also in the public GCS bucket `datasets_challenge/working_data`, see `how_to_get_started.ipynb`).
Everything is in **EPSG:4326** (lat/lon). Reproject to a metric CRS (e.g. EPSG:3035) before computing areas or distances.

The data falls into two groups: **hazard** (where water was or could be) and **exposure** (what was there).

| Folder | What | Format | Resolution / size | Role |
|---|---|---|---|---|
| `ems_be/`, `ems_de/`, `ems_nl/` | Copernicus EMS rapid mapping of the 2021 event (observed flood + damage) | Shapefiles, one folder per product | vector | **Hazard (observed) → labels** |
| `cems-efas/` | EFAS modelled flood depth for return periods RP10 / RP100 / RP500 | GeoTIFF float32, metres, nodata `-9999` | ~90 m (3″), all of Europe, 52k × 110k px | **Hazard (modelled) → features / scenarios** |
| `obat_be/`, `obat_de/`, `obat_nl/` | GHS-OBAT building inventory (one point per building) | GeoPackage, point layer | NL 11.7M buildings; DE gpkg is 8 GB | **Exposure** |
| `ghs_pop/` | GHS-POP population grid, 2020 and 2025 | GeoTIFF float64, people per cell | ~1 km (30″), global | **Exposure** |

## OBAT columns
`id, country, adm1, height, shapefactor, use, epoch, area, perimeter`.
The numeric codes for `use` and `epoch` are explained in `GHS-OBAT_R2024_online_v2.pdf` next to each gpkg. Check there before interpreting them.

## Gotchas
- **Never load OBAT DE whole**: read with `bbox=` or `mask=` (pyogrio/geopandas).
- Read rasters with windows / `rasterio.mask`, never the whole file.
- GHS-POP is 1 km, so population counts for small flood polygons are rough. Building counts are more precise.
- `.venv` has rasterio, geopandas, pyogrio, shapely, scikit-learn.
- `working_data/dem/` exists (elevation); check its source before use.

## What you can do with hazard × exposure
1. Impact of the 2021 event: buildings/people inside observed flood polygons, by AOI / use / epoch.
2. Model vs reality: does the observed extent match EFAS RP100/RP500?
3. Depth per building: sample EFAS depth at each OBAT point.
4. Population exposure: sum GHS-POP inside the flood extent, 2020 vs 2025.
