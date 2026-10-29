"""Clean the Copernicus EMS flood products down to AOIs that have both a PRODUCT and a MONIT01.

Output: one GeoPackage with one layer per EMS layer type (all AOIs stacked, tagged with
emsr/aoi/country/ptype/stage/version) plus two derived layers per AOI:
  - aoi_boundary: union of the PRODUCT and MONIT01 area of interest
  - flood_extent: union of the PRODUCT and MONIT01 observed riverine flood

Usage: python preprocess.py [--data working_data] [--out processed/ems_clean.gpkg]
"""
import argparse
import re
import warnings
from pathlib import Path

from paths import DATA, ROOT

import geopandas as gpd
import pandas as pd
from shapely.validation import make_valid

warnings.filterwarnings("ignore", category=RuntimeWarning)

COUNTRY = {"ems_de": "DE", "ems_be": "BE", "ems_nl": "NL"}
STAGES = ("PRODUCT", "MONIT01")
LAYERS = [
    "areaOfInterestA",
    "observedEventA",
    "builtUpA",
    "builtUpP",
    "transportationL",
    "hydrographyA",
    "hydrographyL",
    "naturalLandUseA",
]
DIR_RE = re.compile(r"(EMSR\d+)_(AOI\d+)_(DEL|GRA)_(PRODUCT|MONIT\d+)_r\d+_\w*?v(\d+)_vector$")
CRS = "EPSG:4326"


def find_products(data_dir):
    rows = []
    for country_dir, cc in COUNTRY.items():
        for d in sorted((data_dir / country_dir).glob("*_vector")):
            m = DIR_RE.match(d.name)
            if not m:
                print(f"  skip (unrecognised name): {d.name}")
                continue
            emsr, aoi, ptype, stage, ver = m.groups()
            rows.append(dict(country=cc, emsr=emsr, aoi=aoi, ptype=ptype, stage=stage, version=int(ver), path=d))
    df = pd.DataFrame(rows)
    # if a stage exists in several versions, keep the latest
    return df.sort_values("version").groupby(["emsr", "aoi", "ptype", "stage"]).tail(1)


def select_complete(products):
    """Keep only AOIs (per EMSR + product type) that have every stage in STAGES."""
    has = products.groupby(["emsr", "aoi", "ptype"]).stage.agg(set)
    complete = has[has.apply(lambda s: set(STAGES) <= s)].index
    dropped = has.index.difference(complete)
    for emsr, aoi, ptype in dropped:
        print(f"  drop {emsr} {aoi} {ptype}: stages {sorted(has[(emsr, aoi, ptype)])}")
    keep = products.set_index(["emsr", "aoi", "ptype"]).loc[complete].reset_index()
    return keep[keep.stage.isin(STAGES)]


def clean(gdf):
    gdf = gdf[gdf.geometry.notna() & ~gdf.geometry.is_empty].copy()
    bad = ~gdf.geometry.is_valid
    gdf.loc[bad, "geometry"] = gdf.loc[bad, "geometry"].apply(make_valid)
    # make_valid can turn polygons into collections; keep only the geometry family we started with
    if len(gdf):
        family = gdf.geometry.geom_type.mode()[0].replace("Multi", "")
        gdf = gdf.explode(index_parts=False)
        gdf = gdf[gdf.geometry.geom_type == family]
    gdf = gdf.replace({"Not Applicable": pd.NA, "Unknown": pd.NA})
    gdf = gdf[~gdf.geometry.duplicated()]
    return gdf.to_crs(CRS)


def read_layer(prod, layer):
    shp = list(prod.path.glob(f"*_{layer}_*.shp"))
    if not shp:
        return None
    gdf = clean(gpd.read_file(shp[0], engine="pyogrio"))
    if layer == "observedEventA":
        # only riverine flood (drop the few mudflow/landslide polygons)
        gdf = gdf[gdf.obj_desc == "Riverine flood"]
    for col in ("country", "emsr", "aoi", "ptype", "stage", "version"):
        gdf[col] = prod[col]
    return gdf


def dissolve_per_aoi(gdf, name):
    keys = ["country", "emsr", "aoi", "ptype"]
    out = gdf[keys + ["geometry"]].dissolve(by=keys).reset_index()
    out["area_km2"] = out.to_crs("EPSG:3035").area / 1e6
    out["layer"] = name
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--data", type=Path, default=DATA)
    ap.add_argument("--out", type=Path, default=ROOT / "processed" / "ems_clean.gpkg")
    args = ap.parse_args()

    print("Scanning EMS products...")
    products = select_complete(find_products(args.data))
    print(f"Keeping {products.groupby(['emsr', 'aoi', 'ptype']).ngroups} AOIs with {' + '.join(STAGES)}:")
    for (emsr, aoi, ptype), g in products.groupby(["emsr", "aoi", "ptype"]):
        print(f"  {emsr} {aoi} {ptype}  " + ", ".join(f"{s} v{v}" for s, v in zip(g.stage, g.version)))

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.unlink(missing_ok=True)

    stacked = {}
    for layer in LAYERS:
        parts = [g for _, p in products.iterrows() if (g := read_layer(p, layer)) is not None and len(g)]
        if not parts:
            continue
        gdf = gpd.GeoDataFrame(pd.concat(parts, ignore_index=True), crs=CRS)
        stacked[layer] = gdf
        gdf.to_file(args.out, layer=layer, engine="pyogrio")

    boundary = dissolve_per_aoi(stacked["areaOfInterestA"], "aoi_boundary")
    flood = dissolve_per_aoi(stacked["observedEventA"], "flood_extent")
    boundary.to_file(args.out, layer="aoi_boundary", engine="pyogrio")
    flood.to_file(args.out, layer="flood_extent", engine="pyogrio")

    # summary per AOI and stage
    counts = pd.DataFrame({
        layer: gdf.groupby(["emsr", "aoi", "ptype", "stage"]).size() for layer, gdf in stacked.items()
    }).fillna(0).astype(int)
    summary = (
        boundary.set_index(["emsr", "aoi", "ptype"])[["country", "area_km2"]].rename(columns={"area_km2": "aoi_km2"})
        .join(flood.set_index(["emsr", "aoi", "ptype"])[["area_km2"]].rename(columns={"area_km2": "flood_km2"}))
        .round(2)
    )
    summary.to_csv(args.out.with_name("ems_summary.csv"))
    counts.to_csv(args.out.with_name("ems_layer_counts.csv"))

    pd.set_option("display.width", 200)
    print("\nPer AOI (union of both stages):\n", summary.to_string())
    print("\nFeatures per layer and stage:\n", counts.to_string())
    print(f"\nWrote {args.out}")


if __name__ == "__main__":
    main()
