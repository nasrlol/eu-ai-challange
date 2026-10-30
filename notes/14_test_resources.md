# Test resources (2026-10-29)

| Machine | RAM free | Use for |
|---|---|---|
| laptop | ~0.9 GB (swap 5.6/7.3 GB in use) | light curl calls only; no builds, no first-time `/region` |
| valencia | ~13 GB, 8 cores, 356 GB disk | all heavy tests; leave the Java server (8 GB) and live `flood-api` :8080 alone |

- Runner: `code/testrun.sh <name> <cmd...>`: rsyncs the repo to `valencia:~/hackathon-test/<name>` (symlinks working_data + .venv), runs `<cmd>` there.
- Limits: 2 slots (flock), each 5 GB RAM, no swap, 3 CPUs, nice 10, 2 h timeout (`TESTRUN_TIMEOUT` overrides). `TEST_PORT` = 8081/8082 by slot.
- `code/testrun.sh --status` shows the slots; `--pull <name> <dir>` copies remote `code/outputs`; `--stop <name>` kills a job.
- Rule: one heavy request (new-area `/region`, `/flood`, `/timeline`, builds) at a time across all sessions.

## Test server (owner: resource session)
- `http://valencia.tail142ce3.ts.net:8081`: slot `srv`, `FLOOD_OUT=~/hackathon-test/srv_out`, 12 h timeout, outputs seeded from laptop `code/outputs`.
- Restart: `code/testrun.sh --stop srv`, then rerun the same `srv` command (full cargo rebuild ~4 min).
- Heavy calls: `flock flood_api/out/heavy.lock curl ...` from the laptop, one at a time across all sessions.
- Restart protocol: ask hackathon-11 (help-data persistence) first, then `flock flood_api/out/heavy.lock code/testrun.sh --stop srv` and rerun. `help.sqlite` sits in `srv_out`, so it survives restarts.
- Python-only fixes: `flock flood_api/out/heavy.lock rsync -a code/*.py valencia:hackathon-test/srv/code/`, no restart (done for G4, plus `eval/api_reference.json`).
- Open: the 16:06 restart ran before the protocol existed (no lock, no hackathon-11 check), so in-flight heavy calls were killed.
- 16:20 final restart followed the protocol (heavy.lock 16:19:58–16:20:46); deleted the empty-named zone (0 deps); OPTIONS preflight returns `access-control-allow-origin: *`.
