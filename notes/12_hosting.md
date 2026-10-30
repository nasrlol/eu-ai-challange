# Hosting on valencia (2026-10-29)

| What | Value |
|---|---|
| Host | `valencia` (Arch, x86_64, 8 cores, 23 GB RAM), Tailscale `valencia.tail142ce3.ts.net` / 100.108.90.108 |
| URL | `http://valencia.tail142ce3.ts.net:8080` (**tailnet only**, not public) |
| Code | `~/hackathon` (rsync copy of local tree, not a git clone; last sync ~15:45, before /evacuation) |
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
| Code | `~/hackathon-test/srv` (rsync of laptop tree via `code/testrun.sh`; `working_data`, `.venv` symlinked to `~/hackathon`) |
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
