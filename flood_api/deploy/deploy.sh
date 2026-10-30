#!/bin/sh
# Pull-based deploy (valencia): move the checkout to origin/live (CI moves it only after green checks on main),
# rebuild flood_api and restart it. Run by flood-api-deploy.timer every minute. A failed build keeps the old
# binary running and is retried on the next run.
set -eu
cd "${REPO_DIR:-$HOME/hackathon}"
git fetch -q origin live 2>/dev/null || exit 0          # no live branch yet
new=$(git rev-parse FETCH_HEAD)
[ "$(cat .git/deployed 2>/dev/null)" = "$new" ] && exit 0
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    echo "local changes in $(pwd), not deploying $new" >&2; exit 1
fi
git checkout -q -B live "$new"
(cd flood_api && GIT_SHA=$new cargo build --release -q)
sudo -n systemctl restart flood-api
echo "$new" > .git/deployed
echo "deployed $(git log --oneline -1)"
