# EU AI Challenge — Flood Simulator

Backend API and modeling pipeline for the July 2021 Central European floods (DE / BE / NL), combining Copernicus EMS / EFAS / GHSL / OSM datasets for region-level flood risk, road damage, evacuation simulation, and volunteer routing.

## Repository Structure

```text
.
├── flood_api/          # Rust HTTP API server (Axum) and deployment units
│   ├── src/            # Region feature extraction, OSM store, routing, model endpoints
│   └── deploy/         # Pull-based CD script and systemd service/timer (valencia)
├── models/             # Python modeling, preprocessing, and simulation CLIs
│   ├── paths.py        # Shared data / cache / outputs path configuration
│   ├── risk_model.py   # Flood susceptibility model & scoring (score_region.py)
│   ├── flood_tool.py   # Any-region flood impact analysis
│   ├── timeline.py     # Time-stepped flood progression & viewer
│   ├── evac_sim.py     # Human evacuation flow simulation
│   ├── road_model.py   # Road exposure, damage validation, closure timeline
│   ├── interp_model.py # Flood extent interpolation between CEMS observations
│   ├── area_layers.py  # Buildings, towns, and shelters GeoJSON layers
│   └── NOTES.md        # Python modeling build log
├── notes/              # Architecture, API contracts, hosting, and handoff docs
│   └── README.md       # Index of all notes
└── .github/workflows/  # CI/CD (build, test, py_compile, smoke test -> branch live)
```

### Ignored / Server-Local Directories

These directories are excluded from git (see `.gitignore`):

- `working_data/` — challenge datasets, DEM, and OSM extracts
- `models/outputs/` — trained models (`risk/model.joblib`), study region hull, and generated layers
- `models/cache/` — recomputable intermediate tables
- `flood_api/out/` — per-request API outputs, SQLite store, and reuse cache
- `flood_api/target/` — Rust build artifacts

## Quick Checks

```bash
# Rust build & tests
cd flood_api && cargo test --release --locked

# Python syntax check
python -m py_compile models/*.py
```

## Documentation

See [notes/README.md](notes/README.md) for the full documentation index, including API design (`notes/08_rust_region_api.md`), frontend handoff (`notes/17_frontend_handoff.md`), and hosting/deployment (`notes/12_hosting.md`).
