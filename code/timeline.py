"""Flood timeline for a region: how a flood develops in time and space.

Driver (pick one):
  --peak-rp 500 --rise-h 24 --fall-h 72 [--step-h 6]   what-if scenario hydrograph
  --series file.csv                                     columns time, return_period
                                                        (or time, discharge with --q10 --q100 --q500)
  --open-meteo [LON,LAT] --start --end                  GloFAS river discharge (Open-Meteo, cached;
                                                        thresholds fitted on 1984-2020 annual maxima)
Each step: return period T → depth map (EFAS RP10/100/500 blended on log T, checked against the
30 m DEM) → roads closed, buildings cut off, evacuation levels (access_model) → viewer.

  python3 timeline.py --region 6.9,50.45,7.15,50.58 --peak-rp 500 --name ahr_whatif

Writes outputs/timelines/<name>/: steps.csv, depth_<step>.tif, cells.gpkg (one layer per step),
cells.geojson (100 m cells, the same grid and ids as flood_api /region, depth per step), viewer.html
(map + time slider; open in a browser).
"""
import argparse, base64, io, json, sys, urllib.request
import geopandas as gpd, numpy as np, pandas as pd, rasterio, shapely
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from pyproj import Transformer
from rasterio.transform import from_origin

import access_model, flood_tool, region as study
from paths import CACHE, DATA, OUTPUTS
from risk_model import Area, blend_T, FEATURES

OUT = OUTPUTS / "timelines"
GRID_DEG = 0.0005    # depth map resolution (~35 x 55 m)
MAX_DEPTH_SHOWN = 3  # m, top of the colour scale
TO_3035 = Transformer.from_crs(4326, 3035, always_xy=True)


# ---------- drivers: time series of return periods ----------

def synthetic(peak, rise_h, fall_h, step_h):
    """log T rises linearly from 2 years to the peak, then falls back (a simple what-if hydrograph)."""
    hours = np.arange(0, rise_h + fall_h + step_h / 2, step_h)
    f = np.where(hours <= rise_h, hours / rise_h, 1 - (hours - rise_h) / fall_h).clip(0, 1)
    T = np.exp(np.log(2) + (np.log(peak) - np.log(2)) * f)
    return pd.DataFrame(dict(time=[f"H+{h:g}" for h in hours], return_period=T))


def gumbel_T(q, q10, q100, q500=None):
    """Return period of discharge q from a Gumbel distribution fitted through Q10 and Q100."""
    y10, y100 = -np.log(-np.log(1 - 1 / 10)), -np.log(-np.log(1 - 1 / 100))
    beta = (q100 - q10) / (y100 - y10)
    mu = q10 - beta * y10
    F = np.exp(-np.exp(-(np.asarray(q, float) - mu) / beta))
    return 1 / np.clip(1 - F, 1e-6, None)


def from_series(path, q10, q100):
    s = pd.read_csv(path)
    if "return_period" not in s:
        if q10 is None or q100 is None:
            sys.exit("a discharge series needs --q10 and --q100 (and optionally --q500)")
        s["return_period"] = gumbel_T(s.discharge, q10, q100)
    return s[["time", "return_period"] + (["discharge"] if "discharge" in s else [])]


def open_meteo(lon, lat, start, end):
    """Daily GloFAS discharge for the event, thresholds from 1984-2020 annual maxima (cached)."""
    cache = CACHE / "discharge"
    cache.mkdir(parents=True, exist_ok=True)

    def get(a, b):
        f = cache / f"{lon:.3f}_{lat:.3f}_{a}_{b}.json"
        if not f.exists():
            url = (f"https://flood-api.open-meteo.com/v1/flood?latitude={lat}&longitude={lon}"
                   f"&daily=river_discharge&start_date={a}&end_date={b}")
            data = json.load(urllib.request.urlopen(url, timeout=60))
            if data.get("error"):
                sys.exit(f"Open-Meteo: {data.get('reason')}. Use --peak-rp or --series instead.")
            f.write_text(json.dumps(data))
        d = json.loads(f.read_text())["daily"]
        return pd.Series(d["river_discharge"], index=pd.to_datetime(d["time"]), dtype=float)

    hist = get("1984-01-01", "2020-12-31")
    annual = hist.groupby(hist.index.year).max().dropna()
    # Gumbel by method of moments
    beta = annual.std() * np.sqrt(6) / np.pi
    mu = annual.mean() - 0.5772 * beta
    q = lambda T: mu - beta * np.log(-np.log(1 - 1 / T))
    ev = get(start, end)
    print(f"  gauge {lon:.3f},{lat:.3f}: Q10 {q(10):.0f}, Q100 {q(100):.0f}, Q500 {q(500):.0f} m³/s; event peak {ev.max():.0f} m³/s")
    return pd.DataFrame(dict(time=ev.index.strftime("%Y-%m-%d"), discharge=ev.values,
                             return_period=gumbel_T(ev.values, q(10), q(100))))


