"""Danger from blocked access: where can help not get in when roads flood?

Works for any region inside the study region (a CEMS AOI or any polygon). Roads, bridges, fire
stations and hospitals come from OpenStreetMap. Help starts from the fire stations and hospitals
and from every road entering the region. For each flood, flooded road stretches are closed and
every building is checked: still reachable, only reachable if flooded bridges hold, reachable via
a long detour, or far from the nearest fire station / hospital.

Per 1 km cell every factor is reported on its own: value, level (fixed thresholds in FACTORS)
and a plain-language reason. The overall level only counts how many factors are high; there is
no weighted score.
"""
import argparse
import joblib, numpy as np, pandas as pd, geopandas as gpd, rasterio, shapely
from pathlib import Path
from pyproj import Transformer
from rasterio.windows import Window, from_bounds
from scipy.sparse import coo_matrix
from scipy.sparse.csgraph import dijkstra
from scipy.spatial import cKDTree

from main import DATA, POP
from paths import OUTPUTS
from pipeline import STOREY_M
from risk_model import EFAS, FEATURES, Area
import osm_data, risk_model
from road_model import sample_raster

OUT = OUTPUTS / "access"
CELL_M = 1000
STEP_M = 50          # roads are split into edges of at most this length
IMPASSABLE_M = 0.3   # water depth that closes a road / counts a building as flooded
ENTRY_M = 300        # road nodes this close to the region edge are where outside help enters
SNAP_M = 2000        # buildings farther than this from a mapped road are not assessed
FACILITY_SNAP_M = 500
TO_LONLAT = Transformer.from_crs(3035, 4326, always_xy=True)

# column, label, medium threshold, high threshold, reason text
FACTORS = [
    ("flooded_pct", "Flooded buildings", 5, 25, "{:.0f}% of buildings under ≥0.3 m of water"),
    ("cut_off_pct", "Cut off by road", 10, 50, "{:.0f}% of buildings unreachable by road"),
    ("bridge_pct", "Depends on bridges", 10, 50, "{:.0f}% reachable only if flooded bridges hold"),
    ("detour", "Long detour", 1.3, 2.0, "typical route {:.1f}× longer than normal"),
    ("help_km", "Far from help", 8, 15, "nearest fire station / hospital {:.0f} km by road"),
    ("people", "People affected", 100, 500, "~{:.0f} people in flooded or cut-off buildings"),
    ("single_storey_pct", "No upper floor", 20, 50, "{:.0f}% of flooded buildings are single-storey"),
    ("likelihood", "Flood-prone terrain", 0.3, 0.5, "relative terrain risk {:.2f} (a ranking, not a probability)"),
]

# Evacuation rules per building (see NOTES.md step 13)
MUST_DEPTH_M = 1.0   # this deep, or flooded and single-storey: no safe place left inside
EVAC_MIN = 25        # people needed in a category before a cell gets that evacuation level
EVAC_LEVELS = ["Evacuate now", "Evacuate early", "Prepare", "None"]


def evacuation(cells):
    """Evacuation level, reasons and priority rank per cell (fixed order, no weighted score)."""
    lvl = np.select([cells.must_people >= EVAC_MIN, cells.must_people + cells.early_people >= EVAC_MIN,
                     cells.prepare_people >= EVAC_MIN], EVAC_LEVELS[:3], EVAC_LEVELS[3])
    cells["evac_level"] = lvl
    reasons = []
    for r in cells.itertuples():
        parts = []
        if r.must_people >= 1:
            parts.append(f"~{r.must_people:.0f} people in buildings with ≥{MUST_DEPTH_M:g} m of water or flooded single-storey (no refuge upstairs)")
        if r.early_people >= 1:
            parts.append(f"~{r.early_people:.0f} in flooded or cut-off buildings")
        if r.air_people >= 1:
            parts.append(f"~{r.air_people:.0f} of them only reachable by boat/air (no road out)")
        if r.prepare_people >= 1:
            parts.append(f"~{r.prepare_people:.0f} at risk of being cut off (bridge, long detour or far from help, flood-prone terrain)")
        parts.append(f"{r.population:,.0f} residents in this km²")
        reasons.append("; ".join(parts))
    cells["evac_reasons"] = reasons
    order = pd.Categorical(cells.evac_level, EVAC_LEVELS, ordered=True)
    rank = cells.assign(_o=order).sort_values(["_o", "must_people", "early_people", "prepare_people"],
                                              ascending=[True, False, False, False]).index
    cells["evac_priority"] = 0
    cells.loc[rank, "evac_priority"] = np.arange(1, len(rank) + 1)
    cells.loc[cells.evac_level == "None", "evac_priority"] = 0
    return cells


