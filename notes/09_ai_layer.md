# AI analysis layer on top of the region API (design, 2026-10-29, not built)

Rule: **the LLM never produces a number or an action.** Rust/Python compute facts + rule-based recommendations; the LLM only selects, orders and phrases them, citing fact ids. A validator rejects anything else → fallback template.

## Fix first (API gaps that would mislead an LLM)
| Gap | Fix |
|---|---|
| `expected_people_flooded_approx` = Σ risk×people, but risk is a ranking, not a probability | drop or rename "risk-weighted population index" |
| ad-hoc `json!` response | wire `types.rs` `RegionResponse` (caveats, sources) |
| `factors` / `overall` not computed in API | compute flooded_pct, people, likelihood; others `NotAvailable` |
| no confidence signal | OOD check: % cells with features outside training 1–99 pct (`code/cache/risk_features_osm.pkl`), river_points, nearest training AOI |
| `ScenarioSummary` empty, top_n hard-coded 20 | fill from rp10/100/500, use `options.top_n` |
| facility distance = straight line | label as such; travel time later (OSRM) |

## Pipeline
1. `RegionResponse` → `facts`: flat list `{id, text, value, unit, source, origin}`.
2. Rules → candidate recs `{id, action, trigger, evidence:[fact ids]}` from a closed catalogue.
3. Claude (one call, facts only, ~2–4k tokens; never grids) → JSON: `statements[{kind: fact|inference|recommendation|unknown, text, evidence[]}]`.
4. Validator: every number in text ∈ cited facts (rounding tolerance); every rec id ∈ candidates; every evidence id exists; no "probability"/"will flood" wording. Fail → template text, flag `ai_rejected`.
5. Log prompt + output + validation in `out/<name>/ai_log.json` (AI sheet, Annex 7).

## Known model limits to pass as facts
- Held-out ROC AUC weak in EMSR517 AOI20 (0.45), AOI09 (0.63), AOI07 (0.67); median ~0.93.
- OSM rivers ≠ CEMS hydrography; GHS-POP 1 km; OSM facilities = today, not 2021.
