# Hackathon notes: July 2021 floods (DE / BE / NL)

1. [01_datasets.md](01_datasets.md): what each dataset is, format, resolution, gotchas
2. [02_ems_aois.md](02_ems_aois.md): EMS products, naming, PRODUCT vs MONIT01, which AOIs are best
3. [03_preprocessing.md](03_preprocessing.md): `preprocess.py`, what it keeps/cleans, output layers
4. [04_modelling_plan.md](04_modelling_plan.md): how to train a general model, algorithm choice, pitfalls
5. [05_external_data.md](05_external_data.md): soil / rainfall / elevation (not in the challenge data)
6. [06_rescue_apis.md](06_rescue_apis.md): public APIs for rescue resources + priority/allocation idea
7. [07_regional_help_sources.md](07_regional_help_sources.md): which regions the AOIs cover + regional hospital/fire data sources
8. [08_rust_region_api.md](08_rust_region_api.md): Rust API design, model feature contract, raster layouts, river-source caveat, API data structures (`types.rs`)
9. [09_ai_layer.md](09_ai_layer.md): LLM layer design: facts → rules → Claude phrases → validator; API gaps to fix first
10. [10_todo.md](10_todo.md): open tasks: API fixes, AI layer, Annex checks
11. [11_help_a_friend.md](11_help_a_friend.md): "Help a friend" volunteer matching + flood-aware routing, design choices, limits
12. [12_hosting.md](12_hosting.md): API hosted on valencia (tailnet), service, update steps
13. [13_api_tests.md](13_api_tests.md): API test campaign: who tests what, bugs/gaps, status
14. [14_test_resources.md](14_test_resources.md): where tests run (laptop vs valencia), limits, `code/testrun.sh`
15. [15_frontend_api.md](15_frontend_api.md): wiring the API into the AI-Challenge/ frontend: decisions, status, limits
16. [../code/NOTES.md](../code/NOTES.md): build log of the Python models (prompts, methods, results, bugs, decisions, limits). Steps 1–11 data + first models; 12 restructure + roadmap; 13 OSM layers, `flood_tool.py` (any region), DEM depth check; 14 evacuation levels; 15 precision checks + v2 features (`evaluate.py`); 16 `timeline.py` (flood in time + viewer); 17 `evac_sim.py` (human evacuation flow by hour/weather); 18 API test campaign (CLI references, fixes E1, G4). Code layout: `code/paths.py`, results in `code/outputs/`, caches in `code/cache/`

Convention: we refer to areas by AOI code (e.g. EMSR517 AOI15), not by town name.

## House rules (parallel Claude sessions)
- Record all work in notes/: short bullets in the topical file, index here. Small Edits only, never rewrite another session's text.
- Layout: Python `code/`, outputs `code/outputs/`, caches `code/cache/`, Rust `flood_api/src/`, raw data only in `working_data/`, temp files in scratchpads.
- Owners: Rust `flood_api/src` = hackathon-56, `code/*.py` = cems-efas-da; others report bugs to coordinator hackathon-41.
- Tests: plan, results, bug IDs (B*/E*/G*) only in [13_api_tests.md](13_api_tests.md); test server on valencia :8081, live :8080 untouched; heavy calls under `flock flood_api/out/heavy.lock`.
- Git: commits/pushes by cems-efas-8c only (user request), only owner-verified code; repo is public → no secrets, hosting notes 12/14 held back pending user decision.