class Network:
    """Road graph: nodes are junctions/vertices, edges are road stretches of at most STEP_M."""

    def __init__(self, roads, water):
        parts, src = shapely.get_parts(shapely.segmentize(roads.geometry.values, STEP_M), return_index=True)
        coords, idx = shapely.get_coordinates(parts, return_index=True)
        nodes, inv = np.unique(np.round(coords), axis=0, return_inverse=True)
        inv = inv.ravel()
        same = (idx[1:] == idx[:-1]) & (inv[1:] != inv[:-1])
        pairs = np.sort(np.c_[inv[:-1][same], inv[1:][same]], axis=1)
        pairs, first = np.unique(pairs, axis=0, return_index=True)  # a stretch shared by two lines counts once
        p, q = coords[:-1][same][first], coords[1:][same][first]
        self.xy, self.a, self.b = nodes, pairs[:, 0], pairs[:, 1]
        self.length, self.mid, self.p, self.q = np.hypot(*(q - p).T), (p + q) / 2, p, q
        self.tree = cKDTree(nodes)

        # Bridges: tagged bridge=* in OSM, or crossing a river / water body
        edges = shapely.linestrings(np.stack([p, q], axis=1))
        line = src[idx[:-1][same][first]]
        tagged = roads["bridge"].to_numpy(bool)[line]
        self.road_class = roads["road_class"].to_numpy()[line]
        crossing = np.zeros(len(edges), bool)
        if len(water):
            crossing[shapely.STRtree(edges).query(water.geometry.values, predicate="intersects")[1]] = True
        self.bridge = tagged | crossing

    def distances(self, open_edges, sources):
        n = len(self.xy)
        g = coo_matrix((self.length[open_edges], (self.a[open_edges], self.b[open_edges])), shape=(n, n)).tocsr()
        if len(sources) == 0:
            return np.full(n, np.inf)
        return dijkstra(g, directed=False, indices=sources, min_only=True)


def level(v, med, high):
    return "n/a" if pd.isna(v) else "high" if v >= high else "medium" if v >= med else "low"


def explain(cells):
    """Add a level per factor, the overall level (count of highs) and the reasons in words."""
    for col, label, med, high, _ in FACTORS:
        cells[f"{col}_level"] = [level(v, med, high) for v in cells[col]]
    lv = cells[[f"{c}_level" for c, *_ in FACTORS]]
    cells["n_high"] = (lv == "high").sum(axis=1)
    n_med = (lv == "medium").sum(axis=1)
    cells["overall"] = np.select([cells.n_high >= 3, cells.n_high == 2, (cells.n_high == 1) | (n_med >= 2)],
                                 ["Critical", "High", "Elevated"], "Low")
    reasons = []
    for _, r in cells.iterrows():
        parts = [f"{'HIGH' if r[f'{c}_level'] == 'high' else 'medium'}: {label}, {text.format(r[c])}"
                 for want in ("high", "medium") for c, label, _, _, text in FACTORS if r[f"{c}_level"] == want]
        reasons.append("; ".join(parts) or "no factor above medium")
    cells["reasons"] = reasons
    return cells


def load_model():
    return joblib.load(risk_model.OUT / "model.joblib")["model"]


STATS = {}  # per flood label: totals that are not per cell (filled by analyse)


def spec_depth(area, spec, xy, dem_check=True):
    """Water depth for EFAS floods (rp10/rp100/rp500, or T=<return period>); None when unknown."""
    if spec in EFAS:
        return area.depth(spec, xy, dem_check)
    if spec.startswith("T="):
        return area.depth_T(float(spec[2:]), xy, dem_check)
    return None


def flood_label(spec):
    if spec.startswith("T="):
        return f"T{float(spec[2:]):g}"
    if spec in EFAS or spec == "observed" or spec.startswith("risk"):
        return spec.replace(">=", "_ge_")
    return Path(spec).stem


