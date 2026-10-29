# Test resources (2026-10-29)

| Machine | RAM free | Use for |
|---|---|---|
| laptop | ~0.9 GB (swap 5.6/7.3 GB in use) | light curl calls only; no builds, no first-time `/region` |
| valencia | ~13 GB, 8 cores, 356 GB disk | all heavy tests; leave the Java server (8 GB) and live `flood-api` :8080 alone |

- Runner: `code/testrun.sh <name> <cmd...>`: rsyncs the repo to `valencia:~/hackathon-test/<name>` (symlinks working_data + .venv), runs `<cmd>` there.
- Limits: 2 slots (flock), each 5 GB RAM, no swap, 3 CPUs, nice 10, 2 h timeout. `TEST_PORT` = 8081/8082 by slot.
- `code/testrun.sh --status` shows the slots; `--pull <name> <dir>` copies remote `code/outputs`.
- Rule: one heavy request (new-area `/region`, `/flood`, `/timeline`, builds) at a time across all sessions.