# ---------- maps ----------

def depth_grid(area, geom):
    """RP10/100/500 depths on a lon/lat grid over the region (blend per step afterwards)."""
    w, s, e, n = gpd.GeoSeries([geom], crs=3035).to_crs(4326).total_bounds
    lons, lats = np.arange(w + GRID_DEG / 2, e, GRID_DEG), np.arange(n - GRID_DEG / 2, s, -GRID_DEG)
    glon, glat = np.meshgrid(lons, lats)
    x, y = TO_3035.transform(glon.ravel(), glat.ravel())
    xy = np.c_[x, y]
    inside = shapely.contains_xy(geom, xy[:, 0], xy[:, 1])
    rps = {}
    for rp in ("rp10", "rp100", "rp500"):
        d = np.zeros(len(xy), "float32")
        d[inside] = area.depth(rp, xy[inside])
        rps[rp] = d.reshape(glon.shape)
    return dict(bounds=(w, s, e, n), shape=glon.shape, inside=inside.reshape(glon.shape), tf=from_origin(w, n, GRID_DEG, GRID_DEG), **rps)


CELL_M = 100  # flood_api /region grid


def region_cells(geom, grid, depths, steps, water):
    """Depth per step on the 100 m cells of flood_api /region (features.rs `build`): centres from the
    region's EPSG:3035 bounds (west + 50 m, north - 50 m), inside the region and outside OSM water,
    id "<x>_<y>" as in /region cells.geojson. A multi-part AOI is gridded from its largest part's
    bounds, as /region does, so the ids line up. Only cells that get wet in some step are kept."""
    part = max(getattr(geom, "geoms", [geom]), key=lambda g: g.area)
    x0, y0, x1, y1 = part.bounds
    xs = x0 + CELL_M / 2 + CELL_M * np.arange(int(np.ceil((x1 - x0 - CELL_M / 2) / CELL_M)))
    ys = y1 - CELL_M / 2 - CELL_M * np.arange(int(np.ceil((y1 - y0 - CELL_M / 2) / CELL_M)))
    xs, ys = xs[xs < x1], ys[ys > y0]
    gx, gy = np.meshgrid(xs, ys)
    x, y = gx.ravel(), gy.ravel()
    keep = shapely.contains_xy(geom, x, y)
    if len(water):
        keep &= ~shapely.contains_xy(water.union_all(), x, y)
    x, y = x[keep], y[keep]
    lon, lat = TO_3035.transform(x, y, direction="INVERSE")
    col, row = ~grid["tf"] * (lon, lat)
    row = np.clip(np.floor(row).astype(int), 0, grid["shape"][0] - 1)
    col = np.clip(np.floor(col).astype(int), 0, grid["shape"][1] - 1)
    d = np.round(np.stack([np.maximum(dd[row, col], 0) for dd in depths], axis=1), 2)
    wet = d.max(axis=1) > 0
    h = CELL_M / 2
    feats = []
    for xi, yi, di in zip(x[wet], y[wet], d[wet]):
        cx, cy = TO_3035.transform(np.array([xi - h, xi + h, xi + h, xi - h, xi - h]), np.array([yi - h, yi - h, yi + h, yi + h, yi - h]), direction="INVERSE")
        feats.append(dict(type="Feature", geometry=dict(type="Polygon", coordinates=[[[round(a, 5), round(b, 5)] for a, b in zip(cx, cy)]]),
                          properties=dict(id=f"{xi:.0f}_{yi:.0f}", d=[float(v) for v in di])))
    return dict(type="FeatureCollection", steps=steps, features=feats)


def png_b64(arr, cmap, vmax, alpha_mask):
    rgba = plt.get_cmap(cmap)(np.clip(arr / vmax, 0, 1))
    rgba[..., 3] = np.where(alpha_mask, 0.8, 0)
    buf = io.BytesIO()
    plt.imsave(buf, rgba, format="png")
    return base64.b64encode(buf.getvalue()).decode()


def write_tif(path, arr, tf):
    with rasterio.open(path, "w", driver="GTiff", height=arr.shape[0], width=arr.shape[1], count=1, dtype="float32",
                       crs="EPSG:4326", transform=tf, nodata=-1, compress="deflate") as dst:
        dst.write(arr.astype("float32"), 1)


