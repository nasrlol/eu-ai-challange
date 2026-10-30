"""Estimate an arbitrary flood anywhere in the study region.

  python3 flood_tool.py --region 6.95,50.45,7.15,50.58 --flood rp100 --name ahr_test
  python3 flood_tool.py --region my_area.geojson --flood my_flood.gpkg risk>=0.5
  python3 flood_tool.py --region EMSR517_AOI03_DEL --flood observed rp100

Region: west,south,east,north in degrees, a polygon file (any CRS), or a CEMS AOI name.
Flood:  rp10 | rp100 | rp500 (EFAS, checked against the 30 m DEM) | risk>=<t> | polygon file |
        observed (CEMS AOI names only).
Writes outputs/regions/<name>/: risk.tif (relative terrain risk, 100 m), access_danger.gpkg and
danger_<flood>.csv (factor table per 1 km cell), evacuation_<flood>.csv (ranked), summary.json.
"""
import argparse, json, sys
import geopandas as gpd, numpy as np, pandas as pd, shapely

import access_model, region as study
from paths import DATA, OUTPUTS
from risk_model import FEATURES, GRID_M, Area, write_map

MAX_KM2 = 3000  # largest training AOI is ~2,900 km² (same limit as flood_api)
CAVEATS = [
    "Risk is a relative ranking learned from the July 2021 floods, not a probability.",
    "EFAS depths are 1-in-10/100/500-year scenarios at 90 m, checked against a 30 m surface model (includes buildings and trees).",
    "Roads, bridges, fire stations and hospitals are today's OpenStreetMap, not the 2021 state.",
    "Population is GHS-POP 2020 (1 km) split over GHS-OBAT buildings.",
    "Help is routed by road distance, not travel time; debris and damaged bridges are not modelled.",
    "Evacuation levels are fixed rules (depth, single-storey, cut off, people counts), not an optimisation; depth is only known for EFAS floods.",
    "CEMS maps (observed flood, training labels) are a snapshot when the satellite passed, not the peak; outside the mapped AOIs 'not flooded' means 'not mapped'.",
    "EFAS leaves out rivers with an upstream catchment under 500 km² (small streams, flash floods).",
    "The 1-in-500-year map is the ceiling of the scenarios; July 2021 went beyond it in places.",
    "GHS-OBAT undercounts buildings in Belgium, so Belgian building and people counts are low.",
    "GHS-POP counts residents only (no commuters or visitors).",
]


def parse_region(text):
    parts = text.split(",")
    if len(parts) == 4:
        w, s, e, n = map(float, parts)
        return gpd.GeoSeries([shapely.box(w, s, e, n)], crs=4326).to_crs(3035).iloc[0], None
    aois = gpd.read_file(study.OUT, layer="aois") if study.OUT.exists() else None
    if aois is not None and text in set(aois["area"]):
        return aois.loc[aois["area"] == text].geometry.iloc[0], text
    return gpd.read_file(text).to_crs(3035).union_all(), None


def cems_folder(aoi_name):
    return next(f for d in DATA.glob("ems_*") for f in d.glob(f"{aoi_name}_PRODUCT_*_vector"))


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0], formatter_class=argparse.RawDescriptionHelpFormatter, epilog=__doc__)
    p.add_argument("--region", required=True)
    p.add_argument("--flood", nargs="+", default=["rp100"])
    p.add_argument("--name", default=None, help="output folder name (default: AOI name or 'region')")
    p.add_argument("--allow-outside", action="store_true", help="clip to the study region instead of refusing")
    args = p.parse_args()

    geom, aoi_name = parse_region(args.region)
    hull = study.hull()
    outside = 1 - geom.intersection(hull).area / geom.area
    if outside > 0.01:
        if not args.allow_outside:
            sys.exit(f"{outside:.0%} of the region lies outside the study region (outputs/region/study_region.gpkg); "
                     "the models were not built for it. Use --allow-outside to clip.")
        geom = geom.intersection(hull)
    if geom.area / 1e6 > MAX_KM2:
        sys.exit(f"region is {geom.area / 1e6:,.0f} km²; split it into parts of at most {MAX_KM2:,} km²")
    if "observed" in args.flood and aoi_name is None:
        sys.exit("'observed' needs a CEMS AOI name as --region")

    name = args.name or aoi_name or "region"
    out = OUTPUTS / "regions" / name
    out.mkdir(parents=True, exist_ok=True)
    area = Area(cems_folder(aoi_name)) if aoi_name else Area(region=geom, name=name)

    # Relative terrain risk on a 100 m grid
    g = area.grid()
    risk = access_model.load_model().predict_proba(g["feats"][FEATURES])[:, 1]
    write_map(g, risk, out / "risk.tif")

    # Access danger per flood
    gdf = access_model.to_gdf(pd.concat(access_model.analyse(area, args.flood), ignore_index=True))
    access_model.write(gdf, out)

    summary = dict(
        name=name, region_km2=round(geom.area / 1e6, 1), outside_study_region_pct=round(100 * outside, 1),
        relative_risk=dict(mean=round(float(risk.mean()), 3), share_cells_ge_0_5=round(float((risk >= 0.5).mean()), 3)),
        floods={spec: dict(cells=g2.overall.value_counts().reindex(["Critical", "High", "Elevated", "Low"], fill_value=0).to_dict(),
                           people_flooded_or_cut_off=round(float(g2.people.sum())),
                           **{k: round(v, 2) if isinstance(v, float) else v
                              for k, v in access_model.STATS.get(spec, {}).items()},  # buildings, flooded, cut off, roads km
                           evacuation=dict(cells=g2.evac_level.value_counts().reindex(access_model.EVAC_LEVELS, fill_value=0).to_dict(),
                                           people_now=round(float(g2.must_people.sum())), people_early=round(float(g2.early_people.sum())),
                                           people_by_boat_or_air=round(float(g2.air_people.sum())),
                                           people_prepare=round(float(g2.prepare_people.sum())),
                                           first_cells=g2[g2.evac_priority > 0].sort_values("evac_priority").head(5)[
                                               ["evac_priority", "evac_level", "lat", "lon", "evac_reasons"]].to_dict("records")),
                           top_cells=g2.head(5)[["overall", "lat", "lon", "people", "reasons"]].round({"people": 1}).to_dict("records"))
                for spec, g2 in gdf.groupby("flood")},
        sources=dict(challenge=["Copernicus EMS", "EFAS RP10/100/500", "GHS-OBAT", "GHS-POP"],
                     external=["Copernicus DEM GLO-30", "OpenStreetMap (Geofabrik)"]),
        caveats=CAVEATS,
    )
    (out / "summary.json").write_text(json.dumps(summary, indent=2, ensure_ascii=False, default=int))
    print(json.dumps({k: v for k, v in summary.items() if k != "caveats"}, indent=2, ensure_ascii=False, default=int)[:3000])
    print(f"\nWrote {out}/: risk.tif, access_danger.gpkg, danger_<flood>.csv, evacuation_<flood>.csv, summary.json")
