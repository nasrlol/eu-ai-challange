# Regional help/resource sources (search 2026-10-29)

## Which regions our 8 AOIs cover (bbox lon/lat)
| AOI | bbox | Region |
|---|---|---|
| EMSR518 AOI01 | 5.30–6.14 / 50.45–51.16 | BE: Wallonia (north edge may touch Flanders) |
| EMSR518 AOI02 | 4.97–5.79 / 50.03–50.45 | BE: Wallonia |
| EMSR517 AOI01, 03, 04, 15 | 6.31–7.59 / 49.54–50.59 | DE: Rhineland-Palatinate (AOI01 south edge may touch Saarland/LU) |
| EMSR517 AOI05, 06 | 6.03–7.73 / 50.51–51.48 | DE: North Rhine-Westphalia |

## Sources found (not downloaded or checked yet)
| Region | What | Source | Licence / note |
|---|---|---|---|
| DE all | Hospitals: address, operator, beds per dept (status 31.12.2024) | statistikportal.de "Verzeichnis der Krankenhäuser" | free, attribution; format unchecked |
| NRW | Hospitals CSV | open.nrw "Krankenhäuser in NRW" | DL-DE-BY 2.0 |
| NRW | Fire/rescue stations, Cologne only (WFS) | open.nrw "Feuer- und Rettungswachen, Löschgruppen" | DL-DE-Zero |
| RLP | Open data catalogue (CKAN API) | daten.rlp.de | no fire/hospital set found yet |
| Saarland | Fire stations WFS | Saarland geoportal | only if AOI01 overlaps |
| BE | Hospitals list | Belgian Hospital Association directory | no official geo file found; reuse terms unknown |
| BE | Fire stations | none official (OSM wiki: NCCN has no authoritative list) | use OSM |
| All | Hospitals, fire, police, ambulance | OSM (Overpass / ohsome / Geofabrik extract) | ODbL |

## Findings
- No statewide official fire-station layer found for RLP, NRW or Wallonia → **OSM is the only uniform source**.
- Official hospital lists with **bed counts** exist for DE (useful as capacity).
- Overpass test on EMSR517 AOI15 failed: main server 406 (needs User-Agent) then 502; kumi mirror no response. Plan B: Geofabrik `.osm.pbf` per region, filter offline.
- ohsome bbox endpoint drops point (node) elements; use the elements/geometry endpoint for hospitals.
