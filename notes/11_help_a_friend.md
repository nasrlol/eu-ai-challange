# "Help a friend" (built 2026-10-29)

A person with a disability in a risk zone asks for help → 3 closest volunteer helpers by **flood-aware travel time** get an offer → first to accept gets a route + turn-by-turn steps.

## Design decisions (chosen by the user)
| Question | Choice |
|---|---|
| Routing | Own router (`roads.rs`): OSM roads from Geofabrik extracts, Dijkstra; edges with EFAS depth > 0.3 m (scenario, default rp100) ×20 time penalty, so used only if unavoidable → `flooded_m` |
| Sign-up | On demand only; no pre-registry of disabled people |
| Deep water | `water_now: true` → status `escalated`, "call 112", no volunteer |
| Demo | Synthetic, EMSR517 AOI15 (`code/help_demo.py`) |

## API (`flood_api/src/help.rs`, SQLite `out/help.sqlite`)
`POST /help/zones {name,bbox}` · `POST /helpers {zone,lon,lat,mode:walk|bike|car}` · `PATCH /helpers/{id}` · `POST /help-requests {zone,lon,lat,need,urgent?,scenario?,water_now?}` · `POST /help-requests/{id}/accept {helper_id}` · `GET /help-requests/{id}` · `POST /help-requests/{id}/close`
- Tables: zones, helpers, help_requests, assignments. Status: open / assigned / done / escalated / unmatched.
- Privacy: no names; need ∈ mobility/visual/hearing/other; requests + helpers purged after 72 h.
- Zone graph in memory, rebuilt lazily from the stored bbox after a restart.

## Demo result (AOI15, 806 km², 273k nodes, 7,810 km roads)
- Zone build ~30–35 s; a request after that takes < 1 s.
- Request in a cell 3.0 m deep at RP100: best helper by car 1.2 km, 12 min, 319 m of it on roads flooded in the scenario (unavoidable last stretch).
- Escalation and error paths tested.

## Assumptions / limits (for brief + AI sheet)
- 0.3 m flooded threshold, ×20 penalty, car speeds per road type: assumptions, not sourced.
- EFAS = scenario max depth, not live water levels; OSM = today's roads, not 2021.
- `in_risk_zone` = EFAS depth > 0 at the point (not the ML risk score).
- No authentication, no helper vetting, no notifications (demo). Health data = GDPR Art. 9 → real use needs a DPIA, consent, an official operator.
- Volunteers add to 112 / civil protection; they don't replace them.

## Shelters (added 2026-10-29)
- OSM kinds (`osm::shelter_kind`): `shelter` = `social_facility=shelter`, `emergency=assembly_point`; `candidate_shelter` = schools, sports halls / centres (assumption, not designated; capacity unknown). Bus/picnic `amenity=shelter` excluded.
- Also in `/region` facilities. OSM cache now `osm_v2_*` (old caches lack shelters).
- `GET /help/zones/{zone}/shelters?scenario=` → list + `dry` (EFAS depth = 0).
- `GET /help-requests/{id}/shelter?mode=&scenario=&candidates=` → nearest dry shelter by flood-aware route (forward Dijkstra); mode default = accepted helper's, else walk.
- AOI15: 164 shelters (11 OSM shelters, 153 candidates); 30 flood at RP100 (18 %), excluded from routing.
