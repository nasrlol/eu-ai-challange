"""Flood impact model chain: hazard -> exposure -> vulnerability -> loss -> people.

Each stage is a function that takes the shared state dict, adds to it, and
returns it, so stages can be swapped or reordered without touching the others.
"""
import argparse, json, geopandas as gpd, numpy as np, rasterio
from pathlib import Path
from main import DEPTH, flood_file, guess_buildings

# JRC global depth-damage curves, Europe (Huizinga et al. 2017): depth (m) -> damage fraction
DEPTHS = [0, 0.5, 1, 1.5, 2, 3, 4, 5, 6]
CURVES = {
    "residential": [0, 0.25, 0.40, 0.50, 0.60, 0.75, 0.85, 0.95, 1.0],
    "commercial":  [0, 0.15, 0.30, 0.45, 0.55, 0.75, 0.90, 1.00, 1.0],
}
# GHS-OBAT `use` codes (assumed: 1 residential, 2 non-residential, 0 unknown -> residential)
USE = {0: "residential", 1: "residential", 2: "commercial"}

STOREY_M = 3.0         # storey height used to turn building height into floors
M2_PER_PERSON = 40.0   # residential floor area per occupant
DISPLACED_M = 0.5      # water depth at which occupants are counted as displaced


def hazard(s):
    """Observed flood extent + modelled depth raster."""
    s["flood"] = gpd.read_file(s["flood_path"]).to_crs(4326)
    print(f"[hazard]        {len(s['flood'])} flood polygons, {s['flood'].to_crs(3035).area.sum() / 1e6:.2f} km²")
    return s


def exposure(s):
    """Buildings in the AOI; flooded = inside the observed extent, depth sampled from the raster."""
    bld = gpd.read_file(s["buildings_path"], bbox=tuple(s["flood"].total_bounds), engine="pyogrio")
    inside = gpd.sjoin(bld, s["flood"][["geometry"]], predicate="within").index.unique()
    bld["flooded"] = bld.index.isin(inside)
    bld["depth"] = 0.0
    if s["depth_path"].exists() and bld.flooded.any():
        pts = bld[bld.flooded].geometry
        with rasterio.open(s["depth_path"]) as r:
            d = np.array([v[0] for v in r.sample(zip(pts.x, pts.y))], dtype=float)
        bld.loc[bld.flooded, "depth"] = np.clip(np.nan_to_num(np.where(d < 0, 0, d)), 0, s["max_depth"])
    s["buildings"] = bld
    print(f"[exposure]      {bld.flooded.sum()} of {len(bld)} buildings flooded, "
          f"mean depth {bld.loc[bld.flooded, 'depth'].mean():.2f} m")
    return s


def vulnerability(s):
    """Depth -> damage fraction via the curve for each building's use."""
    bld = s["buildings"]
    bld["class"] = bld["use"].map(USE).fillna("residential")
    bld["damage_frac"] = 0.0
    for cls, curve in CURVES.items():
        m = bld["class"] == cls
        bld.loc[m, "damage_frac"] = np.interp(bld.loc[m, "depth"], DEPTHS, curve)
    hit = bld[bld.flooded]
    print(f"[vulnerability] mean damage fraction {hit.damage_frac.mean():.2f}, "
          f"{(hit.damage_frac >= 0.5).sum()} buildings ≥ 50% damaged")
    return s


def loss(s):
    """Damage fraction x replacement value (floor area x unit cost)."""
    bld = s["buildings"]
    bld["floors"] = np.maximum(1, np.round(bld["height"] / STOREY_M))
    bld["floor_area"] = bld["area"] * bld["floors"]
    bld["loss_eur"] = bld["damage_frac"] * bld["floor_area"] * s["cost_m2"]
    print(f"[loss]          €{bld.loss_eur.sum() / 1e6:.2f} M direct building damage "
          f"(at €{s['cost_m2']:.0f}/m²)")
    return s


def people(s):
    """Occupants from residential floor area; displaced where depth exceeds the threshold."""
    bld = s["buildings"]
    bld["occupants"] = np.where(bld["class"] == "residential", bld["floor_area"] / M2_PER_PERSON, 0)
    affected = bld.loc[bld.flooded, "occupants"].sum()
    displaced = bld.loc[bld.depth >= DISPLACED_M, "occupants"].sum()
    print(f"[people]        ~{affected:.0f} occupants affected, ~{displaced:.0f} displaced (depth ≥ {DISPLACED_M} m)")
    return s


PIPELINE = [hazard, exposure, vulnerability, loss, people]


def run(flood_path, buildings_path, depth_path=DEPTH, cost_m2=1500.0, max_depth=6.0, stages=PIPELINE):
    s = dict(flood_path=flood_path, buildings_path=buildings_path, depth_path=depth_path,
             cost_m2=cost_m2, max_depth=max_depth)
    for stage in stages:
        s = stage(s)
    return s


if __name__ == "__main__":
    p = argparse.ArgumentParser(description="Run the flood impact model chain on CEMS flood extents.")
    p.add_argument("flood", nargs="+", type=Path, help="observedEventA file, or a CEMS vector folder")
    p.add_argument("--buildings", type=Path, help="GHS-OBAT .gpkg (default: guessed from the ems_XX folder)")
    p.add_argument("--depth", type=Path, default=DEPTH, help="depth raster")
    p.add_argument("--cost", type=float, default=1500.0, help="replacement cost, €/m² floor area")
    p.add_argument("--max-depth", type=float, default=6.0, help="cap on sampled depth, m")
    p.add_argument("-o", "--out", type=Path, help="write flooded buildings with all model columns to this .gpkg")
    p.add_argument("--json", type=Path, help="write one summary per flood to this .json (used by flood_api)")
    args = p.parse_args()

    summaries = []

    for i, arg in enumerate(args.flood):
        path = flood_file(arg)
        bld_path = args.buildings or guess_buildings(path)
        print(("\n" if i else "") + f"== {path.name}")
        if not (bld_path and bld_path.exists()):
            print("No building data available; run main.py for a general summary instead")
            continue
        s = run(path, bld_path, args.depth, args.cost, args.max_depth)
        if args.out:
            s["buildings"][s["buildings"].flooded].to_file(args.out, layer=path.stem, driver="GPKG")
        bld = s["buildings"]
        hit = bld[bld.flooded]
        summaries.append(dict(
            flood=path.name, flood_km2=s["flood"].to_crs(3035).area.sum() / 1e6, buildings=len(bld),
            buildings_flooded=len(hit), mean_depth_m=hit.depth.mean() if len(hit) else 0.0,
            buildings_ge_50pct_damaged=int((hit.damage_frac >= 0.5).sum()), loss_eur_m=hit.loss_eur.sum() / 1e6,
            people_affected=hit.occupants.sum(), people_displaced=bld.loc[bld.depth >= DISPLACED_M, "occupants"].sum(),
            assumptions=dict(cost_eur_m2=args.cost, max_depth_m=args.max_depth, m2_per_person=M2_PER_PERSON,
                             displaced_at_m=DISPLACED_M, depth="EFAS RP100 sampled inside the observed extent")))
    if args.json:
        args.json.write_text(json.dumps(summaries, indent=2, default=float))
