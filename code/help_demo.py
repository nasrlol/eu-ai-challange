"""Synthetic "Help a friend" demo on EMSR517 AOI15: register helpers where people live, send help
requests from cells flooded in the EFAS RP100 scenario, accept the best offer and save the route.
All people here are generated; no real personal data.  Usage: help_demo.py [api_url]"""
import json, sys, urllib.request
import numpy as np, pandas as pd
from pathlib import Path

API = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8080"
ROOT = Path(__file__).resolve().parent.parent
CELLS = ROOT / "flood_api/out/EMSR517_AOI15_test/features.csv"  # written by POST /region
OUT = Path(__import__('os').environ.get('HELP_DEMO_OUT', ROOT / 'flood_api/out/help_demo'))
N_HELPERS, SEED = 40, 0


def call(method, path, body=None):
    req = urllib.request.Request(API + path, method=method, data=json.dumps(body).encode() if body else None,
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=600) as r:
            return json.load(r)
    except urllib.error.HTTPError as e:
        sys.exit(f"{method} {path}: {e.code} {e.read().decode()}")


rng = np.random.default_rng(SEED)
cells = pd.read_csv(CELLS)
jitter = lambda n: rng.uniform(-0.0006, 0.0006, (n, 2))  # within a 100 m cell

zone = "EMSR517_AOI15"
print(call("POST", "/help/zones", {"name": zone, "bbox": [cells.lon.min(), cells.lat.min(), cells.lon.max(), cells.lat.max()]}))

# Helpers where people live, mostly by car
homes = cells.sample(N_HELPERS, weights=cells.pop_density.clip(lower=0), random_state=SEED)
modes = rng.choice(["car", "bike", "walk"], N_HELPERS, p=[0.6, 0.2, 0.2])
for (lon, lat), m in zip(homes[["lon", "lat"]].values + jitter(N_HELPERS), modes):
    call("POST", "/helpers", {"zone": zone, "lon": lon, "lat": lat, "mode": m})

# Requests from inhabited cells flooded at RP100; the last one already has water at the door
risky = cells[(cells.rp100 > 0) & (cells.pop_density > 0)].sample(3, random_state=SEED)
pts = risky[["lon", "lat"]].values + jitter(3)
needs = ["mobility", "visual", "mobility"]
reqs = [call("POST", "/help-requests", {"zone": zone, "lon": lon, "lat": lat, "need": n, "water_now": i == 2})
        for i, ((lon, lat), n) in enumerate(zip(pts, needs))]
for r in reqs:
    print(json.dumps({k: r.get(k) for k in ["id", "status", "risk", "offers", "message"]}))

# Best helper accepts the first request
first = reqs[0]
if not first["offers"]:
    sys.exit("no offers for the first request")
acc = call("POST", f"/help-requests/{first['id']}/accept", {"helper_id": first["offers"][0]["helper_id"]})
route = acc["route"]
OUT.mkdir(parents=True, exist_ok=True)
fc = {"type": "FeatureCollection", "features": [
    {"type": "Feature", "geometry": route["geometry"], "properties": {k: route[k] for k in ["eta_s", "distance_m", "flooded_m"]}},
    {"type": "Feature", "geometry": {"type": "Point", "coordinates": list(pts[0])}, "properties": {"role": "person"}},
]}
(OUT / "route.geojson").write_text(json.dumps(fc))
print(f"route: {route['distance_m']:.0f} m, {route['eta_s'] / 60:.1f} min, flooded {route['flooded_m']:.0f} m -> {OUT / 'route.geojson'}")
for s in route["steps"]:
    print(f"  {s['instruction']} ({s['distance_m']:.0f} m)")
call("POST", f"/help-requests/{first['id']}/close")
