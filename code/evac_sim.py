"""Human evacuation flow ("the human flood"): who leaves, by which roads, how congested, how long.

For a region, a flood stage, an hour of the day, weekday/weekend and the weather:
  1. Evacuees per building = occupants (GHS-POP) x share at home at that hour x compliance
     (by evacuation category from access_model: now / early / prepare). Evacuees with no usable
     road route to any safe place are counted as boat / air.
  2. Mode: cars = evacuees x car share / persons per car; the rest need organised transport.
  3. Destination: the nearest safe road node by travel time (dry, >= SAFE_M from any flooded
     road), or a dry road leaving the region.
  4. Routes on the roads still usable (flooded stretches and flooded bridges closed); every car
     is loaded onto its route. Per road: cars, demand per hour over the departure window,
     capacity left after background traffic at that hour and the weather, congestion level,
     delay (BPR curve) and time to clear the queue.
  5. Per building: travel time + worst bottleneck → time to reach safety; per 1 km cell and region.
All behaviour numbers are assumptions (constants below); there is no evacuation data to fit them.

  python3 evac_sim.py --region 6.9,50.45,7.15,50.58 --flood T=500 --roads-flood none --hours 3 8 17 --weather heavy_rain --name ahr_evac

Writes outputs/evacuations/<name>/: summary.json, roads_<hour>.gpkg, cells_<hour>.gpkg, viewer.html.
"""
import argparse, json, sys
import geopandas as gpd, numpy as np, pandas as pd, shapely
from scipy.sparse import coo_matrix
from scipy.sparse.csgraph import dijkstra
from scipy.spatial import cKDTree

import access_model, flood_tool, region as study
from paths import OUTPUTS
from risk_model import Area

OUT = OUTPUTS / "evacuations"
SAFE_M = 500                 # a destination must be this far from any flooded road
COMPLIANCE = {"now": 0.9, "early": 0.6, "prepare": 0.15}   # share who actually leave (prepare = voluntary "shadow" evacuation)
PERSONS_PER_CAR = 2.5
CAR_SHARE = 0.85             # evacuees who leave by car; the rest need organised transport
SPEED_KMH = {"Highway": 100, "Primary road": 70, "Secondary road": 60, "Local road": 40}
CAPACITY_VPH = {"Highway": 3600, "Primary road": 1500, "Secondary road": 1100, "Local road": 600}
WEATHER = {  # capacity factor, speed factor
    "dry": (1.0, 1.0), "rain": (0.92, 0.9), "heavy_rain": (0.8, 0.8), "fog": (0.85, 0.75), "snow": (0.7, 0.6)}
LEVELS = [(0.6, "free"), (0.9, "busy"), (1.2, "congested"), (np.inf, "overloaded")]


def home_share(hour, weekday):
    """Share of residents at home (assumed daily profile; GHS-POP counts where people live)."""
    if hour < 6 or hour >= 22:
        return 0.95
    if hour < 9:
        return 0.7 if weekday else 0.9
    if hour < 16:
        return 0.45 if weekday else 0.7
    if hour < 19:
        return 0.65 if weekday else 0.75
    return 0.85


def background(hour, weekday):
    """Share of road capacity already used by normal traffic (assumed)."""
    if hour < 6 or hour >= 22:
        return 0.05
    if weekday and (7 <= hour < 9 or 16 <= hour < 19):
        return 0.5
    return 0.3 if 9 <= hour < 16 else 0.2


def departure_window_h(hour):
    """Hours over which evacuees leave after the order (slower at night: warning reaches fewer people)."""
    return 3.0 if hour < 6 or hour >= 22 else 2.0


