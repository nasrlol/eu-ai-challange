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
12. [../code/NOTES.md](../code/NOTES.md): build log of the Python models (prompts, methods, results, bugs), final analysis and roadmap for arbitrary floods (step 12). Code layout: `code/paths.py`, results in `code/outputs/`, caches in `code/cache/`

Convention: we refer to areas by AOI code (e.g. EMSR517 AOI15), not by town name.