def flood_mask(area, spec, xy, dem_check=True, model=None):
    """Which points are flooded under a flood spec: observed | rp10 | rp100 | rp500 |
    T=<return period> | risk>=<t> | path to a polygon file (any CRS)."""
    d = spec_depth(area, spec, xy, dem_check)
    if d is not None:
        return d >= IMPASSABLE_M
    if spec == "observed":
        if area.flood.is_empty:
            raise ValueError(f"{area.name}: no observed flood (only CEMS AOIs have one; use rp10/rp100/rp500 or T=<years>)")
        return shapely.contains_xy(area.flood, xy[:, 0], xy[:, 1])
    if spec.startswith("risk>="):
        return (model or load_model()).predict_proba(area.features(xy)[FEATURES])[:, 1] >= float(spec[6:])
    poly = gpd.read_file(spec).to_crs(3035).union_all()
    return shapely.contains_xy(poly, xy[:, 0], xy[:, 1])


def buildings_in(region):
    """GHS-OBAT buildings inside the region, from every country file that overlaps it."""
    w, s, e, n = gpd.GeoSeries([region], crs=3035).to_crs(4326).total_bounds
    frames = []
    for path in sorted(DATA.glob("obat_*/*/*.gpkg")):
        b = gpd.read_file(path, bbox=(w, s, e, n), engine="pyogrio", columns=["height"])
        if len(b):
            frames.append(b.to_crs(3035))
    b = pd.concat(frames, ignore_index=True)
    xy = shapely.get_coordinates(b.geometry.values)
    inside = shapely.contains_xy(region, xy[:, 0], xy[:, 1])
    return b[inside].reset_index(drop=True), xy[inside]


def occupants_of(bxy, region):
    """Each GHS-POP pixel's population split evenly over the buildings inside it."""
    w, s, e, n = gpd.GeoSeries([region], crs=3035).to_crs(4326).total_bounds
    with rasterio.open(POP) as src:
        win = from_bounds(w, s, e, n, src.transform)
        win = Window(int(np.floor(win.col_off)), int(np.floor(win.row_off)), int(np.ceil(win.width)) + 1, int(np.ceil(win.height)) + 1)
        pop = np.maximum(src.read(1, window=win, boundless=True, fill_value=0), 0)
        col, row = ~src.window_transform(win) * TO_LONLAT.transform(bxy[:, 0], bxy[:, 1])
    row = np.clip(np.floor(row).astype(int), 0, pop.shape[0] - 1)
    col = np.clip(np.floor(col).astype(int), 0, pop.shape[1] - 1)
    pixel = row * pop.shape[1] + col
    return pop[row, col] / np.bincount(pixel)[pixel]


def context(area):
    """Everything about a region that does not depend on the flood: road graph, help bases,
    buildings, occupants, terrain risk. None if the region has no roads."""
    region = area.aoi
    roads = osm_data.layer("roads", region)
    water = osm_data.layer("water", region)
    if roads.empty:
        print(f"{area.name}: no roads, skipped")
        return None
    net = Network(roads, water)
    entry = np.flatnonzero(shapely.dwithin(region.boundary, shapely.points(net.xy), ENTRY_M))
    fac = osm_data.layer("facilities", region)
    fdist, fnode = net.tree.query(shapely.get_coordinates(fac.geometry.values)) if len(fac) else (np.array([]), np.array([], int))
    bases = np.unique(fnode[fdist <= FACILITY_SNAP_M])
    sources = np.union1d(entry, bases)
    dry = net.distances(np.ones(len(net.length), bool), sources)

    b, bxy = buildings_in(region)
    snap, node = net.tree.query(bxy)
    risk_tif = risk_model.OUT / f"risk_{area.name}.tif"
    model = load_model()
    # held-out map for training AOIs (honest), the final model anywhere else
    terrain = sample_raster(risk_tif, bxy) if risk_tif.exists() else model.predict_proba(area.features(bxy)[FEATURES])[:, 1]
    return dict(net=net, entry=entry, bases=bases, sources=sources, dry=dry, b=b, bxy=bxy, snap=snap, node=node,
                assessed=(snap <= SNAP_M) & np.isfinite(dry[node]),
                single=np.maximum(1, np.round(b.height.to_numpy() / STOREY_M)) == 1,
                occupants=occupants_of(bxy, region), terrain=terrain, model=model,
                cell=pd.MultiIndex.from_arrays([np.floor(bxy[:, 0] / CELL_M).astype(int), np.floor(bxy[:, 1] / CELL_M).astype(int)]))


