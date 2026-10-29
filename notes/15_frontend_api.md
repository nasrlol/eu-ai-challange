# Frontend ↔ API integration (hackathon-e9, 2026-10-29)

Frontend: `AI-Challenge/`, a separate git repo (origin sistahoppet/AI-Challenge).
- **Never commit there.** It is in this repo's `.gitignore`.
- It is a static MapLibre app. Its JS modules are classic scripts, loaded in the order set in `index.html`.
- Today it reads `data/*.geojson` (made by `scripts/export_*.py`). `build.py` turns it into one offline HTML file in `dist/`.

## Status
- **Waiting:** implementation starts once the test campaign in notes/13 is closed. hackathon-41 will signal it.
- **Nothing built yet.** Files edited so far: none.

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
- `evacuation_*.csv` gives lat/lon rounded to 0.1°, which is not usable for map geometry. Use it for rankings and numbers only.
- Testing rules:
  - Test server is `:8081`. Never call live `:8080`.
  - Heavy POSTs go through `flock flood_api/out/heavy.lock`. Run names are `e9_*`.
  - Browser tests stay on light or cached calls until the campaign ends.
