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
- Not done: public exposure via nginx (no auth on the API; help endpoints hold health-related data → keep tailnet-only unless auth added).