# ---------- viewer ----------

VIEWER = """<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Flood Timeline</title>
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
.ctl{display:flex;gap:8px;align-items:center;margin-bottom:8px}input[type=range]{flex:1}button{background:var(--accent);color:#fff;border:0;border-radius:4px;padding:5px 10px;cursor:pointer}
.big{font-size:20px;font-weight:600}.kpis{display:grid;grid-template-columns:1fr 1fr;gap:8px;margin:10px 0}
.kpi{border:1px solid var(--rule);border-radius:6px;padding:6px 8px}.kpi span{display:block;color:var(--muted);font-size:11px}.kpi b{font-size:16px}
svg{width:100%;height:110px}.legend span{display:inline-block;width:12px;height:12px;margin-right:4px;vertical-align:-2px}
.note{color:var(--muted);font-size:11px;margin-top:10px}ol{padding-left:18px;margin:6px 0}li{margin-bottom:4px;font-size:12px}
</style></head><body>
<header><h1>Flood timeline: __NAME__</h1><p>__DRIVER__ · relative scenario, not a forecast of this exact flood</p></header>
<main><div id="map"></div><aside>
<div class="ctl"><button id="play">▶ Play</button><input id="t" type="range" min="0" max="0" value="0"></div>
<div class="big" id="when"></div><div id="rp" style="color:var(--muted)"></div>
<svg id="chart" viewBox="0 0 320 110" preserveAspectRatio="none"></svg>
<div class="kpis" id="kpis"></div>
<div class="legend"><b>Evacuation</b><br><span style="background:#b2182b"></span>Evacuate now <span style="background:#ef8a62"></span>Early <span style="background:#fddbc7;border:1px solid #c99"></span>Prepare<br>
<b>Water depth</b> 0 → __VMAX__ m (blue) · <b>+</b> fire station / hospital</div>
<h3 style="font-size:14px;margin:12px 0 4px">Evacuate first (this step)</h3><ol id="first"></ol>
<p class="note">__CAVEATS__</p></aside></main>
<script>
const D=__DATA__;
const map=L.map('map').fitBounds([[D.bounds[1],D.bounds[0]],[D.bounds[3],D.bounds[2]]]);
L.tileLayer('https://tile.openstreetmap.org/{z}/{x}/{y}.png',{maxZoom:18,attribution:'© OpenStreetMap'}).addTo(map);
const overlay=L.imageOverlay('data:image/png;base64,'+D.steps[0].png,[[D.bounds[1],D.bounds[0]],[D.bounds[3],D.bounds[2]]],{opacity:.85}).addTo(map);
L.geoJSON(D.region,{style:{color:'#444',weight:1.5,fill:false,dashArray:'4'}}).addTo(map);
L.geoJSON(D.facilities,{pointToLayer:(f,ll)=>L.circleMarker(ll,{radius:4,color:'#1a7f37',weight:2,fillOpacity:1}).bindTooltip(f.properties.label)}).addTo(map);
const col={"Evacuate now":"#b2182b","Evacuate early":"#ef8a62","Prepare":"#fddbc7"};
let cells=L.layerGroup().addTo(map);
function chart(i){const s=D.steps,w=320,h=110,lt=Math.log,mx=lt(Math.max(600,...s.map(x=>x.T)));
 const X=k=>k/(s.length-1||1)*w,Y=T=>h-8-(lt(Math.max(T,1))/mx)*(h-16);
 let g='';for(const [T,c] of [[10,'#9ecae1'],[100,'#4292c6'],[500,'#08519c']])g+=`<line x1="0" x2="${w}" y1="${Y(T)}" y2="${Y(T)}" stroke="${c}" stroke-dasharray="3"/><text x="2" y="${Y(T)-2}" font-size="9" fill="${c}">RP${T}</text>`;
 g+=`<polyline fill="none" stroke="currentColor" stroke-width="2" points="${s.map((x,k)=>X(k)+','+Y(x.T)).join(' ')}"/>`;
 g+=`<circle cx="${X(i)}" cy="${Y(s[i].T)}" r="4" fill="#b2182b"/>`;document.getElementById('chart').innerHTML=g;}
function show(i){const s=D.steps[i];overlay.setUrl('data:image/png;base64,'+s.png);cells.clearLayers();
 L.geoJSON(s.cells,{style:f=>({color:'#7a1020',weight:.6,fillColor:col[f.properties.evac_level],fillOpacity:.55}),
  onEachFeature:(f,l)=>l.bindPopup(`<b>${f.properties.evac_level}</b><br>${f.properties.evac_reasons}`)}).addTo(cells);
 document.getElementById('when').textContent=s.time;
 document.getElementById('rp').textContent=`Return period ≈ ${s.T<2?'< 2':Math.round(s.T)} years`+(s.discharge!=null?` · ${Math.round(s.discharge)} m³/s`:'');
 const k=[['Flooded area',s.flooded_km2.toFixed(1)+' km²'],['Roads closed',s.roads_closed_km.toFixed(1)+' km'],['Buildings flooded',s.buildings_flooded.toLocaleString()],['Buildings cut off',s.buildings_cut_off.toLocaleString()],['Evacuate now',Math.round(s.people_now).toLocaleString()+' people'],['Evacuate early',Math.round(s.people_early).toLocaleString()+' people'],['By boat / air only',Math.round(s.people_air).toLocaleString()+' people'],['Prepare',Math.round(s.people_prepare).toLocaleString()+' people']];
 document.getElementById('kpis').innerHTML=k.map(([a,b])=>`<div class="kpi"><span>${a}</span><b>${b}</b></div>`).join('');
 document.getElementById('first').innerHTML=s.first.map(c=>`<li><b>${c.evac_level}</b> (${c.lat}, ${c.lon}): ${c.evac_reasons}</li>`).join('')||'<li>No cell needs evacuation at this step.</li>';
 chart(i);}
const t=document.getElementById('t');t.max=D.steps.length-1;t.oninput=()=>show(+t.value);
let timer=null;document.getElementById('play').onclick=e=>{if(timer){clearInterval(timer);timer=null;e.target.textContent='▶ Play';return}
 e.target.textContent='❚❚ Pause';timer=setInterval(()=>{t.value=(+t.value+1)%D.steps.length;show(+t.value)},900)};
show(0);
</script></body></html>"""


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0], formatter_class=argparse.RawDescriptionHelpFormatter, epilog=__doc__)
    p.add_argument("--region", required=True, help="w,s,e,n | polygon file | CEMS AOI name")
    p.add_argument("--name", default="timeline")
    p.add_argument("--peak-rp", type=float)
    p.add_argument("--rise-h", type=float, default=24)
    p.add_argument("--fall-h", type=float, default=72)
    p.add_argument("--step-h", type=float, default=6)
    p.add_argument("--series", help="CSV: time, return_period | time, discharge")
    p.add_argument("--q10", type=float); p.add_argument("--q100", type=float)
    p.add_argument("--open-meteo", nargs="?", const="auto", help="LON,LAT of the river point (default: region centre)")
    p.add_argument("--start"); p.add_argument("--end")
    args = p.parse_args()

    geom, aoi_name = flood_tool.parse_region(args.region)
    outside = 1 - geom.intersection(study.hull()).area / geom.area
    if outside > 0.01:
        sys.exit(f"{outside:.0%} of the region lies outside the study region; the models were not built for it.")
    if geom.area / 1e6 > flood_tool.MAX_KM2:
        sys.exit(f"region is {geom.area / 1e6:,.0f} km²; max {flood_tool.MAX_KM2:,}")

    if args.peak_rp:
        steps, driver = synthetic(args.peak_rp, args.rise_h, args.fall_h, args.step_h), \
            f"What-if: rises to a 1-in-{args.peak_rp:g}-year flood in {args.rise_h:g} h, recedes over {args.fall_h:g} h"
    elif args.series:
        steps, driver = from_series(args.series, args.q10, args.q100), f"Series: {args.series}"
    elif args.open_meteo:
        if args.open_meteo == "auto":
            c = gpd.GeoSeries([geom.centroid], crs=3035).to_crs(4326).iloc[0]
            lon, lat = c.x, c.y
        else:
            lon, lat = map(float, args.open_meteo.split(","))
        steps, driver = open_meteo(lon, lat, args.start, args.end), f"GloFAS discharge at {lon:.3f}, {lat:.3f} (Open-Meteo)"
    else:
        sys.exit("choose a driver: --peak-rp, --series or --open-meteo")

    name = args.name
    out = OUT / name
    out.mkdir(parents=True, exist_ok=True)
    area = Area(flood_tool.cems_folder(aoi_name)) if aoi_name else Area(region=geom, name=name)

    # Steps with the same (rounded) return period share one analysis
    steps["T"] = [float(f"{t:.2g}") for t in steps.return_period.clip(lower=1)]  # 2 significant digits
    specs = [f"T={T:g}" for T in sorted(steps["T"].unique()) if T > 2]
    print(f"{len(steps)} steps, {len(specs)} distinct flood stages", flush=True)
    cells = pd.concat(access_model.analyse(area, specs), ignore_index=True) if specs else pd.DataFrame()
    gdf = access_model.to_gdf(cells).to_crs(4326) if len(cells) else None

    grid = depth_grid(area, geom)
    px_km2 = (GRID_DEG * 111.32) ** 2 * np.cos(np.radians((grid["bounds"][1] + grid["bounds"][3]) / 2))
    fac = access_model.osm_data.layer("facilities", geom)
    fac = fac[shapely.contains_xy(geom.buffer(2000), *shapely.get_coordinates(fac.geometry.values).T)].to_crs(4326)
    fac["label"] = fac.amenity.str.replace("_", " ") + fac.name.fillna("").map(lambda n: f": {n}" if n else "")

    rows, js_steps, step_depths = [], [], []
    for i, r in steps.reset_index(drop=True).iterrows():
        d = blend_T(r["T"], grid["rp10"], grid["rp100"], grid["rp500"])
        step_depths.append(np.where(grid["inside"], d, 0))
        write_tif(out / f"depth_{i:02d}.tif", np.where(grid["inside"], d, -1), grid["tf"])
        label = access_model.flood_label(f"T={r['T']:g}")
        c = gdf[gdf["flood"] == label] if gdf is not None and r["T"] > 2 else None
        st = access_model.STATS.get(label, dict(buildings_flooded=0, buildings_cut_off=0, roads_closed_km=0.0))
        ev = c[c.evac_level != "None"] if c is not None else None
        row = dict(step=i, time=r["time"], T=float(r["T"]), discharge=float(r["discharge"]) if "discharge" in r and pd.notna(r.get("discharge")) else None,
                   flooded_km2=float((d >= access_model.IMPASSABLE_M).sum() * px_km2),
                   roads_closed_km=st["roads_closed_km"], buildings_flooded=st["buildings_flooded"], buildings_cut_off=st["buildings_cut_off"],
                   people_now=float(c.must_people.sum()) if c is not None else 0.0, people_early=float(c.early_people.sum()) if c is not None else 0.0,
                   people_air=float(c.air_people.sum()) if c is not None else 0.0, people_prepare=float(c.prepare_people.sum()) if c is not None else 0.0)
        rows.append(row)
        if ev is not None and len(ev):
            ev.to_file(out / "cells.gpkg", layer=f"step_{i:02d}", driver="GPKG")
        first = ev.sort_values(["must_people", "early_people"], ascending=False).head(5) if ev is not None else pd.DataFrame()
        js_steps.append(dict(row, png=png_b64(d, "Blues", MAX_DEPTH_SHOWN, grid["inside"] & (d >= 0.05)),
                             cells=json.loads(ev[["evac_level", "evac_reasons", "geometry"]].to_json()) if ev is not None and len(ev) else
                             dict(type="FeatureCollection", features=[]),
                             first=first[["evac_level", "lat", "lon", "evac_reasons"]].to_dict("records") if len(first) else []))
        print(f"  {r['time']:>10}  T≈{r['T']:>6g}  flooded {row['flooded_km2']:6.1f} km²  roads closed {row['roads_closed_km']:6.1f} km  "
              f"evacuate now {row['people_now']:7.0f}  early {row['people_early']:7.0f}", flush=True)

    pd.DataFrame(rows).round(2).to_csv(out / "steps.csv", index=False)
    fc = region_cells(geom, grid, step_depths, [dict(step=r["step"], time=r["time"], T=r["T"]) for r in rows],
                      access_model.osm_data.layer("water", geom))
    (out / "cells.geojson").write_text(json.dumps(fc, separators=(",", ":")))
    data = dict(bounds=grid["bounds"], steps=js_steps,
                region=json.loads(gpd.GeoSeries([geom], crs=3035).to_crs(4326).to_json()),
                facilities=json.loads(fac[["label", "geometry"]].to_json()))
    html = (VIEWER.replace("__NAME__", name).replace("__DRIVER__", driver).replace("__VMAX__", str(MAX_DEPTH_SHOWN))
            .replace("__CAVEATS__", " ".join(flood_tool.CAVEATS) + " Between EFAS return periods the depth is interpolated on log(T); "
                     "below a 2-year flow rivers are assumed to stay in their banks.")
            .replace("__DATA__", json.dumps(data, default=float)))
    (out / "viewer.html").write_text(html)
    print(f"\nWrote {out}/: steps.csv, depth_<step>.tif, cells.gpkg, cells.geojson ({len(fc['features']):,} cells), viewer.html")


if __name__ == "__main__":
    main()
