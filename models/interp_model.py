"""Flood extent between two CEMS observations (PRODUCT -> MONIT01).

Cells flooded at both times stay flooded. Receding cells dry from the highest above the river
down; newly flooded cells fill from the lowest up, so the water level moves linearly in time.
Only the area seen by both images can be simulated. Areas with three observations are used
to check the method: predict the middle one from the outer two.
"""
import argparse, datetime as dt, itertools
import numpy as np, pandas as pd, geopandas as gpd, rasterio, shapely
from pathlib import Path
from rasterio.features import rasterize, shapes
from rasterio.transform import from_origin
from scipy.ndimage import distance_transform_edt

from main import DATA
from paths import OUTPUTS
from risk_model import Area

OUT = OUTPUTS / "interp"
CELL = 20          # simulation grid, m
MERGE_H = 3        # images closer than this are treated as one observation


def parse_time(date, tm):
    t = dt.datetime.strptime(date, "%Y/%m/%d" if date[4] == "/" else "%d/%m/%Y")
    if tm.startswith("T"):
        h, m, s = map(int, tm[1:9].split(":"))
        t = t.replace(hour=h, minute=m, second=s)
    return t


def observations(folders):
    """Every post-event image in the folders: time, flooded area, and the footprint it actually saw."""
    obs = {}
    for folder in folders:
        layer = lambda kind: gpd.read_file(next(folder.glob(f"*_{kind}_*.shp"))).to_crs(3035)
        ev = layer("observedEventA")
        ev = ev[ev.notation == "Flooded area"]  # 'Flood trace' marks where water was, not where it is
        foot = layer("imageFootprintA")
        src = gpd.read_file(next(folder.glob("*_source_*.dbf")))
        for r in src[src.eventphase == "Post-event"].itertuples():
            fp = foot[foot.or_src_id == r.src_id]
            if fp.empty:
                continue
            o = obs.setdefault((r.src_date, r.source_tm, r.source_nam),
                               dict(time=parse_time(r.src_date, r.source_tm), sensor=r.source_nam, wet=[], foot=[]))
            o["wet"] += list(ev[ev.dmg_src_id == r.src_id].geometry)
            o["foot"] += list(fp.geometry)
    merged = []
    for o in sorted(obs.values(), key=lambda o: o["time"]):
        if merged and (o["time"] - merged[-1]["time"]).total_seconds() < MERGE_H * 3600:
            merged[-1]["wet"] += o["wet"]; merged[-1]["foot"] += o["foot"]; merged[-1]["sensor"] += " + " + o["sensor"]
        else:
            merged.append(o)
    return [dict(o, wet=shapely.union_all(o["wet"]), foot=shapely.union_all(o["foot"])) for o in merged]


class Grid:
    def __init__(self, region):
        x0, y0, x1, y1 = region.bounds
        self.shape = (int(np.ceil((y1 - y0) / CELL)), int(np.ceil((x1 - x0) / CELL)))
        self.tf = from_origin(x0, y1, CELL, CELL)
        self.inside = self.raster(region)

    def raster(self, geom):
        if geom.is_empty:
            return np.zeros(self.shape, bool)
        return rasterize([geom], out_shape=self.shape, transform=self.tf, dtype="uint8").astype(bool)

    def centres(self, mask):
        r, c = np.nonzero(mask)
        x, y = self.tf * (c + 0.5, r + 0.5)
        return np.c_[x, y]


def simulate(a, b, area, region):
    """Per-cell flip time (fraction 0..1 of the interval) for every cell that changes between a and b."""
    g = Grid(region)
    wa, wb = g.raster(a["wet"]) & g.inside, g.raster(b["wet"]) & g.inside
    flip = np.full(g.shape, np.nan, "float32")
    for m, lowest_first in ((wa & ~wb, False), (~wa & wb, True)):  # receding: highest dries first
        if m.any():
            hand = area.features(g.centres(m))["hand"].to_numpy()
            rank = pd.Series(hand).rank(method="first", ascending=lowest_first).to_numpy()
            flip[m] = (rank - 0.5) / len(rank)
    return dict(grid=g, wa=wa, wb=wb, flip=flip)


def extent_at(sim, f):
    wa, wb, flip = sim["wa"], sim["wb"], sim["flip"]
    return (wa & wb) | (wa & ~wb & (flip > f)) | (~wa & wb & (flip <= f))


def morph_at(wa, wb, f):
    """Baseline: blend the signed distance to each shoreline, ignoring terrain."""
    sdf = lambda m: distance_transform_edt(~m) - distance_transform_edt(m)
    return ((1 - f) * sdf(wa) + f * sdf(wb)) <= 0


