"""Map layers for one region, for the browser: buildings, town names and shelters with an estimated size.

  python3 area_layers.py --region 5.85,50.58,5.9,50.6 --name demo

Writes outputs/layers/<name>/ (served by GET /files):
  buildings.geojson  GHS-OBAT buildings as points: h height m, fl floors (height / 3 m), use (OBAT code:
                     1 residential, 2 non-residential, see the GHS-OBAT PDF), a footprint m², pop people
                     (GHS-POP split over the buildings of each pixel, as access_model), d10 / d100 / d500
                     water depth m at the building (EFAS, DEM-checked, as /flood)
  places.geojson     OSM city / town / village / suburb / hamlet names (population when tagged)
  shelters.geojson   OSM shelters, assembly points, schools and sports halls; cap = estimated places:
                     floor area of the GHS-OBAT buildings inside the mapped outline (or the building at
                     the point) × USABLE / M2_PER_PERSON. An assumption, not a capacity anyone declared.
  summary.json       counts, assumptions, file paths
"""
import argparse, json, sys
import geopandas as gpd, numpy as np, pandas as pd, shapely
from pyproj import Transformer

import access_model, flood_tool, osm_data, region as study
from paths import DATA, OUTPUTS
from risk_model import Area

OUT = OUTPUTS / "layers"
MAX_BUILDINGS = 300_000      # ~40 MB of GeoJSON; split bigger regions
STOREY_M = 3.0
USABLE = 0.5                 # share of the floor area people can sleep on (corridors, toilets, kitchens excluded): assumption
M2_PER_PERSON = 3.5          # Sphere minimum covered living space per person in shelters
POINT_SNAP_M = 25            # a shelter mapped as a point takes the GHS-OBAT building within this distance
TO_LONLAT = Transformer.from_crs(3035, 4326, always_xy=True)


def obat(region):
    """GHS-OBAT buildings in the region with the columns the layer needs (EPSG:3035 points)."""
    w, s, e, n = gpd.GeoSeries([region], crs=3035).to_crs(4326).total_bounds
    frames = [gpd.read_file(p, bbox=(w, s, e, n), engine="pyogrio", columns=["height", "use", "area"]).to_crs(3035)
              for p in sorted(DATA.glob("obat_*/*/*.gpkg"))]
    b = pd.concat([f for f in frames if len(f)], ignore_index=True) if any(len(f) for f in frames) else gpd.GeoDataFrame(geometry=[], crs=3035)
    if not len(b):
        return b, np.empty((0, 2))
    xy = shapely.get_coordinates(b.geometry.values)
    inside = shapely.contains_xy(region, xy[:, 0], xy[:, 1])
    return b[inside].reset_index(drop=True), xy[inside]


def points_fc(xy, props):
    """FeatureCollection of points from EPSG:3035 coordinates and a list of property dicts."""
    lon, lat = TO_LONLAT.transform(xy[:, 0], xy[:, 1]) if len(xy) else ([], [])
    return dict(type="FeatureCollection", features=[
        dict(type="Feature", geometry=dict(type="Point", coordinates=[round(float(x), 6), round(float(y), 6)]), properties=p)
        for x, y, p in zip(lon, lat, props)])