def state(ctx, area, spec, dem_check=True):
    """Flood-dependent arrays for one flood spec: closed roads and, per building, flooded / cut off /
    bridge-dependent / detour / distance to help / depth and the evacuation categories."""
    net, node, assessed, dry = ctx["net"], ctx["node"], ctx["assessed"], ctx["dry"]
    edge_wet = flood_mask(area, spec, net.mid, dem_check, ctx["model"])
    b_wet = flood_mask(area, spec, ctx["bxy"], dem_check, ctx["model"])
    d_hold = net.distances(~edge_wet | net.bridge, ctx["sources"])  # flooded bridges stay usable
    d_fail = net.distances(~edge_wet, ctx["sources"])                 # flooded bridges are lost
    d_help = net.distances(~edge_wet | net.bridge, ctx["bases"])      # from fire stations / hospitals only
    reach_hold, reach_fail = np.isfinite(d_hold[node]), np.isfinite(d_fail[node])
    cut = assessed & ~reach_hold
    bridge_dep = assessed & reach_hold & ~reach_fail
    with np.errstate(invalid="ignore", divide="ignore"):
        detour = np.where(assessed & reach_hold & (dry[node] > 0), d_hold[node] / np.maximum(dry[node], 1), np.nan)
    help_km = np.where(assessed & np.isfinite(d_help[node]), (d_help[node] + ctx["snap"]) / 1000, np.nan)

    # Evacuation categories per building; depth is only known for EFAS floods
    depth = spec_depth(area, spec, ctx["bxy"], dem_check)
    depth = np.full(len(b_wet), np.nan) if depth is None else depth
    with np.errstate(invalid="ignore"):
        must = b_wet & ((depth >= MUST_DEPTH_M) | ctx["single"])
        early = ~must & (b_wet | cut)
        prepare = ~must & ~early & (ctx["terrain"] >= 0.5) & (bridge_dep | (detour >= 2) | (help_km >= 15))
    return dict(edge_wet=edge_wet, b_wet=b_wet, cut=cut, bridge_dep=bridge_dep, detour=detour, help_km=help_km,
                depth=depth, must=must, early=early, prepare=prepare)


def analyse(area, floods, dem_check=True):
    """Factor table per 1 km cell for each flood spec, for an Area (CEMS AOI or any region)."""
    ctx = context(area)
    if ctx is None:
        return []
    occupants, net = ctx["occupants"], ctx["net"]
    out = []
    for spec in floods:
        st = state(ctx, area, spec, dem_check)
        b_wet, cut, must, early, prepare = st["b_wet"], st["cut"], st["must"], st["early"], st["prepare"]
        df = pd.DataFrame(dict(wet=b_wet, cut=cut, bridge=st["bridge_dep"], assessed=ctx["assessed"], detour=st["detour"],
                               help_km=st["help_km"], single_wet=np.where(b_wet, ctx["single"], np.nan), terrain=ctx["terrain"],
                               occupants=occupants, affected=np.where(b_wet | cut, occupants, 0),
                               must=np.where(must, occupants, 0), early=np.where(early, occupants, 0),
                               prepare=np.where(prepare, occupants, 0), air=np.where((must | early) & cut, occupants, 0)),
                          index=ctx["cell"])
        g = df.groupby(level=[0, 1])
        cells = pd.DataFrame(dict(
            buildings=g.size(),
            flooded_pct=100 * g.wet.mean(),
            cut_off_pct=100 * g.cut.sum() / g.assessed.sum().replace(0, np.nan),
            bridge_pct=100 * g.bridge.sum() / g.assessed.sum().replace(0, np.nan),
            detour=g.detour.median(),
            help_km=g.help_km.median(),
            population=g.occupants.sum(),
            people=g.affected.sum(),
            single_storey_pct=100 * g.single_wet.mean().where(g.wet.sum() >= 5),
            likelihood=g.terrain.mean(),
            must_people=g.must.sum(), early_people=g.early.sum(), prepare_people=g.prepare.sum(), air_people=g.air.sum(),
        ))
        cells = evacuation(explain(cells.reset_index(names=["cx", "cy"])))
        label = flood_label(spec)
        cells.insert(0, "flood", label)
        STATS[label] = dict(buildings=len(b_wet), buildings_flooded=int(b_wet.sum()), buildings_cut_off=int(cut.sum()),
                            roads_closed_km=float(net.length[st["edge_wet"]].sum() / 1000), road_km=float(net.length.sum() / 1000))
        cells.insert(0, "area", area.name)
        out.append(cells)
        print(f"{area.name} [{cells.flood.iloc[0]}]: {len(b_wet):,} buildings, {b_wet.sum():,} flooded, {cut.sum():,} cut off, "
              f"{st['bridge_dep'].sum():,} bridge-dependent, {len(ctx['bases'])} help bases, {(cells.overall != 'Low').sum()} cells above Low",
              flush=True)
    return out