def simulate(ctx, st, hour, weekday=True, weather="dry", closed=None):
    """`st` = the flood people evacuate from (who leaves, where is safe); `closed` = roads under water
    while they leave (default: the same flood, i.e. evacuating at its peak)."""
    net, node, occupants = ctx["net"], ctx["node"], ctx["occupants"]
    cap_f, speed_f = WEATHER[weather]
    if hour < 6 or hour >= 22:
        speed_f *= 0.9  # darkness
    n_nodes = len(net.xy)
    target_wet = st["edge_wet"]
    usable = ~(target_wet if closed is None else closed)

    share = home_share(hour, weekday)
    evac = occupants * share * np.select([st["must"], st["early"], st["prepare"]],
                                         [COMPLIANCE["now"], COMPLIANCE["early"], COMPLIANCE["prepare"]], 0)

    # 3. Safe destinations: nodes not touching a flooded road and >= SAFE_M from one, or dry exits
    touches = np.zeros(n_nodes, bool)
    touches[net.a[target_wet]] = touches[net.b[target_wet]] = True
    if target_wet.any():
        far = cKDTree(net.mid[target_wet]).query(net.xy)[0] >= SAFE_M
    else:
        far = np.ones(n_nodes, bool)
    safe = np.flatnonzero((far & ~touches) | np.isin(np.arange(n_nodes), ctx["entry"]) & ~touches)

    # 4. Fastest routes to the nearest safe node (free-flow time), as a shortest-path tree
    speed = np.array([SPEED_KMH.get(c, 40) for c in net.road_class]) * speed_f
    t0 = net.length / 1000 / speed  # hours
    g = coo_matrix((t0[usable], (net.a[usable], net.b[usable])), shape=(n_nodes, n_nodes)).tocsr()
    dist, pred, _ = dijkstra(g, directed=False, indices=safe, min_only=True, return_predecessors=True)
    reach = np.isfinite(dist)

    # 1-2. Who can drive out (a route to safety exists from their road), cars and transport needs
    near_road = ctx["snap"] <= access_model.SNAP_M
    can_go = near_road & reach[node]
    by_air = evac * ~can_go           # no usable road route to any safe place
    road = evac * can_go
    cars = road * CAR_SHARE / PERSONS_PER_CAR
    transit = road * (1 - CAR_SHARE)

    # Edge index for each (node, predecessor) pair
    keys = np.minimum(net.a, net.b).astype(np.int64) * n_nodes + np.maximum(net.a, net.b)
    order_k = np.argsort(keys)
    def edge_of(u, v):
        k = np.minimum(u, v).astype(np.int64) * n_nodes + np.maximum(u, v)
        return order_k[np.searchsorted(keys[order_k], k)]

    # Load cars onto the tree: push flow from far nodes towards their destination
    flow_node = np.bincount(node, weights=cars, minlength=n_nodes)
    has_pred = reach & (pred >= 0)
    order = np.flatnonzero(has_pred)[np.argsort(-dist[has_pred])]
    edge_flow = np.zeros(len(net.length))
    pe = edge_of(order, pred[order])
    for n, e in zip(order, pe):  # far to near: each node passes its accumulated cars to its predecessor
        edge_flow[e] += flow_node[n]
        flow_node[pred[n]] += flow_node[n]

    window = departure_window_h(hour)
    cap = np.array([CAPACITY_VPH.get(c, 600) for c in net.road_class]) * cap_f * (1 - background(hour, weekday))
    vc = edge_flow / window / cap
    clear_h = edge_flow / cap                       # time to push all cars through this stretch
    t_edge = t0 * (1 + 0.15 * np.minimum(vc, 3) ** 4)

    # 5. Time to safety per node: travel time + worst bottleneck on the way (near to far)
    tt, bott = np.zeros(n_nodes), np.zeros(n_nodes)
    for n, e in zip(order[::-1], pe[::-1]):
        tt[n] = tt[pred[n]] + t_edge[e]
        bott[n] = max(bott[pred[n]], clear_h[e])
    t_safe = np.where(can_go, tt[node] + bott[node], np.nan)

    used = edge_flow > 0
    level = np.full(len(vc), "", object)
    for hi, name in reversed(LEVELS):
        level[used & (vc < hi)] = name
    status = np.where(~usable, "flooded (closed)", np.where(used, "used", "usable"))
    roads = dict(status=status, cars=edge_flow, vc=vc, level=level, clear_h=clear_h, delay_factor=t_edge / np.maximum(t0, 1e-9))
    bld = dict(evacuees=evac, cars=cars, transit=transit, by_air=by_air, t_safe=t_safe)
    w = np.where(np.isfinite(t_safe), cars, 0)
    finish = np.nan if w.sum() == 0 else float(np.quantile(np.repeat(t_safe[w > 0], np.ceil(w[w > 0]).astype(int)), 0.95))
    summary = dict(hour=hour, weekday=weekday, weather=weather, home_share=share, background_traffic=background(hour, weekday),
                   departure_window_h=window, evacuees=float(evac.sum()), cars=float(cars.sum()), need_transport=float(transit.sum()),
                   boat_or_air=float(by_air.sum()), already_safe=float(road[np.isin(node, safe)].sum()),
                   safe_destinations=int(len(safe)), roads_used_km=float(net.length[used].sum() / 1000),
                   roads_closed_km=float(net.length[~usable].sum() / 1000),
                   roads_congested_km=float(net.length[used & (vc >= 0.9)].sum() / 1000),
                   time_to_safety_p95_h=None if np.isnan(finish) else round(departure_window_h(hour) + finish, 2))
    return roads, bld, summary