def num(v, digits):
    return None if v is None or not np.isfinite(v) else round(float(v), digits)


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0], formatter_class=argparse.RawDescriptionHelpFormatter, epilog=__doc__)
    p.add_argument("--region", required=True, help="w,s,e,n | polygon file | CEMS AOI name")
    p.add_argument("--name", required=True)
    args = p.parse_args()

    geom, aoi_name = flood_tool.parse_region(args.region)
    outside = 1 - geom.intersection(study.hull()).area / geom.area
    if outside > 0.01:
        sys.exit(f"{outside:.0%} of the region lies outside the study region; the models were not built for it.")
    if geom.area / 1e6 > flood_tool.MAX_KM2:
        sys.exit(f"region is {geom.area / 1e6:,.0f} km²; split it into parts of at most {flood_tool.MAX_KM2:,} km²")

    b, bxy = obat(geom)
    if len(b) > MAX_BUILDINGS:
        sys.exit(f"{len(b):,} buildings in this region; at most {MAX_BUILDINGS:,} per request, split it into smaller parts")
    out = OUTPUTS / "layers" / args.name
    out.mkdir(parents=True, exist_ok=True)

    # buildings
    if len(b):
        area = Area(flood_tool.cems_folder(aoi_name)) if aoi_name else Area(region=geom, name=args.name)
        depth = {rp: area.depth(rp, bxy) for rp in ("rp10", "rp100", "rp500")}
        pop = access_model.occupants_of(bxy, geom)
    floors = np.maximum(1, np.round(b.height.to_numpy() / STOREY_M)) if len(b) else np.array([])
    props = [dict(h=num(b.height.iat[i], 1), fl=int(floors[i]), use=int(b.use.iat[i]) if pd.notna(b.use.iat[i]) else None,
                  a=num(b["area"].iat[i], 0), pop=num(pop[i], 2), d10=num(depth["rp10"][i], 2), d100=num(depth["rp100"][i], 2),
                  d500=num(depth["rp500"][i], 2)) for i in range(len(b))]
    (out / "buildings.geojson").write_text(json.dumps(points_fc(bxy, props), separators=(",", ":")))

    # places
    pl = osm_data.layer("places", geom)
    pl = pl[shapely.contains_xy(geom, *shapely.get_coordinates(pl.geometry.values).T)] if len(pl) else pl
    pl = pl.sort_values("population", ascending=False, na_position="last")
    (out / "places.geojson").write_text(json.dumps(points_fc(
        shapely.get_coordinates(pl.geometry.values),
        [dict(name=r.name, place=r.place, population=None if pd.isna(r.population) else int(r.population)) for r in pl.itertuples()]),
        separators=(",", ":"), ensure_ascii=False))

    # shelters: floor area of the buildings inside the outline, or of the building at the point
    sh = osm_data.layer("shelters", geom)
    pt = sh.geometry.representative_point()
    sh = sh[shapely.contains_xy(geom, pt.x.to_numpy(), pt.y.to_numpy())].reset_index(drop=True) if len(sh) else sh
    floor_area = (b["area"].fillna(0).to_numpy() * floors) if len(b) else np.array([])
    tree = shapely.STRtree(shapely.points(bxy)) if len(b) else None
    rows, sxy = [], []
    for r in sh.itertuples():
        g = r.geometry
        if tree is None:
            idx = []
        elif g.geom_type in ("Polygon", "MultiPolygon"):
            idx = tree.query(g, predicate="contains")
        else:
            idx = tree.query(g, predicate="dwithin", distance=POINT_SNAP_M)
            idx = idx[np.argsort(shapely.distance(shapely.points(bxy[idx]), g))[:1]] if len(idx) else idx
        fa = float(floor_area[idx].sum()) if len(idx) else 0.0
        c = g.representative_point()
        sxy.append((c.x, c.y))
        rows.append(dict(name=r.name if isinstance(r.name, str) else None, kind=r.kind, tag=r.tag,
                         floor_m2=round(fa) if fa else None, cap=int(fa * USABLE / M2_PER_PERSON) if fa else None))
    (out / "shelters.geojson").write_text(json.dumps(points_fc(np.array(sxy).reshape(-1, 2), rows), separators=(",", ":"), ensure_ascii=False))

    with_cap = [r["cap"] for r in rows if r["cap"]]
    summary = dict(
        name=args.name, region=args.region, area_km2=round(geom.area / 1e6, 2),
        buildings=len(b), people=round(float(pop.sum())) if len(b) else 0,
        buildings_wet=dict({rp: int((depth[rp] >= access_model.IMPASSABLE_M).sum()) for rp in depth}) if len(b) else {},
        places=len(pl), shelters=len(rows), shelters_with_capacity=len(with_cap), shelter_places_est=int(sum(with_cap)),
        assumptions=dict(storey_m=STOREY_M, usable_share=USABLE, m2_per_person=M2_PER_PERSON, point_snap_m=POINT_SNAP_M),
        caveats=["Shelter capacity is estimated from building size (GHS-OBAT floor area × usable share / m² per person), "
                 "not declared by anyone; candidate_shelter = schools and sports halls, not confirmed shelters.",
                 "Building depth: EFAS scenario maps checked against the DEM, not the 2021 event.",
                 "People per building: GHS-POP 2020 split evenly over the buildings of each ~1 km pixel."],
        files={k: str(out / f"{k}.geojson") for k in ("buildings", "places", "shelters")})
    (out / "summary.json").write_text(json.dumps(summary, indent=2))
    print(json.dumps({k: v for k, v in summary.items() if k not in ("caveats", "assumptions")}, indent=2))


if __name__ == "__main__":
    main()
