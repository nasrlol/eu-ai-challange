# Policy advisor API (2026-10-30, user request)

Separate repo **github.com/nasrlol/flood-policy-advisor** (private), local copy `policy_api/` (ignored by this repo's `.gitignore`). Rust/axum.

- **Input:** the flood_api results the frontend already has (`/flood`, `/evacuation`, `/timeline`, `/region`, `/impact`, `/models`), sent unchanged.
- **Output:** 3–6 policies from our closed catalogue, in order, each with `now` / `soon` / `later`, a plain reason, and the model facts it used.
- **Catalogue:** `src/policies.rs`, 14 policies (ST-ALERT, ST-PREP, ST-AI, ST-CRISIS, ST-MITIG, ST-COMMS, ST-INCLUSIVE, ST-CAREHOMES, ST-EVACAPP, LT-ZONING, LT-RECOVERY, LT-AWARE, LT-SEWAGE, LT-RETENTION). Sources: Policy Brief + Policies since the flood 2021.
- **Pipeline:** facts (ids like `flood.rp100.people_leave_now`) → rule triggers → LLM → validator.
  - The validator checks that policy ids come from an enum, that evidence ids exist, that every number appears in the facts or the catalogue, and bans "will flood".
  - If validation fails, the rule-based answer is returned and `ai_rejected` says why.
  - Same design as [09_ai_layer.md](09_ai_layer.md). LLM log: `out/ai_log.jsonl` (for the AI sheet). The key is never logged.
- **Provider per request:** `provider` = `claude` | `local` | `rules`.
  - Claude key: from the frontend in the header `x-anthropic-key` (that request only), else the server env.
  - Model `claude-opus-5-5`, effort medium, `fallbacks: "default"`.
- **Routes:** `GET /health`, `GET /policies`, `POST /advise` (`?async=1` → `GET /jobs/{id}`).

## Hosting (valencia, tailnet only)
| What | Value |
|---|---|
| URL | `http://valencia.tail142ce3.ts.net:8082` |
| Code | `~/policy_api` (rsync from laptop; valencia has no GitHub access to the private repo) |
| Service | systemd `policy-api`, settings in `/etc/policy-api.env` (root, 600): `ANTHROPIC_API_KEY` (set 2026-10-30), `POLICY_ADDR`, `POLICY_LOCAL_MODEL=qwen3:8b`, `POLICY_TIMEOUT_S=1800` |
| Local LLM | ollama (pacman), 127.0.0.1:11434, `qwen3:8b`, keep-alive 10 min (8 GB RAM while loaded) |
- Update: rsync → `cargo build --release` → `sudo systemctl restart policy-api`.
- Add a key: edit `/etc/policy-api.env` → restart `policy-api`.

## Local model verdict
- Not good enough for interactive use.
- Speed on the CPU-only VM (Broadwell, 8 vCPU): prompt ~9 tok/s, output ~2–4 tok/s. One answer (~5k-token prompt) takes 10–15+ min.
- **Minecraft server left running** (user: stop it only if the local model is strong enough). It barely uses CPU, so stopping it would not help.
- Full run through the service (03:00): **1173 s (19.5 min)**. It passed the validator.
  - Order was sensible: ALERT, EVACAPP, PREP, INCLUSIVE, ZONING, MITIG.
  - But it mixed scenarios: 8,148 cut-off buildings (1-in-100 run) presented as 1-in-500.
  - Some evidence ids were wrong (police stations cited for inclusive evacuation).
- Local stays selectable as a slow async option. Rules answer instantly.

## Tests (2026-10-30)
- `cargo test`: validator, 3 passed.
- Real study-bbox results from :8081 (`pol_test` + `app_5600_…`): 66 facts.
  - Rules: ST-ALERT, ST-EVACAPP, ST-PREP, ST-INCLUSIVE, LT-ZONING, ST-MITIG.
- Claude with a fake key → 401 → rules fallback with the reason. Claude without a key → 400. CORS preflight with `x-anthropic-key` OK.
- Real Claude (2026-10-30, server key, study bbox): 15 s, passed the validator. Order ST-ALERT, ST-PREP, ST-EVACAPP, ST-CAREHOMES, ST-INCLUSIVE, LT-ZONING; evidence and 2021 lessons correct. Default provider is now Claude.
