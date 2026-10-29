# EMS products and AOIs

## Folder naming
`EMSR517_AOI15_GRA_MONIT01_r1_RTP01_v3`

| Part | Meaning |
|---|---|
| `EMSR517` | Activation: **517 = DE, 518 = BE, 520 = NL** |
| `AOI15` | Area of interest #15 |
| `DEL` / `GRA` | **DEL** = delineation (flood outline only). **GRA** = grading (per-building/road damage grade) |
| `PRODUCT` / `MONIT01` | **PRODUCT** = first map, made as fast as possible. **MONIT01** = first monitoring update of the same AOI, from newer images; usually larger and more complete |
| `r1` / `v3` | Release 1, version 3 (corrected twice) |
| `RTP01` | Internal EMS production code, ignore |

## Layer suffixes
**A** = area (polygon), **L** = line, **P** = point.

| Layer | Content |
|---|---|
| `areaOfInterestA` | Mapped area outline. Outside it "no flood" means "not mapped" |
| `observedEventA` | Observed flood. `notation`: *Flooded area* (water visible) / *Flood trace* (receded, marks left) |
| `builtUpA` | Built-up areas from OSM (DEL AOIs). Context only, no damage grade |
| `builtUpP` | Building points with `damage_gra` (GRA AOIs). **The damage labels** |
| `transportationL` | Roads/rail; damage grade in GRA AOIs |
| `hydrographyA` / `hydrographyL` | Normal water bodies / river centrelines (use for distance-to-river) |
| `naturalLandUseA` | CORINE 2018 land cover, 3 coarse classes (forest / agricultural / other) |
| `imageFootprintA`, `source`, `osmdiff`, `summaryTable` | Image outlines, list of images used, OSM changes, summary counts |

**Imagery:**
- DEL products are mapped from Sentinel-1 radar (10 m), so they are coarse and miss water in built-up areas.
- GRA products are mapped from Pleiades (0.5 m); EMSR517 AOI15 MONIT01 also uses aerial photos (0.12 m).
- The images themselves are **not** in the dataset, only listed in `source`.

## AOIs with damage labels (builtUpP)

| Product | Buildings | Damaged | Destroyed | Possibly damaged | Imagery |
|---|---|---|---|---|---|
| **EMSR517 AOI15 GRA MONIT01 v3** | **7,006** | 5,078 | 753 | 1,175 | 0.12 m aerial + 0.5 m Pleiades |
| EMSR517 AOI15 GRA PRODUCT v1 | 3,814 | 687 | 336 | 2,791 | 0.5 m |
| EMSR517 AOI18 GRA | 1,958 | 0 | 0 | 1,958 | single class, useless for training |
| EMSR517 AOI07 GRA | 1,752 | 25 | 10 | 1,717 | 0.5 m |
| EMSR517 AOI08–11, 14, 16, 17, 20 GRA | < 520 each | | | | |

**Recommendation: EMSR517 AOI15 GRA MONIT01** has the most data, the best imagery and a usable spread across damage classes.
**Catch:** `builtUpP` only lists affected buildings. Get "not affected" negatives from OBAT buildings inside the AOI that have no EMS point within ~10 m.

## No soil / rainfall / elevation
No AOI (including AOI15) contains soil, rainfall, elevation or soil-moisture data. See [05_external_data.md](05_external_data.md).

## Polygon overlap
Flood polygons within one EMS file overlap. Summing polygon areas overcounts. Always dissolve first.
Example: EMSR518 AOI01 MONIT01 is 19.97 km² summed but **13.99 km²** dissolved.