VIEW_MIN_CARS = 5  # viewer only shows road stretches carrying at least this many cars (the .gpkg keeps all)


def _slim(gdf):
    """For the viewer: lon/lat rounded to ~1 m."""
    g = gdf.to_crs(4326)
    return g.set_geometry(shapely.set_precision(g.geometry.values, 1e-5))


def _view_roads(r):
    """Viewer layer: stretches with >= VIEW_MIN_CARS cars, consecutive pieces with the same class,
    level and car count merged into one line."""
    v = r[r.cars >= VIEW_MIN_CARS].copy()
    v["cars"] = v.cars.round()
    v = v.dissolve(by=["road_class", "level", "cars"], aggfunc={"vc": "max", "clear_h": "max"}).reset_index()
    v["geometry"] = shapely.line_merge(v.geometry.values)
    return _slim(v)[["road_class", "cars", "vc", "level", "clear_h", "geometry"]].round(3)


def edges_gdf(net, roads, keep):
    lines = shapely.linestrings(np.stack([net.p[keep], net.q[keep]], axis=1))
    df = pd.DataFrame({k: v[keep] for k, v in roads.items()})
    df["road_class"] = net.road_class[keep]
    df["length_m"] = net.length[keep]
    return gpd.GeoDataFrame(df, geometry=lines, crs=3035)


def cells_gdf(ctx, bld):
    df = pd.DataFrame({k: v for k, v in bld.items()}, index=ctx["cell"])
    df["t_x_cars"] = np.nan_to_num(df.t_safe) * df.cars
    g = df.groupby(level=[0, 1])
    c = pd.DataFrame(dict(evacuees=g.evacuees.sum(), cars=g.cars.sum(), need_transport=g.transit.sum(),
                          boat_or_air=g.by_air.sum(), time_to_safety_h=g.t_x_cars.sum() / g.cars.sum().replace(0, np.nan)))
    c = c[c.evacuees >= 1].reset_index(names=["cx", "cy"])
    M = access_model.CELL_M
    return gpd.GeoDataFrame(c.drop(columns=["cx", "cy"]), crs=3035,
                            geometry=shapely.box(c.cx * M, c.cy * M, (c.cx + 1) * M, (c.cy + 1) * M))


