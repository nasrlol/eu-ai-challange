import argparse, geopandas as gpd, pandas as pd, rasterio, numpy as np
from rasterio.mask import mask
from pathlib import Path

from paths import DATA

DEPTH = DATA / "cems-efas/Europe_RP100_filled_depth.tif"
POP = DATA / "ghs_pop/GHS_POP_E2020_GLOBE_R2023A_4326_30ss_V1_0/GHS_POP_E2020_GLOBE_R2023A_4326_30ss_V1_0.tif"


def flood_file(path):
    """A file is used as-is; a CEMS folder resolves to its observedEventA layer."""
    if path.is_file():
        return path
    for ext in ("shp", "json"):
        if hits := sorted(path.glob(f"*observedEventA*.{ext}")):
            return hits[0]
    raise SystemExit(f"{path}: no observedEventA layer found")


def guess_buildings(path):
    """ems_de/... -> obat_de/**/*.gpkg, if that folder exists."""
    for parent in path.resolve().parents:
        if parent.name.startswith("ems_"):
            return next((DATA / ("obat_" + parent.name[4:])).glob("**/*.gpkg"), None)
    return None


def general(flood, path):
    print(f"File:      {path}")
    print(f"CRS:       {flood.crs}")
    print(f"Features:  {len(flood)} ({', '.join(flood.geom_type.unique())})")
    print(f"Area:      {flood.to_crs(3035).area.sum() / 1e6:.3f} km²")
    print("Bounds:    lon {:.4f}..{:.4f}, lat {:.4f}..{:.4f}".format(*flood.to_crs(4326).total_bounds[[0, 2, 1, 3]]))
    for col in flood.columns.drop("geometry"):
        if not pd.api.types.is_numeric_dtype(flood[col]) and flood[col].nunique() <= 10:
            counts = ", ".join(f"{k} ({v})" for k, v in flood[col].value_counts().items())
            print(f"{col + ':':<10} {counts}")


def buildings(flood, path):
    # Only read buildings inside the AOI's bounding box (the DE file is 8 GB, so never load it all)
    bld = gpd.read_file(path, bbox=tuple(flood.total_bounds), engine="pyogrio")
    hit = gpd.sjoin(bld, flood[["geometry"]], predicate="within").drop_duplicates("id")
    print(f"\n{len(hit)} flooded buildings (of {len(bld)} in the bounding box)")
    if hit.empty:
        return hit
    print(hit.groupby(["use", "epoch"]).size().to_string())
    print(f"Footprint: {hit['area'].sum():.0f} m², "
          f"mean height {hit.height.mean():.1f} m")
    return hit


def depth(hit, path):
    with rasterio.open(path) as r:
        d = np.array([v[0] for v in r.sample(zip(hit.geometry.x, hit.geometry.y))], dtype=float)
    d[d < 0] = np.nan
    if np.isnan(d).all():
        print("RP100 depth: no modelled depth at these buildings")
    else:
        print(f"RP100 depth at flooded buildings: mean {np.nanmean(d):.2f} m, max {np.nanmax(d):.2f} m, "
              f"{np.isnan(d).sum()} outside the modelled extent")


def population(flood, path):
    with rasterio.open(path) as r:
        arr, _ = mask(r, flood.to_crs(r.crs).geometry, crop=True, nodata=0)
    print(f"{arr.sum():.0f} people (approx.)")


if __name__ == "__main__":
    p = argparse.ArgumentParser(description="Summarise CEMS flood extents, with building impact where available.")
    p.add_argument("flood", nargs="+", type=Path, help="observedEventA file, or a CEMS vector folder")
    p.add_argument("--buildings", type=Path, help="GHS-OBAT .gpkg (default: guessed from the ems_XX folder)")
    p.add_argument("--no-buildings", action="store_true", help="skip the building analysis")
    p.add_argument("--depth", type=Path, default=DEPTH, help="RP100 depth raster")
    p.add_argument("--pop", type=Path, default=POP, help="population raster")
    args = p.parse_args()

    for i, arg in enumerate(args.flood):
        if i:
            print("\n" + "-" * 60 + "\n")
        path = flood_file(arg)
        flood = gpd.read_file(path).to_crs(4326)
        general(flood, path)

        bld_path = None if args.no_buildings else args.buildings or guess_buildings(path)
        if bld_path and bld_path.exists():
            hit = buildings(flood, bld_path)
            if len(hit) and args.depth.exists():
                depth(hit, args.depth)
        else:
            print("\nNo building data available; skipping building analysis")

        if args.pop.exists():
            population(flood, args.pop)
