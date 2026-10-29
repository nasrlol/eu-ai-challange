# External data (not in the challenge data)

The challenge zip contains **no** soil, rainfall, elevation or soil-moisture data, including for EMSR517 AOI15.
Closest available proxies:
- `naturalLandUseA`: coarse CORINE 2018 land cover
- `hydrographyA/L`: rivers and lakes
- EFAS depth maps: soil and terrain are built into the model behind them, but not available separately

**First check that the hackathon rules allow external data.**

| What | Source | Resolution | Why |
|---|---|---|---|
| Elevation / slope / height above river | Copernicus DEM GLO-30 | 30 m | Usually the strongest single predictor |
| Event rainfall (14–15 July 2021) | ERA5-Land, DWD RADOLAN (DE only) | ~9 km / 1 km | Extreme rainfall drove the event |
| Antecedent soil moisture | ERA5-Land soil water layers | ~9 km | Wet soil means more runoff |
| Soil texture | ISRIC SoilGrids, ESDAC European Soil Database | 250 m | How well the ground absorbs water |
| Hydrologic soil groups | HYSOGs250m | 250 m | Ready-made runoff classes A–D |
| Sealed surfaces / land cover | Copernicus Imperviousness HRL, ESA WorldCover | 10 m | Much finer than the CORINE classes in EMS |

For a steep, narrow valley like EMSR517 AOI15, terrain (elevation above river, distance to river) will probably matter more than 250 m soil data.
Soil and rainfall matter more when comparing across many AOIs or the whole region.
Clip any downloads to `aoi_boundary` from `processed/ems_clean.gpkg`.

## Sewer / urban drainage
- **Not in the challenge data** (checked 2026-10-29; only grep hit was the street name "Regout" in EMSR520 AOI01).
- Open sewer network data is rare and municipal; usually not public. Use as a qualitative point in the brief, or proxy with imperviousness (sealed surfaces).