VIEWER = """<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Evacuation Flow</title>
<link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/leaflet/1.9.4/leaflet.min.css">
<script src="https://cdnjs.cloudflare.com/ajax/libs/leaflet/1.9.4/leaflet.min.js"></script>
<style>
:root{--bg:#f6f7f9;--panel:#fff;--ink:#1b1f24;--muted:#5b6670;--rule:#d5dbe1;--accent:#1f5f8b}
@media (prefers-color-scheme:dark){:root{--bg:#121417;--panel:#1b1f24;--ink:#e8ecf0;--muted:#9aa5b1;--rule:#2c333b;--accent:#6aa9d8}}
*{box-sizing:border-box}body{margin:0;font:14px/1.45 system-ui,sans-serif;background:var(--bg);color:var(--ink)}
header{padding:12px 16px;border-bottom:1px solid var(--rule)}h1{font-size:18px;margin:0}header p{margin:2px 0 0;color:var(--muted);font-size:12px}
main{display:grid;grid-template-columns:1fr 360px;height:calc(100vh - 62px)}#map{height:100%}
aside{background:var(--panel);border-left:1px solid var(--rule);padding:14px 16px;overflow:auto}
@media (max-width:800px){main{grid-template-columns:1fr;height:auto}#map{height:60vh}aside{border-left:0;border-top:1px solid var(--rule)}}
select{width:100%;padding:6px;margin-bottom:10px;background:var(--panel);color:var(--ink);border:1px solid var(--rule);border-radius:4px}
.kpis{display:grid;grid-template-columns:1fr 1fr;gap:8px;margin:8px 0}.kpi{border:1px solid var(--rule);border-radius:6px;padding:6px 8px}
.kpi span{display:block;color:var(--muted);font-size:11px}.kpi b{font-size:16px}.legend span{display:inline-block;width:18px;height:4px;margin-right:4px;vertical-align:middle}
.note{color:var(--muted);font-size:11px;margin-top:10px}
</style></head><body>
<header><h1>Evacuation flow: __NAME__</h1><p>__FLOOD__ · cars routed to the nearest safe area on the roads still usable</p></header>
<main><div id="map"></div><aside>
<label for="run">Time of day / weather</label><select id="run"></select>
<div class="kpis" id="kpis"></div>
<div class="legend"><b>Roads</b><br><span style="background:#2b83ba"></span>flooded (closed)<br><span style="background:#1a9641"></span>free <span style="background:#fdae61"></span>busy
<span style="background:#d7191c"></span>congested <span style="background:#7b0019"></span>overloaded<br><b>Cells</b>: shading = evacuees; click for details</div>
<p class="note">__ASSUMPTIONS__</p></aside></main>
<script>
const D=__DATA__;
const map=L.map('map').fitBounds([[D.bounds[1],D.bounds[0]],[D.bounds[3],D.bounds[2]]]);
L.tileLayer('https://tile.openstreetmap.org/{z}/{x}/{y}.png',{maxZoom:18,attribution:'© OpenStreetMap'}).addTo(map);
L.geoJSON(D.closed,{style:{color:'#2b83ba',weight:3,opacity:.8}}).addTo(map);
const lvl={free:'#1a9641',busy:'#fdae61',congested:'#d7191c',overloaded:'#7b0019'};
let layer=L.layerGroup().addTo(map);const sel=document.getElementById('run');
D.runs.forEach((r,i)=>sel.add(new Option(r.label,i)));
function show(i){const r=D.runs[i];layer.clearLayers();const mx=Math.max(1,...r.cells.features.map(f=>f.properties.evacuees));
 L.geoJSON(r.cells,{style:f=>({color:'#555',weight:.4,fillColor:'#6a51a3',fillOpacity:.1+.5*f.properties.evacuees/mx}),
  onEachFeature:(f,l)=>{const p=f.properties;l.bindPopup(`<b>${Math.round(p.evacuees)} evacuees</b><br>${Math.round(p.cars)} cars · ${Math.round(p.need_transport)} need transport · ${Math.round(p.boat_or_air)} boat/air<br>time to safety ≈ ${p.time_to_safety_h==null?'n/a':p.time_to_safety_h.toFixed(1)+' h'}`)}}).addTo(layer);
 L.geoJSON(r.roads,{style:f=>({color:lvl[f.properties.level]||'#888',weight:1.5+Math.min(5,Math.sqrt(f.properties.cars)/6),opacity:.9}),
  onEachFeature:(f,l)=>l.bindTooltip(`${f.properties.road_class}: ${Math.round(f.properties.cars)} cars, load ${f.properties.vc.toFixed(2)} (${f.properties.level}), queue clears in ${(f.properties.clear_h*60).toFixed(0)} min`)}).addTo(layer);
 const s=r.summary,f=x=>Math.round(x).toLocaleString();
 const k=[['Evacuees',f(s.evacuees)],['Cars',f(s.cars)],['Need transport',f(s.need_transport)],['Boat / air only',f(s.boat_or_air)],['Already safe',f(s.already_safe)],['At home now',Math.round(s.home_share*100)+'%'],['Roads used',s.roads_used_km.toFixed(1)+' km'],['Congested',s.roads_congested_km.toFixed(1)+' km'],['Roads closed',s.roads_closed_km.toFixed(1)+' km'],['95% safe after',s.time_to_safety_p95_h==null?'n/a':s.time_to_safety_p95_h+' h']];
 document.getElementById('kpis').innerHTML=k.map(([a,b])=>`<div class="kpi"><span>${a}</span><b>${b}</b></div>`).join('');}
sel.onchange=()=>show(+sel.value);show(0);
</script></body></html>"""


