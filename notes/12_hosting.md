# Hosting on valencia (2026-10-29)

| What | Value |
|---|---|
| Host | `valencia` (Arch, x86_64, 8 cores, 23 GB RAM), Tailscale `valencia.tail142ce3.ts.net` / 100.108.90.108 |
| URL | `http://valencia.tail142ce3.ts.net:8080` (**tailnet only**, not public) |
| Code | `~/hackathon`: git checkout of nasrlol/eu-ai-challange, branch `api-help-shelters-models` (https, read-only: valencia has no GitHub key) |
| Data | all of `working_data/` (18 GB) + `code/cache` + `code/outputs` |
| Service | systemd `flood-api` (`FLOOD_ADDR=100.108.90.108:8080`, restart on failure, starts at boot) |
| Python | `~/hackathon/.venv` via uv, 3.12, versions pinned to the local venv (sklearn 1.9.1 must match `model.joblib`) |
| Rust | Arch `rust` package; build `cd flood_api && cargo build --release` |

- Update: rsync code → `cargo build --release` → `sudo systemctl restart flood-api`. Logs: `journalctl -u flood-api -f`.
- First `/region` in a new area is slow (~110 s, OSM PBF parse); cached after that.
## Test server (:8081, details in [14_test_resources.md](14_test_resources.md))
| What | Value |
|---|---|
| URL | `http://valencia.tail142ce3.ts.net:8081` (tailnet only) |
| Code | `~/hackathon-test/srv`: same git checkout (still refreshed by rsync via `code/testrun.sh`; `working_data`, `.venv` symlinked to `~/hackathon`, in `.git/info/exclude`) |
| Out | `FLOOD_OUT=~/hackathon-test/srv_out` (`help.sqlite`, survives restarts); `code/outputs` seeded from laptop |
| Limits | `systemd-run --user --scope`: 5 GB RAM, no swap, 3 CPUs, nice 10, 12 h timeout; 2 slots via flock |
| History | 15:58 first build (B1/G1/G2), 16:06 restart (B2–B9, E2–E5, G3), hot-sync G4 (no restart), 16:20 final restart under heavy.lock (B10, G5 CORS, G6, E6; empty-named zone deleted; CORS preflight OK) |

- Restart: ask hackathon-11 → `flock flood_api/out/heavy.lock code/testrun.sh --stop srv` → rerun. Python-only fixes: rsync `code/*.py` under the same lock, no restart.
- Live :8080 and the Java server are never touched by tests.

- Not done: public exposure via nginx (no auth on the API; help endpoints hold health-related data → keep tailnet-only unless auth added).

- 2026-10-29 21:53 + 21:58 (cems-efas-72, user OK): live :8080 redeployed with everything up to /region v2, async jobs, DELETE routes, B12. Synced code/notes only (rsync excludes code/outputs, code/cache, flood_api/out → live help.sqlite kept); model/outputs md5 = laptop. Smoke: /health, /models, /impact, CORS, async, DELETE 404, inverted bbox 422.

## Frontend (2026-10-30, cems-efas-72, user request)
| What | Value |
|---|---|
| URL | `http://valencia.tail142ce3.ts.net:5500` (**tailnet only**) |
| Code | `~/frontend` = rsync of laptop `AI-Challenge/` working copy (no .git; never commit/push there) |
| Service | systemd `flood-frontend`: `python3 -m http.server 5500 --bind 100.108.90.108`, restart on failure, starts at boot |
| API | page on valencia defaults to live `:8080` (same host); elsewhere `:8081`; `?api=` overrides |
- Update: (old, before git) rsync; now `git pull` in `~/frontend`, see below.
- Live pre-built for the default study bbox 5.6,50.55,5.96,50.64: help zone `zone_5600_50550_5960_50640`, `/region` `app_5600_50550_5960_50640`.
- 2026-10-30 (user request): `~/frontend` is now a git clone of sistahoppet/AI-Challenge, branch `frontend-perf` (commit 017e2f8). Update: push from laptop → `ssh valencia 'cd ~/frontend && git pull'` (static, no restart). Old rsync copy kept in `~/frontend.rsync-bak`.
- 2026-10-30 02:22: live redeployed with the OSM store (`flood_api/out/osm_store`, 802 MB; rebuild `./target/release/flood_api build-osm-store` after updating extracts, ~2.5 min, otherwise startup says "stale" and falls back to extracts). Test server shares it via symlink.

## Policy advisor API (2026-10-30, user request)
- `http://valencia.tail142ce3.ts.net:8082`, systemd `policy-api`; ollama `qwen3:8b` on 127.0.0.1:11434. Details: [16_policy_api.md](16_policy_api.md).

## Public site dev.nsrddyn.com (2026-10-30, user request)
| What | Value |
|---|---|
| URL | `https://dev.nsrddyn.com` (public): `/` → frontend :5500, `/api/` → live flood_api :8080, `/policy/` → policy_api :8082 |
| nginx | `/etc/nginx/sites/dev.conf`; dev added to port-80 ACME block in `00-services-http.conf` |
| Limits | DELETE → 405; region/flood/evacuation/timeline/help/zones 10/min/IP; `/policy/advise` 4/min/IP (spends server Claude key); 20 r/s/IP overall |
| Cert | own LE cert `dev.nsrddyn.com` (webroot `/srv/http/acme`, auto-renew via certbot-renew.timer) |
| Frontend | AI-Challenge `frontend-perf` ef59419: on host dev.nsrddyn.com the API defaults are `origin/api`, `origin/policy` |
- Live 2026-10-30 ~10:40 (user added DNS A `dev` → 207.180.230.218 in Google Cloud DNS). Smoke OK: `/`, /api/health, /api/models, /policy/health 200; http→https 301; DELETE 405; POST / 403; POST /api/flood, /policy/advise reach the APIs (422 on empty body).
- Help endpoints (help-requests, helpers) are now public with no auth.
- api.dev.nsrddyn.com: staged then dropped (user, 2026-10-30): API stays as paths /api, /policy on dev.nsrddyn.com.

