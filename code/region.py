"""Study region: the convex hull around all CEMS PRODUCT areas of interest.

Arbitrary floods are only estimated inside this hull; the AOIs themselves are where we have labels.
Writes outputs/region/study_region.gpkg with two layers: `hull` and `aois`.
"""
import geopandas as gpd, shapely
from main import DATA
from paths import OUTPUTS

OUT = OUTPUTS / "region" / "study_region.gpkg"


def aois():
    rows = []
    for folder in sorted(f for d in sorted(DATA.glob("ems_*")) for f in d.glob("*_PRODUCT_*_vector")):
        geom = gpd.read_file(next(folder.glob("*_areaOfInterestA_*.shp"))).to_crs(3035).union_all()
        rows.append(dict(area="_".join(folder.name.split("_")[:3]), geometry=geom))
    return gpd.GeoDataFrame(rows, crs=3035)


def hull():
    """The study region polygon (EPSG:3035); builds the file on first use."""
    if not OUT.exists():
        a = aois()
        OUT.parent.mkdir(parents=True, exist_ok=True)
        a.to_file(OUT, layer="aois", driver="GPKG")
        gpd.GeoDataFrame(dict(name=["study_region"]), geometry=[a.union_all().convex_hull], crs=3035).to_file(OUT, layer="hull", driver="GPKG")
    return gpd.read_file(OUT, layer="hull").geometry.iloc[0]


if __name__ == "__main__":
    h = hull()
    a = gpd.read_file(OUT, layer="aois")
    print(f"Study region {h.area / 1e6:,.0f} km², AOIs cover {a.union_all().area / h.area:.1%}; "
          f"lon/lat {gpd.GeoSeries([h], crs=3035).to_crs(4326).total_bounds.round(2)} -> {OUT}")
