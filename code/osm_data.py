"""OpenStreetMap layers for the study region, extracted once from the Geofabrik files in working_data/osm.

Layers in cache/osm_study_region.gpkg (EPSG:3035):
  waterways   river / stream / canal lines (same filter as flood_api, notes/08)
  water       natural=water polygons (lakes, reservoirs, river surfaces)
  roads       drivable highways with road_class and bridge flag
  facilities  fire stations and hospitals as points
"""
import argparse
import geopandas as gpd, pandas as pd, shapely

from paths import CACHE, DATA

FILE = CACHE / "osm_study_region.gpkg"
PAD_DEG = 0.05
WATERWAYS = ("river", "stream", "canal")
HIGHWAY_CLASS = {
    "motorway": "Highway", "trunk": "Highway", "motorway_link": "Highway", "trunk_link": "Highway",
    "primary": "Primary road", "primary_link": "Primary road",
    "secondary": "Secondary road", "secondary_link": "Secondary road",
    **{k: "Local road" for k in ("tertiary", "tertiary_link", "unclassified", "residential", "living_street", "service")},
}
AMENITIES = ("fire_station", "hospital")


def _sql_in(values):
    return "(" + ", ".join(f"'{v}'" for v in values) + ")"


def _read(path, layer, where, bbox):
    return gpd.read_file(path, layer=layer, where=where, bbox=bbox, engine="pyogrio")


def build():
    from region import hull
    w, s, e, n = gpd.GeoSeries([hull()], crs=3035).to_crs(4326).total_bounds + [-PAD_DEG, -PAD_DEG, PAD_DEG, PAD_DEG]
    parts = {k: [] for k in ("waterways", "water", "roads", "facilities")}
    for pbf in sorted((DATA / "osm").glob("*.osm.pbf")):
        print(f"  {pbf.name}", flush=True)
        lines = _read(pbf, "lines", f"waterway IN {_sql_in(WATERWAYS)} OR highway IN {_sql_in(HIGHWAY_CLASS)}", (w, s, e, n))
        parts["waterways"].append(lines[lines.waterway.isin(WATERWAYS)][["osm_id", "waterway", "geometry"]])
        roads = lines[lines.highway.isin(list(HIGHWAY_CLASS))].copy()
        roads["road_class"] = roads.highway.map(HIGHWAY_CLASS)
        tags = roads.other_tags.fillna("")
        roads["bridge"] = tags.str.contains('"bridge"=>"', regex=False) & ~tags.str.contains('"bridge"=>"no"', regex=False)
        parts["roads"].append(roads[["osm_id", "highway", "road_class", "bridge", "geometry"]])

        polys = _read(pbf, "multipolygons", f"natural = 'water' OR amenity IN {_sql_in(AMENITIES)}", (w, s, e, n))
        polys["osm_id"] = polys.osm_id.fillna(polys.osm_way_id)
        parts["water"].append(polys[polys.natural == "water"][["osm_id", "geometry"]])
        fac = polys[polys.amenity.isin(AMENITIES)][["osm_id", "name", "amenity", "geometry"]].copy()
        fac["geometry"] = fac.geometry.representative_point()
        pts = _read(pbf, "points", " OR ".join(f"other_tags LIKE '%\"amenity\"=>\"{a}\"%'" for a in AMENITIES), (w, s, e, n))
        pts["amenity"] = pts.other_tags.str.extract(r'"amenity"=>"([^"]+)"')[0]
        parts["facilities"] += [fac, pts[pts.amenity.isin(AMENITIES)][["osm_id", "name", "amenity", "geometry"]]]

    FILE.parent.mkdir(parents=True, exist_ok=True)
    for name, frames in parts.items():
        g = pd.concat(frames, ignore_index=True)
        g = g.drop_duplicates("osm_id")  # extracts overlap at their borders
        g = gpd.GeoDataFrame(g, geometry="geometry", crs=4326).to_crs(3035)
        g.to_file(FILE, layer=name, driver="GPKG")
        print(f"  {name}: {len(g):,}")


def layer(name, region):
    """One layer clipped to the bounding box of `region` (EPSG:3035), buffered by 2 km."""
    if not FILE.exists():
        build()
    x0, y0, x1, y1 = region.buffer(2000).bounds
    return gpd.read_file(FILE, layer=name, bbox=(x0, y0, x1, y1), engine="pyogrio")


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.parse_args()
    build()