def iou(pred, true, inside):
    p, t = pred & inside, true & inside
    union = (p | t).sum()
    return (p & t).sum() / union if union else np.nan


def write_outputs(sim, a, b, name, step_h):
    g, hours = sim["grid"], (b["time"] - a["time"]).total_seconds() / 3600
    state = np.zeros(g.shape, "uint8")
    state[sim["wa"] & sim["wb"]] = 1; state[sim["wa"] & ~sim["wb"]] = 2; state[~sim["wa"] & sim["wb"]] = 3
    state[~g.inside] = 255
    with rasterio.open(OUT / f"{name}.tif", "w", driver="GTiff", height=g.shape[0], width=g.shape[1], count=2,
                       dtype="float32", crs="EPSG:3035", transform=g.tf, nodata=-1, compress="deflate") as dst:
        dst.write(np.where(state == 255, -1, state.astype("float32")), 1)   # 0 dry, 1 wet both, 2 receding, 3 rising
        dst.write(np.where(np.isnan(sim["flip"]), -1, sim["flip"] * hours), 2)  # hours after t0 the cell flips
        dst.set_band_description(1, "state"); dst.set_band_description(2, "flip_hours")

    rows, polys = [], []
    steps = sorted(set(np.r_[np.arange(0, hours, step_h), hours]))
    for h in steps:
        m = extent_at(sim, h / hours)
        t = a["time"] + dt.timedelta(hours=float(h))
        rows.append(dict(pair=name, time=t, hours=h, flooded_km2=m.sum() * CELL ** 2 / 1e6))
        geoms = [shapely.geometry.shape(s) for s, v in shapes(m.astype("uint8"), mask=m, transform=g.tf) if v]
        polys.append(gpd.GeoDataFrame(dict(time=[t.isoformat()], hours=[h]), geometry=[shapely.union_all(geoms)], crs=3035))
    pd.concat(polys).to_file(OUT / "extents.gpkg", layer=name, driver="GPKG")
    return rows


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--step", type=float, default=6, help="hours between simulated extents")
    args = p.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)

    series, checks = [], []
    for monit in sorted(DATA.glob("ems_*/*_MONIT01_*_vector")):
        product = next(monit.parent.glob(monit.name.split("_MONIT01_")[0] + "_PRODUCT_*_vector"))
        obs = observations([product, monit])
        area = Area(product)
        aoi = area.aoi
        print(f"\n== {area.name}: " + ", ".join(f"{o['time']:%d/%m %H:%M} {o['sensor']}" for o in obs), flush=True)

        # Consecutive pairs: the simulation
        for i, (a, b) in enumerate(zip(obs, obs[1:])):
            region = shapely.intersection_all([a["foot"], b["foot"], aoi])
            if region.area < 1e6:
                print(f"   {a['time']:%d/%m} -> {b['time']:%d/%m}: footprints barely overlap, skipped")
                continue
            sim = simulate(a, b, area, region)
            name = f"{area.name}_{a['time']:%d%H}_{b['time']:%d%H}"
            series += write_outputs(sim, a, b, name, args.step)
            print(f"   {name}: overlap {region.area / 1e6:.0f} km², flooded {sim['wa'].sum() * CELL**2 / 1e6:.2f} -> "
                  f"{sim['wb'].sum() * CELL**2 / 1e6:.2f} km²")

        # Triples: hide the middle observation and predict it from the outer two
        for a, mid, b in itertools.combinations(obs, 3):
            region = shapely.intersection_all([a["foot"], mid["foot"], b["foot"], aoi])
            if region.area < 1e6:
                continue
            sim = simulate(a, b, area, region)
            f = (mid["time"] - a["time"]) / (b["time"] - a["time"])
            truth, inside = sim["grid"].raster(mid["wet"]), sim["grid"].inside
            nearest = sim["wa"] if f < 0.5 else sim["wb"]
            checks.append(dict(area=area.name, predicted=f"{mid['time']:%d/%m %H:%M}", fraction=round(f, 2),
                               overlap_km2=region.area / 1e6,
                               iou_terrain=iou(extent_at(sim, f), truth, inside),
                               iou_morph=iou(morph_at(sim["wa"], sim["wb"], f), truth, inside),
                               iou_nearest=iou(nearest, truth, inside)))
        del area

    pd.DataFrame(series).to_csv(OUT / "timeseries.csv", index=False)
    pd.set_option("display.width", 160)
    if checks:
        print("\nValidation: predict the middle image from the outer two (IoU, 1 = perfect)")
        print(pd.DataFrame(checks).round(3).to_string(index=False))
    else:
        print("\nNo area has three overlapping observations; the method could not be validated")
    print(f"\nWrote {OUT}/extents.gpkg, timeseries.csv and per-pair state/flip-time rasters")