def run(region, flood="rp100", hours=(3, 8, 13, 17), weekday=True, weather="dry", name="evacuation", roads_flood=None):
    """roads_flood: flood stage that sets which roads are closed while people leave
    (None = same as `flood`, evacuating at the peak; "none" = order given before any road floods)."""
    geom, aoi_name = flood_tool.parse_region(region)
    outside = 1 - geom.intersection(study.hull()).area / geom.area
    if outside > 0.01:
        raise ValueError(f"{outside:.0%} of the region lies outside the study region")
    if geom.area / 1e6 > flood_tool.MAX_KM2:
        raise ValueError(f"region is {geom.area / 1e6:,.0f} km²; max {flood_tool.MAX_KM2:,}")
    if "observed" in (flood, roads_flood) and aoi_name is None:
        raise ValueError("'observed' needs a CEMS AOI name as region (a bbox or polygon has no observed flood)")
    weathers = [weather] if isinstance(weather, str) else list(weather)
    for w in weathers:
        if w not in WEATHER:
            raise ValueError(f"weather must be one of {list(WEATHER)}")
    out = OUTPUTS / "evacuations" / name
    out.mkdir(parents=True, exist_ok=True)
    area = Area(flood_tool.cems_folder(aoi_name)) if aoi_name else Area(region=geom, name=name)
    ctx = access_model.context(area)
    if ctx is None:
        raise ValueError("no roads in region")
    st = access_model.state(ctx, area, flood)
    if roads_flood is None or roads_flood == flood:
        closed, roads_label = st["edge_wet"], f"roads closed as at {flood} (evacuating at the peak)"
    elif roads_flood == "none":
        closed, roads_label = np.zeros(len(st["edge_wet"]), bool), "all roads open (order given before water arrives)"
    else:
        closed, roads_label = access_model.state(ctx, area, roads_flood)["edge_wet"], f"roads closed as at {roads_flood}"
    closed_gdf = edges_gdf(ctx["net"], dict(status=np.where(closed, "flooded (closed)", "")), closed)

    runs, summaries = [], []
    for w in weathers:
        for h in hours:
            roads, bld, s = simulate(ctx, st, h, weekday, w, closed)
            tag = f"{h:02d}h_{w}"
            r = edges_gdf(ctx["net"], roads, roads["cars"] > 0)
            c = cells_gdf(ctx, bld)
            r.to_file(out / f"roads_{tag}.gpkg", driver="GPKG")
            c.to_file(out / f"cells_{tag}.gpkg", driver="GPKG")
            summaries.append(dict(s, run=tag))
            runs.append(dict(label=f"{h:02d}:00 {'weekday' if weekday else 'weekend'}, {w.replace('_', ' ')}", summary=s,
                             roads=json.loads(_view_roads(r).to_json()),
                             cells=json.loads(c.to_crs(4326).round(2).to_json())))
            print(f"  {tag}: {s['evacuees']:,.0f} evacuees, {s['cars']:,.0f} cars, {s['boat_or_air']:,.0f} boat/air, "
                  f"{s['roads_congested_km']:.1f} km congested, 95% safe after {s['time_to_safety_p95_h']} h", flush=True)

    result = dict(name=name, region=region, flood=flood, roads=roads_label, weekday=weekday, buildings=len(ctx["bxy"]),
                  buildings_flooded=int(st["b_wet"].sum()), buildings_cut_off=int(st["cut"].sum()), runs=summaries,
                  assumptions=dict(compliance=COMPLIANCE, persons_per_car=PERSONS_PER_CAR, car_share=CAR_SHARE, speed_kmh=SPEED_KMH,
                                   capacity_vph=CAPACITY_VPH, weather_capacity_speed=WEATHER, safe_distance_m=SAFE_M),
                  caveats=flood_tool.CAVEATS + ["Behaviour (who is home, compliance, cars, capacities, weather effects) is assumed, not fitted: "
                                                "no evacuation data exists for this region. Static assignment: everyone takes the fastest route "
                                                "computed before congestion; no rerouting, no contraflow."],
                  files=dict(folder=str(out), viewer=str(out / "viewer.html")))
    (out / "summary.json").write_text(json.dumps(result, indent=2, default=float))
    bounds = gpd.GeoSeries([geom], crs=3035).to_crs(4326).total_bounds.tolist()
    data = dict(bounds=bounds, runs=runs, closed=json.loads(_slim(closed_gdf)[["geometry"]].to_json()))
    html = (VIEWER.replace("__NAME__", name).replace("__FLOOD__", f"Flood: {flood}; {roads_label}")
            .replace("__ASSUMPTIONS__", " ".join(result["caveats"]))
            .replace("__DATA__", json.dumps(data, default=float)))
    (out / "viewer.html").write_text(html)
    return result


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0], formatter_class=argparse.RawDescriptionHelpFormatter, epilog=__doc__)
    p.add_argument("--region", required=True)
    p.add_argument("--flood", default="rp100", help="rp10 | rp100 | rp500 | T=<years> | observed (AOI) | polygon file")
    p.add_argument("--hours", nargs="+", type=int, default=[3, 8, 13, 17])
    p.add_argument("--weekend", action="store_true")
    p.add_argument("--weather", nargs="+", default=["dry"], choices=list(WEATHER))
    p.add_argument("--name", default="evacuation")
    p.add_argument("--roads-flood", default=None,
                   help="flood stage for road closures while people leave: none | T=<years> | rp10 ... (default: same as --flood)")
    a = p.parse_args()
    try:
        r = run(a.region, a.flood, a.hours, not a.weekend, a.weather, a.name, a.roads_flood)
    except ValueError as e:
        sys.exit(str(e))
    print(f"\nWrote {r['files']['folder']}/: summary.json, roads_<run>.gpkg, cells_<run>.gpkg, viewer.html")
