"""Browser copies (lon/lat GeoJSON) of the stored road_model / interp_model results, one file per AOI.

  outputs/roads/geojson/<AOI>.geojson     road pieces of roads_at_risk.gpkg: class, name, length, and per scenario
                                          whether the piece is under water (observed, rp10/100/500), risk, damage_prob
  outputs/interp/geojson/<pair>.geojson   extents.gpkg layer per interval: one (multi)polygon per step (time, hours)

Served by GET /files and listed in GET /roads/{aoi} and GET /interp/{aoi} (files.*_geojson). Rerun after
road_model.py or interp_model.py.
"""
import geopandas as gpd, pyogrio, shapely

from paths import OUTPUTS

ROADS, INTERP = OUTPUTS / "roads", OUTPUTS / "interp"


def write(gdf, path, precision):
    g = gdf.to_crs(4326)
    g["geometry"] = shapely.set_precision(g.geometry.values, precision)
    g = g[~g.geometry.is_empty]
    path.parent.mkdir(parents=True, exist_ok=True)
    g.to_file(path, driver="GeoJSON", COORDINATE_PRECISION=6)
    return len(g)


def main():
    roads = gpd.read_file(ROADS / "roads_at_risk.gpkg", engine="pyogrio")
    for area, g in roads.groupby("area"):
        n = write(g.drop(columns="area").round(3), ROADS / "geojson" / f"{area}.geojson", 1e-5)
        print(f"roads {area}: {n:,} pieces")
    for pair, _ in pyogrio.list_layers(INTERP / "extents.gpkg"):
        g = gpd.read_file(INTERP / "extents.gpkg", layer=pair, engine="pyogrio")
        g["geometry"] = g.geometry.simplify(5)  # m, before reprojecting
        n = write(g, INTERP / "geojson" / f"{pair}.geojson", 1e-5)
        print(f"interp {pair}: {n} steps")


if __name__ == "__main__":
    main()