## Server dirs under git (2026-10-30, user request)
| Dir | Repo / branch |
|---|---|
| `~/hackathon`, `~/hackathon-test/srv` | nasrlol/eu-ai-challange `api-help-shelters-models` (git init in place + `reset` to origin, files untouched) |
| `~/frontend` | sistahoppet/AI-Challenge **`main`** (59d2942); `frontend-perf` deleted locally + on valencia (its last commit ef59419 superseded: main defaults to `https://dev.nsrddyn.com/api`) |
| `~/policy_api` | nasrlol/flood-policy-advisor `main` |
- Update: push from laptop → `ssh valencia 'cd <dir> && git pull'` (+ cargo build / restart for Rust). Code = branch; only `notes/` on valencia is older (diff left, not overwritten).
- Smoke after switch: :5500 200, dev.nsrddyn.com 200, /api/health ok, flood-api + policy-api active.
- 2026-10-30 cleanup: challenge annexes, team .docx/.pptx and drive-download zips untracked → local `docs/` (gitignored with `*.zip`); still in git history. Team notebook → `starter_notebook/how_to_get_started_ems.ipynb`.
- Frontend CD (after AI-Challenge#1 merges): `~/frontend` on branch `live`, systemd `frontend-deploy.timer` pulls every minute; no manual `git pull` needed.

## CI/CD for the APIs (2026-10-30, user request)
| API | Workflow | CD unit (valencia) | Verify |
|---|---|---|---|
| flood_api (this repo) | `.github/workflows/flood-api.yml`: cargo build + test, `py_compile code/*.py`, smoke `/health` `/version` `/models` | `flood_api/deploy/flood-api-deploy.{service,timer}` | `dev.nsrddyn.com/api/version` = commit |
| policy_api | `policy_api/.github/workflows/ci.yml`: cargo build + test, smoke `/health` `/version` `/policies` | `policy_api/deploy/policy-api-deploy.{service,timer}` | `dev.nsrddyn.com/policy/version` = commit |
- Same pattern as the frontend: green `main` → branch `live` (GITHUB_TOKEN), valencia pulls `live` every minute, `GIT_SHA=<commit> cargo build --release`, `sudo -n systemctl restart`. Done marker `.git/deployed`; failed build → old binary keeps running, retried next minute. Dirty checkout → refuses (needs `git checkout -- .` once on `~/hackathon`).
- New route `GET /version` in both APIs (`option_env!("GIT_SHA")`, `dev` otherwise).
- Install (once, after merge): `sudo cp <deploy>/*-deploy.{service,timer} /etc/systemd/system/ && sudo systemctl enable --now <name>-deploy.timer`.
- PRs: nasrlol/eu-ai-challange#1 (also merges api-help-shelters-models into main), nasrlol/flood-policy-advisor#1. CI green on both (tests 4 + 3, smoke OK). Install timers after merge.
- 2026-10-30 ~13:10 (user: "commit and push to the server"): live `~/hackathon` switched to branch `frontend-asks` (2244e16; server-local old notes kept in `git stash` "pre-deploy server-local notes 2026-10-30"), `GIT_SHA=… cargo build --release`, `systemctl restart flood-api`. Ran `region.py` (study_region.geojson) + `export_geojson.py`; `osm_data.py --extra` started in the background (log `flood_api/out/osm_extra.log`) for `/layers`. nginx: `layers` added to the heavy-route rate limit (backup `/etc/nginx/sites/dev.conf.bak-20261030`), `nginx -t` OK, reloaded.
- Smoke on dev.nsrddyn.com: /version = 2244e16; /models lists layers; Q8 bbox /region 57,749 cells; /region aoi EMSR517_AOI18_GRA 26.7 km² 2,666 cells; /timeline cells_geojson; /flood ref rp100 449 / 522 / 22.94 km (= CLI reference), reuse → `reused: true`; `/`, /policy/health 200. `/layers` not run (user: skip). CD timers still not installed (PR #1 not merged).
- 2026-10-30 13:00 (user: "pull everything and update"): `~/frontend` → AI-Challenge main 93d4764 (21 commits: 19 AOIs via ?aoi=, shelter recommendations, simpler screen, CI/CD), flood-frontend restarted, served files = GitHub main. `~/policy_api` → main b9b58ee (CI/CD merge, GET /version), rebuilt + restarted, /policy/version OK, server key kept. `~/hackathon-test/srv` → 6102db6 (notes only; old notes in `git stash`; test server not restarted). `~/hackathon` unchanged on `frontend-asks` (main only adds the PR #1 merge commit; switching to main would drop /layers etc.). Laptop: AI-Challenge + policy_api on main, up to date. Deploy timers still not installed: install only after PR #2 is in main (hackathon `live` = 49f891d lacks frontend-asks).
- 2026-10-30: `osm_data.py --extra` done on live: cache layers places 13,096, shelters 17,055 → `/layers` no longer needs the first-call build.
