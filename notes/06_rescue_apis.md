# Rescue resources & allocation (idea, not built yet)

Goal: rank areas by danger, then see where rescue resources are far away → allocate.
Check first: external data allowed? (Annex 3/4). Label it as external in the brief.

## APIs (unverified, check limits/terms)
| Need | API |
|---|---|
| Fire stations, hospitals, police, shelters, helipads | OSM Overpass (no key) |
| Same, as of 2021-07-14 | ohsome API (OSM history) |
| Health facilities | Healthsites.io (key), Eurostat GISCO healthcare (download) |
| Travel time / isochrones | OpenRouteService (key), OSRM (no key) |
| Alerts | GDACS, Copernicus EMS feed, NINA/bund.dev (DE), Meteoalarm |
| River levels / discharge | Pegelonline (DE), Rijkswaterstaat Waterinfo (NL), hydrometrie.wallonie.be (BE), Open-Meteo Flood |

## Pipeline
1. Risk per cell: flooded buildings × EFAS depth × GHS-POP (+ damage model on EMSR517 AOI15).
2. Resources: Overpass/ohsome inside `aoi_boundary` + buffer.
3. Access: travel time nearest station → cell; drop roads damaged in `transportationL`.
4. Priority = risk × travel time → ranked list for a named actor (e.g. crisis centre).

Keys out of code. Query one AOI at a time (Overpass fair use).