def to_gdf(cells):
    geom = shapely.box(cells.cx * CELL_M, cells.cy * CELL_M, (cells.cx + 1) * CELL_M, (cells.cy + 1) * CELL_M)
    gdf = gpd.GeoDataFrame(cells.drop(columns=["cx", "cy"]), geometry=geom, crs=3035)
    lon, lat = TO_LONLAT.transform(*shapely.get_coordinates(gdf.geometry.centroid).T)
    gdf["lon"], gdf["lat"] = lon.round(4), lat.round(4)
    order = pd.Categorical(gdf.overall, ["Critical", "High", "Elevated", "Low"], ordered=True)
    return gdf.assign(_o=order).sort_values(["flood", "_o", "people"], ascending=[True, True, False]).drop(columns="_o")


def write(gdf, folder):
    folder.mkdir(parents=True, exist_ok=True)
    for spec, g in gdf.groupby("flood"):
        g = evacuation(g.reset_index(drop=True))  # priority ranked across all areas of this flood
        g.to_file(folder / "access_danger.gpkg", layer=spec, driver="GPKG")
        g[g.overall != "Low"].drop(columns="geometry").round(2).to_csv(folder / f"danger_{spec}.csv", index=False)
        cols = ["evac_priority", "evac_level", "area", "lat", "lon", "population", "must_people", "early_people",
                "air_people", "prepare_people", "evac_reasons", "overall", "reasons"]
        g[g.evac_level != "None"].sort_values("evac_priority")[cols].round(1).to_csv(folder / f"evacuation_{spec}.csv", index=False)


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("folders", nargs="*", type=Path, help="CEMS PRODUCT folders (default: all)")
    p.add_argument("--flood", nargs="+", default=["observed", "rp10", "rp100", "rp500"],
                   help="observed | rp10 | rp100 | rp500 | risk>=0.5 | polygon file")
    p.add_argument("--no-dem-check", action="store_true", help="use raw 90 m EFAS depth (no 30 m DEM check)")
    args = p.parse_args()

    folders = args.folders or sorted(f for d in sorted(DATA.glob("ems_*")) for f in d.glob("*_PRODUCT_*_vector"))
    cells = []
    for f in folders:
        area = Area(f)
        cells += analyse(area, args.flood, dem_check=not args.no_dem_check)
        del area
    gdf = to_gdf(pd.concat(cells, ignore_index=True))
    write(gdf, OUT)

    pd.set_option("display.width", 200)
    print("\nCells per overall level (overall = number of factors at HIGH: ≥3 Critical, 2 High, 1 or ≥2 medium Elevated)")
    print(pd.crosstab(gdf["flood"], gdf.overall).reindex(columns=["Critical", "High", "Elevated", "Low"], fill_value=0).to_string())
    print("\nPeople in flooded or cut-off buildings:", gdf.groupby("flood").people.sum().round().to_dict())
    ev = gdf.groupby("flood")[["must_people", "early_people", "air_people", "prepare_people"]].sum().round()
    print("\nEvacuation (people): now / early / of which by boat or air / prepare\n" + ev.to_string())
    print(f"\nWrote {OUT}/access_danger.gpkg (one layer per flood) and danger_<flood>.csv")
